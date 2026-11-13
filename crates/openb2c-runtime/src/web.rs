use crate::{
    AuthContext, Service,
    identity::{Certificate, Registry, fake_allowed, production, secret},
    service::{ListOptions, Result, ServiceError, etag},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use chrono::Utc;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Settings {
    pub auth_enabled: bool,
    pub max_body: usize,
    pub max_page: u32,
    pub timeout_ms: u64,
    pub cors: Vec<String>,
    pub version: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            auth_enabled: true,
            max_body: 1048576,
            max_page: 1000,
            timeout_ms: 30000,
            cors: vec!["*".into()],
            version: "0.1.0".into(),
        }
    }
}
impl Settings {
    pub fn from_environment() -> anyhow::Result<Self> {
        let positive = |name: &str, default: u64| -> anyhow::Result<u64> {
            let value = env::var(name)
                .ok()
                .map(|s| s.parse::<u64>())
                .transpose()?
                .unwrap_or(default);
            anyhow::ensure!(value > 0, "{name} must be positive");
            Ok(value)
        };
        let settings = Self {
            auth_enabled: env::var("AUTH_ENABLED").as_deref() != Ok("false"),
            max_body: positive("MAX_REQUEST_BODY_BYTES", 1048576)? as usize,
            max_page: positive("MAX_PAGE_LIMIT", 1000)?.min(u32::MAX as u64) as u32,
            timeout_ms: positive("ROUTE_TIMEOUT_MS", 30000)?,
            cors: env::var("CORS_ORIGINS")
                .unwrap_or_else(|_| if production() { "" } else { "*" }.into())
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            version: env!("CARGO_PKG_VERSION").into(),
        };
        anyhow::ensure!(
            !production()
                || settings.auth_enabled
                || env::var("ALLOW_INSECURE_AUTH_DISABLED").as_deref() == Ok("true"),
            "AUTH_ENABLED=false is forbidden in production without ALLOW_INSECURE_AUTH_DISABLED=true"
        );
        anyhow::ensure!(
            !production() || (!settings.cors.is_empty() && !settings.cors.iter().any(|s| s == "*")),
            "Production requires explicit CORS_ORIGINS"
        );
        Ok(settings)
    }
}
#[derive(serde::Serialize)]
struct Metrics {
    #[serde(rename = "startedAt")]
    started_at: String,
    requests: u64,
    statuses: BTreeMap<u16, u64>,
    routes: BTreeMap<String, u64>,
    #[serde(rename = "totalDurationMs")]
    total_duration_ms: u128,
}
#[derive(Clone)]
pub(crate) struct AppState {
    pub service: Arc<Mutex<Service>>,
    pub openapi: Arc<String>,
    pub settings: Settings,
    pub registry: Arc<Registry>,
    pub logo: Arc<Option<(Vec<u8>, String)>>,
    metrics: Arc<Mutex<Metrics>>,
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            axum::Json(json!({"ok":false,"error":self.message,"code":self.code})),
        )
            .into_response()
    }
}
pub fn router(service: Service, openapi: String, auth_enabled: bool) -> Router {
    let settings = Settings {
        auth_enabled,
        ..Default::default()
    };
    router_with_settings(
        service,
        openapi,
        settings,
        Registry::ephemeral().expect("Operating-system entropy required"),
    )
}
pub fn router_with_settings(
    service: Service,
    openapi: String,
    settings: Settings,
    registry: Registry,
) -> Router {
    build_router(service, openapi, settings, registry, None)
}
fn build_router(
    service: Service,
    openapi: String,
    settings: Settings,
    registry: Registry,
    logo: Option<(Vec<u8>, String)>,
) -> Router {
    let state = AppState {
        service: Arc::new(Mutex::new(service)),
        openapi: Arc::new(openapi),
        settings,
        registry: Arc::new(registry),
        logo: Arc::new(logo),
        metrics: Arc::new(Mutex::new(Metrics {
            started_at: crate::identity::now(),
            requests: 0,
            statuses: BTreeMap::new(),
            routes: BTreeMap::new(),
            total_duration_ms: 0,
        })),
    };
    Router::new().fallback(any(handle)).with_state(state)
}
pub async fn serve(schema_json: &str, openapi: &str) -> anyhow::Result<()> {
    serve_at(schema_json, openapi, Path::new(".")).await
}
pub async fn serve_at(schema_json: &str, openapi: &str, root: &Path) -> anyhow::Result<()> {
    serve_embedded(schema_json, openapi, root, None, false).await
}
pub async fn serve_embedded(
    schema_json: &str,
    openapi: &str,
    root: &Path,
    logo: Option<(&[u8], &str)>,
    mcp_http: bool,
) -> anyhow::Result<()> {
    let schema: openb2c_schema::Schema = serde_json::from_str(schema_json)?;
    let settings = Settings::from_environment()?;
    let registry = Registry::from_environment()?;
    for (kind, integration) in schema
        .metadata
        .get("integrations")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let commerce = schema
            .metadata
            .get("commerce")
            .is_some_and(|c| c["enabled"] == true);
        let effects = schema
            .operations
            .values()
            .flat_map(|ops| ops.values())
            .flat_map(|op| op.effects.iter())
            .collect::<Vec<_>>();
        let active = match kind.as_str() {
            "identityEmail" => schema.tables.contains_key("identity_challenge"),
            "emailEffects" => {
                commerce
                    || effects
                        .iter()
                        .any(|e| e["notify"]["template"]["channel"] == "email")
            }
            "payment" | "paymentWebhook" => commerce,
            "webhookEffects" => effects.iter().any(|e| e["call"]["service"] == "webhook"),
            _ => true,
        };
        if !active {
            continue;
        }
        for (name, spec) in integration["env"].as_object().into_iter().flatten() {
            if production() && spec["requiredInProduction"] == true && !fake_allowed() {
                anyhow::ensure!(
                    env::var(name).is_ok_and(|s| !s.is_empty()),
                    "{name} is required in production"
                );
            }
        }
    }
    let db = crate::database_path(&schema);
    let service = crate::open_from_environment(schema, root)?;
    let _worker = crate::effects::Worker::start(service.schema.clone(), &db)?;
    eprintln!(
        "{}",
        json!({"level":"info","message":"startup diagnostics","version":settings.version,"database":db,"authEnabled":settings.auth_enabled,"migrationCount":service.query("SELECT id FROM openb2c_migration", &[])?.len()})
    );
    let app = build_router(
        service,
        openapi.into(),
        settings,
        registry,
        logo.map(|(bytes, mime)| (bytes.to_vec(), mime.into())),
    );
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var(if mcp_http { "MCP_PORT" } else { "PORT" })
        .unwrap_or_else(|_| if mcp_http { "3090" } else { "3085" }.into())
        .parse::<u16>()?;
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    eprintln!(
        "{}",
        json!({"level":"info","message":"OpenB2C server listening","address":listener.local_addr()?.to_string()})
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    drop(_worker);
    eprintln!("{}", json!({"level":"info","message":"shutdown complete"}));
    Ok(())
}
async fn handle(State(state): State<AppState>, request: Request<Body>) -> Response {
    let started = Instant::now();
    let (mut parts, body) = request.into_parts();
    parts.headers.remove("x-openb2c-peer-ip");
    if let Some(peer) = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        parts.headers.insert(
            "x-openb2c-peer-ip",
            HeaderValue::from_str(&peer.ip().to_string())
                .expect("IP address is an HTTP header value"),
        );
    }
    let request_id = parts
        .headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 128)
        .map(str::to_string)
        .unwrap_or_else(|| {
            secret("").unwrap_or_else(|_| Utc::now().timestamp_nanos_opt().unwrap_or(0).to_string())
        });
    let origin = parts
        .headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let allowed_origin = origin
        .as_ref()
        .filter(|origin| state.settings.cors.iter().any(|s| s == "*" || s == *origin))
        .cloned();
    let response = if let Some(version) = parts
        .headers
        .get("x-openb2c-api-version")
        .and_then(|v| v.to_str().ok())
        .filter(|v| *v != state.settings.version)
    {
        ServiceError::new(
            "unsupported_version",
            format!("Unsupported API version {version}"),
        )
        .into_response()
    } else if parts.method == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        match to_bytes(body, state.settings.max_body).await {
            Err(_) => {
                ServiceError::new("payload_too_large", "Request exceeds configured body limit")
                    .into_response()
            }
            Ok(body) => {
                let timeout = state.settings.timeout_ms;
                let state = state.clone();
                let method = parts.method.clone();
                let uri = parts.uri.clone();
                let headers = parts.headers.clone();
                let work = tokio::task::spawn_blocking(move || {
                    let _deadline = crate::service::Deadline::enter(Duration::from_millis(timeout));
                    dispatch(
                        &state,
                        &method,
                        uri.path(),
                        uri.query().unwrap_or(""),
                        &headers,
                        &body,
                    )
                    .unwrap_or_else(IntoResponse::into_response)
                });
                match tokio::time::timeout(Duration::from_millis(timeout), work).await {
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) => ServiceError::new("internal_error", "Request handler failed")
                        .into_response(),
                    Err(_) => ServiceError::new("timeout", "Request timed out").into_response(),
                }
            }
        }
    };
    let mut response = response;
    let headers = response.headers_mut();
    for (name, value) in [
        ("x-openb2c-api-version", state.settings.version.as_str()),
        ("x-request-id", request_id.as_str()),
        (
            "x-correlation-id",
            parts
                .headers
                .get("x-correlation-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(&request_id),
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "same-origin"),
    ] {
        if let Ok(v) = HeaderValue::from_str(value) {
            headers.insert(axum::http::header::HeaderName::from_static(name), v);
        }
    }
    headers.insert("vary", HeaderValue::from_static("Origin"));
    if let Some(origin) = allowed_origin.and_then(|s| HeaderValue::from_str(&s).ok()) {
        headers.insert("access-control-allow-origin", origin);
        headers.insert(
            "access-control-allow-credentials",
            HeaderValue::from_static("true"),
        );
        headers.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
        );
        headers.insert("access-control-allow-headers",HeaderValue::from_static("Content-Type, Authorization, If-Match, X-Certificate, X-Timestamp, X-Signature, X-OpenB2C-API-Version, Idempotency-Key, X-Request-ID, X-Correlation-ID, Mcp-Session-Id"));
        headers.insert(
            "access-control-expose-headers",
            HeaderValue::from_static(
                "ETag, X-OpenB2C-API-Version, X-Request-ID, X-Correlation-ID, Mcp-Session-Id",
            ),
        );
    }
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.requests += 1;
        *metrics
            .statuses
            .entry(response.status().as_u16())
            .or_default() += 1;
        *metrics
            .routes
            .entry(format!("{} {}", parts.method, parts.uri.path()))
            .or_default() += 1;
        metrics.total_duration_ms += started.elapsed().as_millis();
    }
    eprintln!(
        "{}",
        json!({"level":"info","message":"request","requestId":request_id,"correlationId":parts.headers.get("x-correlation-id").and_then(|v|v.to_str().ok()).unwrap_or(&request_id),"method":parts.method.as_str(),"path":parts.uri.path(),"status":response.status().as_u16(),"ms":started.elapsed().as_millis()})
    );
    response
}

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get("cookie")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .filter_map(|p| p.trim().split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.into())
        })
}
pub(crate) fn authenticate(
    state: &AppState,
    service: &Service,
    headers: &HeaderMap,
    method: &str,
    path: &str,
) -> Result<AuthContext> {
    if !state.settings.auth_enabled {
        return Ok(AuthContext::system());
    }
    if let Some(certificate) = headers.get("x-certificate") {
        use base64::Engine;
        let text = certificate
            .to_str()
            .map_err(|_| ServiceError::new("malformed", "Invalid certificate header"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text)
            .unwrap_or_else(|_| text.as_bytes().to_vec());
        let cert: Certificate = serde_json::from_slice(&bytes)
            .map_err(|_| ServiceError::new("malformed", "Invalid certificate"))?;
        return service.verify_request(
            &state.registry,
            &cert,
            method,
            path,
            header(headers, "x-timestamp"),
            header(headers, "x-signature"),
        );
    }
    if let Some(authorization) = headers.get("authorization") {
        let token = authorization
            .to_str()
            .ok()
            .and_then(|s| s.strip_prefix("Bearer "))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ServiceError::new("unauthorized", "Expected bearer credential"))?;
        return service.authenticate(token);
    }
    if let Some(token) = cookie(headers, "openb2c_session") {
        return Ok(service.authenticate(&token).unwrap_or_default());
    }
    Ok(AuthContext::default())
}
pub(crate) fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
pub(crate) fn client_ip(headers: &HeaderMap) -> &str {
    ["x-forwarded-for", "x-real-ip", "x-openb2c-peer-ip"]
        .into_iter()
        .map(|name| header(headers, name).split(',').next().unwrap_or("").trim())
        .find(|value| value.parse::<std::net::IpAddr>().is_ok())
        .unwrap_or("unknown")
}
pub(crate) fn json_body(headers: &HeaderMap, bytes: &[u8]) -> Result<Value> {
    if header(headers, "content-type")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        != "application/json"
    {
        return Err(ServiceError::new(
            "unsupported_media_type",
            "Content-Type must be application/json",
        ));
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| ServiceError::new("malformed", "Malformed JSON"))?;
    if !value.is_object() {
        return Err(ServiceError::new("invalid", "Expected a JSON object"));
    }
    Ok(value)
}
fn ok(value: Value) -> Response {
    axum::Json(value).into_response()
}
fn tagged(table: &str, row: Value) -> Response {
    let tag = etag(table, &row);
    let mut response = ok(row);
    if let Ok(tag) = HeaderValue::from_str(&tag) {
        response.headers_mut().insert("etag", tag);
    }
    response
}
fn protected(auth: &AuthContext, scope: &str) -> Result<()> {
    if auth.has_scope(scope) {
        Ok(())
    } else {
        Err(ServiceError::new(
            if auth.user_id.is_none() {
                "unauthorized"
            } else {
                "forbidden"
            },
            "Permission required",
        ))
    }
}
fn dispatch(
    state: &AppState,
    method: &Method,
    path: &str,
    query: &str,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Result<Response> {
    let _source = crate::service::SourceScope::enter(if path == "/mcp" {
        "mcp"
    } else if path.starts_with("/api/") || path.starts_with("/commerce/") {
        "rest"
    } else {
        "ui"
    });
    let service = state
        .service
        .lock()
        .map_err(|_| ServiceError::new("internal_error", "Service unavailable"))?;
    let auth = authenticate(state, &service, headers, method.as_str(), path)?;
    if path == "/health" {
        return Ok(axum::Json(json!({"status":"ok","app":crate::ui::slug(&service.schema.organization.name),"version":state.settings.version,"auth":state.settings.auth_enabled})).into_response());
    }
    if path == "/openapi.json" {
        return Ok((
            [("content-type", "application/json")],
            state.openapi.as_str().to_string(),
        )
            .into_response());
    }
    if path == "/identity/public-key" && method == Method::GET {
        return Ok(axum::Json(json!({"publicKey":state.registry.public_key})).into_response());
    }
    if path == "/identity/challenge" && method == Method::POST {
        let input = json_body(headers, bytes)?;
        let public = input["publicKey"]
            .as_str()
            .filter(|s| hex::decode(s).is_ok_and(|b| b.len() == 32))
            .ok_or_else(|| {
                ServiceError::new("invalid", "publicKey must be an Ed25519 public key")
            })?;
        let email = input["email"]
            .as_str()
            .ok_or_else(|| ServiceError::new("invalid", "email is required"))?
            .trim()
            .to_lowercase();
        let (id, code) = service.challenge(&email, public, client_ip(headers))?;
        service.deliver_identity_email(&email, &code, id)?;
        let mut value = json!({"challengeId":id,"message":"verification code sent to email"});
        if !production() && env::var("EMAIL_PROVIDER").as_deref() != Ok("resend") {
            value["code"] = json!(code);
        }
        return Ok(axum::Json(value).into_response());
    }
    if path == "/identity/verify" && method == Method::POST {
        let input = json_body(headers, bytes)?;
        let value = service.verify_challenge(
            &state.registry,
            input["challengeId"]
                .as_i64()
                .ok_or_else(|| ServiceError::new("invalid", "challengeId is required"))?,
            input["code"].as_str().unwrap_or(""),
            input["signature"].as_str().unwrap_or(""),
            None,
        )?;
        return Ok(axum::Json(value).into_response());
    }
    if path == "/auth/context" {
        if auth.user_id.is_none() && !auth.has_scope("*") {
            return Err(ServiceError::new("unauthorized", "Authentication required"));
        }
        return Ok(axum::Json(json!(auth)).into_response());
    }
    if path == "/auth/revoke-current" && method == Method::POST {
        if let Some(token) = header(headers, "authorization")
            .strip_prefix("Bearer ")
            .map(str::to_string)
            .or_else(|| cookie(headers, "openb2c_session"))
        {
            service.revoke_session(&token)?;
        } else if let Some(cert) = headers.get("x-certificate").and_then(|s| s.to_str().ok()) {
            use base64::Engine;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(cert)
                .unwrap_or_else(|_| cert.as_bytes().to_vec());
            let cert: Certificate = serde_json::from_slice(&raw)
                .map_err(|_| ServiceError::new("invalid", "Invalid certificate"))?;
            service.db.execute(
                "UPDATE identity_registry SET revoked=1 WHERE email=? AND public_key=?",
                [&cert.email, &cert.public_key],
            )?;
        } else {
            return Err(ServiceError::new(
                "unauthorized",
                "Session or certificate required",
            ));
        }
        return Ok(ok(json!({"revoked":true})));
    }
    if path == "/mcp" && method == Method::POST {
        use rusqlite::OptionalExtension;
        let input = json_body(headers, bytes)?;
        service.db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_mcp_session(id TEXT PRIMARY KEY,expires_at TEXT NOT NULL);")?;
        let initialize = input["method"] == "initialize";
        let session = if initialize {
            secret("mcp_")?
        } else {
            let id = header(headers, "mcp-session-id");
            let valid:Option<String>=service.db.query_row("SELECT id FROM openb2c_mcp_session WHERE id=? AND julianday(expires_at)>julianday('now')",[id],|r|r.get(0)).optional()?;
            if valid.is_none() {
                return Ok((StatusCode::BAD_REQUEST,axum::Json(json!({"jsonrpc":"2.0","id":input.get("id"),"error":{"code":-32600,"message":"Invalid or missing session ID"}}))).into_response());
            }
            require_mcp_auth(&auth)?;
            id.to_string()
        };
        if initialize {
            service.db.execute(
                "DELETE FROM openb2c_mcp_session WHERE julianday(expires_at)<=julianday('now')",
                [],
            )?;
            service.db.execute(
                "INSERT INTO openb2c_mcp_session(id,expires_at) VALUES (?,?)",
                [&session, &crate::identity::after(86400)],
            )?;
        }
        if input.get("id").is_none() {
            return Ok(StatusCode::ACCEPTED.into_response());
        }
        let mut response = axum::Json(crate::mcp::respond(&service, &input, &auth)).into_response();
        response.headers_mut().insert(
            "mcp-session-id",
            HeaderValue::from_str(&session)
                .map_err(|_| ServiceError::new("internal_error", "Invalid session ID"))?,
        );
        return Ok(response);
    }
    if path.starts_with("/ops/") {
        protected(&auth, "effect.admin")?;
        let value = match (method.as_str(), path) {
            ("GET", "/ops/effects") => json!({"items":service.effects(None)?}),
            ("POST", "/ops/effects/retry") => {
                if !bytes.is_empty() {
                    let input = json_body(headers, bytes)?;
                    if let Some(id) = input["id"].as_i64() {
                        service.retry_effect(id)?;
                    }
                }
                service.dispatch_effects(true)?
            }
            ("GET", "/ops/fake-emails") => {
                if !fake_allowed() {
                    return Err(ServiceError::new("forbidden", "Fake providers disabled"));
                }
                json!({"items":service.fake_emails()?})
            }
            ("GET", "/ops/metrics") => serde_json::to_value(
                &*state
                    .metrics
                    .lock()
                    .map_err(|_| ServiceError::new("internal_error", "Metrics unavailable"))?,
            )
            .map_err(|_| ServiceError::new("internal_error", "Metrics unavailable"))?,
            ("GET", "/ops/migrations") => {
                json!({"items":service.query("SELECT * FROM openb2c_migration ORDER BY applied_at,id",&[])?})
            }
            _ => return Err(ServiceError::new("not_found", "Unknown operation endpoint")),
        };
        return Ok(axum::Json(value).into_response());
    }
    if path.starts_with("/commerce/") {
        let segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
        let value = match (method.as_str(), segments.as_slice()) {
            ("GET", ["commerce", "catalog"]) => service.catalog()?,
            ("POST", ["commerce", "checkout"]) => {
                service.checkout(&json_body(headers, bytes)?, &auth)?
            }
            ("POST", ["commerce", "bookings", "reserve"]) => {
                service.reserve_booking(&json_body(headers, bytes)?, &auth)?
            }
            ("POST", ["commerce", "orders" | "bookings", id, "payment-session"]) => {
                service.payment_session(&parse_id(id)?, &auth)?
            }
            ("POST", ["commerce", "orders" | "bookings", "expire"]) => {
                protected(&auth, "commerce.expire")?;
                service.expire_orders()?
            }
            ("POST", ["commerce", "payments", "webhook"]) => {
                let secret = env::var("PAYMENT_WEBHOOK_SECRET").unwrap_or_default();
                if secret.is_empty()
                    || !crate::effects::verify_hmac(
                        &secret,
                        bytes,
                        header(headers, "x-payment-signature"),
                    )
                {
                    return Err(ServiceError::new(
                        "unauthorized",
                        "Invalid payment webhook signature",
                    ));
                }
                service.payment_webhook(&json_body(headers, bytes)?)?
            }
            ("GET", ["commerce", "payments", "local-simulator"]) => {
                return crate::ui::page(
                    state, &service, method, path, query, headers, bytes, &auth,
                );
            }
            _ => return Err(ServiceError::new("not_found", "Unknown commerce endpoint")),
        };
        return Ok(ok(value));
    }
    if path.starts_with("/api/") {
        let parts = path
            .trim_start_matches("/api/")
            .split('/')
            .collect::<Vec<_>>();
        let table = service
            .schema
            .tables
            .keys()
            .find(|t| format!("{t}s") == parts[0])
            .ok_or_else(|| ServiceError::new("not_found", "Unknown entity"))?;
        let response = match (method.as_str(), parts.as_slice()) {
            ("GET", [_]) => {
                let query: BTreeMap<String, String> = serde_urlencoded::from_str(query)
                    .map_err(|_| ServiceError::new("malformed", "Invalid query parameters"))?;
                let mut options = ListOptions::default();
                for (key, value) in query {
                    match key.as_str() {
                        "limit" => {
                            options.limit = value
                                .parse::<u32>()
                                .ok()
                                .filter(|v| *v > 0)
                                .ok_or_else(|| {
                                    ServiceError::new("invalid", "limit must be positive")
                                })?
                                .min(state.settings.max_page)
                        }
                        "offset" => {
                            options.offset = value.parse().map_err(|_| {
                                ServiceError::new("invalid", "offset must be nonnegative")
                            })?
                        }
                        "sort" => options.sort = Some(value),
                        "order" if ["asc", "desc"].contains(&value.as_str()) => {
                            options.descending = value == "desc"
                        }
                        _ if key.starts_with("filter[") && key.ends_with(']') => {
                            let field = &key[7..key.len() - 1];
                            let col = service.columns(table)?.get(field).ok_or_else(|| {
                                ServiceError::new("invalid", "Unknown filter field")
                            })?;
                            let value = match col.kind {
                                openb2c_schema::SqlType::Integer => {
                                    json!(value.parse::<i64>().map_err(|_| {
                                        ServiceError::new("invalid", "Integer filter expected")
                                    })?)
                                }
                                openb2c_schema::SqlType::Real => {
                                    json!(value.parse::<f64>().map_err(|_| {
                                        ServiceError::new("invalid", "Numeric filter expected")
                                    })?)
                                }
                                _ => json!(value),
                            };
                            options.filter.insert(field.into(), value);
                        }
                        _ => {
                            return Err(ServiceError::new(
                                "invalid",
                                format!("Unknown query parameter {key}"),
                            ));
                        }
                    }
                }
                axum::Json(service.list(table, &options, &auth)?).into_response()
            }
            ("POST", [_]) => {
                let row = service.create(table, &json_body(headers, bytes)?, &auth)?;
                let mut r = tagged(table, row);
                *r.status_mut() = StatusCode::CREATED;
                r
            }
            ("GET", [_, id]) => tagged(
                table,
                service.find(table, &parse_entity_id(&service, table, id)?, &auth)?,
            ),
            ("PUT" | "PATCH" | "DELETE", [_, id]) => {
                let input = if *method == Method::DELETE {
                    json!({})
                } else {
                    json_body(headers, bytes)?
                };
                tagged(
                    table,
                    service.mutate(
                        table,
                        &parse_entity_id(&service, table, id)?,
                        if *method == Method::DELETE {
                            "delete"
                        } else {
                            "update"
                        },
                        &input,
                        &auth,
                        headers.get("if-match").and_then(|v| v.to_str().ok()),
                    )?,
                )
            }
            ("POST", [_, id, op]) => tagged(
                table,
                service.mutate(
                    table,
                    &parse_entity_id(&service, table, id)?,
                    &op.replace('-', "_"),
                    &if bytes.is_empty() {
                        json!({})
                    } else {
                        json_body(headers, bytes)?
                    },
                    &auth,
                    headers.get("if-match").and_then(|v| v.to_str().ok()),
                )?,
            ),
            _ => return Err(ServiceError::new("not_found", "Unknown API route")),
        };
        return Ok(response);
    }
    crate::ui::page(state, &service, method, path, query, headers, bytes, &auth)
}
pub(crate) fn parse_id(s: &str) -> Result<Value> {
    s.parse::<i64>()
        .map(|v| json!(v))
        .map_err(|_| ServiceError::new("invalid", "ID must be an integer"))
}
fn parse_entity_id(service: &Service, table: &str, id: &str) -> Result<Value> {
    if service.columns(table)?[service.primary_key(table)?].kind == openb2c_schema::SqlType::Integer
    {
        parse_id(id)
    } else {
        Ok(json!(id))
    }
}
pub fn escape(s: &str) -> String {
    crate::ui::escape(s)
}
fn require_mcp_auth(auth: &AuthContext) -> Result<()> {
    if auth.user_id.is_some() || auth.has_scope("*") {
        Ok(())
    } else {
        Err(ServiceError::new("unauthorized", "Authentication required"))
    }
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
        } else {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    eprintln!(
        "{}",
        json!({"ts":crate::identity::now(),"level":"info","message":"shutdown requested"})
    );
}
