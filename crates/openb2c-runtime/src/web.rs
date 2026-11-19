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
        #[cfg(not(feature = "reference"))]
        anyhow::ensure!(
            settings.auth_enabled,
            "Authentication is required by this compiled service"
        );
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
struct AppState {
    pub service: Arc<Mutex<Service>>,
    pub openapi: Arc<String>,
    pub settings: Settings,
    pub registry: Arc<Registry>,
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
    build_router(service, openapi, settings, registry)
}
fn build_router(
    service: Service,
    openapi: String,
    settings: Settings,
    registry: Registry,
) -> Router {
    let state = AppState {
        service: Arc::new(Mutex::new(service)),
        openapi: Arc::new(openapi),
        settings,
        registry: Arc::new(registry),
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
    let schema: openb2c_schema::Schema = serde_json::from_str(schema_json)?;
    let settings = Settings::from_environment()?;
    let registry = if schema.tables.contains_key("identity_registry") {
        Registry::from_environment()?
    } else {
        Registry::ephemeral()?
    };
    for (kind, integration) in schema
        .metadata
        .get("integrations")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if !schema.integration_active(kind) {
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
    let app = build_router(service, openapi.into(), settings, registry);
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var("PORT")
        .unwrap_or_else(|_| "3085".into())
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
    if parts.uri.path().starts_with("/b2c/downloads") {
        headers.insert(
            "cache-control",
            HeaderValue::from_static("private, no-store"),
        );
    }
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

fn authenticate(
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
        if !service.model_has_table("identity_registry") {
            return Err(ServiceError::new(
                "unauthorized",
                "Certificate identity is not enabled",
            ));
        }
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
    Ok(AuthContext::default())
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
fn client_ip(headers: &HeaderMap) -> &str {
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
    let _source = crate::service::SourceScope::enter("rest");
    let service = state
        .service
        .lock()
        .map_err(|_| ServiceError::new("internal_error", "Service unavailable"))?;
    service.assert_current_model()?;
    let auth = authenticate(state, &service, headers, method.as_str(), path)?;
    if matches!(
        path,
        "/auth/signup" | "/auth/login" | "/auth/logout" | "/auth/password"
    ) {
        if !service.cap_enabled("password_auth") || *method != Method::POST {
            return Err(ServiceError::new(
                "not_found",
                "Unknown authentication endpoint",
            ));
        }
        let peer = headers
            .get("x-openb2c-peer-ip")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown");
        let token = header(headers, "authorization")
            .strip_prefix("Bearer ")
            .unwrap_or("");
        let value = match path {
            "/auth/signup" => service.password_signup(&json_body(headers, bytes)?, peer)?,
            "/auth/login" => service.password_login(&json_body(headers, bytes)?, peer)?,
            "/auth/password" => {
                service.password_change(token, &json_body(headers, bytes)?, peer)?
            }
            _ => {
                if !bytes.is_empty() && json_body(headers, bytes)? != json!({}) {
                    return Err(ServiceError::new(
                        "invalid",
                        "Logout accepts an empty object",
                    ));
                }
                service.password_logout(token)?
            }
        };
        let mut response = axum::Json(value).into_response();
        response
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        if path == "/auth/signup" {
            *response.status_mut() = StatusCode::CREATED;
        }
        return Ok(response);
    }
    if path.starts_with("/identity/") && !service.model_has_table("identity_challenge") {
        return Err(ServiceError::new(
            "not_found",
            "Identity capability is not enabled",
        ));
    }
    if path == "/health" {
        return Ok(axum::Json(json!({"status":"ok","app":crate::text::slug(&service.schema.organization.name),"version":state.settings.version,"auth":state.settings.auth_enabled})).into_response());
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
    #[cfg(not(feature = "reference"))]
    if path.starts_with("/backoffice/") || path.starts_with("/b2c/") {
        return crate::composition::rest(&service, method, path, headers, bytes, &auth);
    }
    #[cfg(feature = "reference")]
    if path.starts_with("/backoffice/") {
        return Ok(ok(crate::backoffice::rest::dispatch(
            &service, method, path, headers, bytes, &auth,
        )?));
    }
    #[cfg(feature = "reference")]
    if path.starts_with("/b2c/") {
        let segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
        if let ["b2c", "downloads", id, "content"] = segments.as_slice()
            && *method == Method::GET
        {
            let id =
                id.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| {
                    ServiceError::new("invalid", "A positive record ID is required")
                })?;
            let file = service.b2c_download(id, &auth)?;
            let mut response = file.bytes.into_response();
            for (name, value) in [
                ("content-type", file.media_type),
                (
                    "content-disposition",
                    format!("attachment; filename=\"{}\"", file.name),
                ),
                ("etag", format!("\"{}\"", file.sha256)),
                ("cache-control", "private, no-store".into()),
            ] {
                response.headers_mut().insert(
                    axum::http::header::HeaderName::from_static(name),
                    HeaderValue::from_str(&value).map_err(|_| {
                        ServiceError::new("internal_error", "Invalid stored file metadata")
                    })?,
                );
            }
            return Ok(response);
        }
        return Ok(ok(crate::b2c::rest::dispatch(
            &service, method, path, headers, bytes, &auth,
        )?));
    }
    if path.starts_with("/api/") {
        let parts = path
            .trim_start_matches("/api/")
            .split('/')
            .collect::<Vec<_>>();
        let table = service
            .api_table(parts[0])
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
                            let value = match service.column_kind(table, field)? {
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
    Err(ServiceError::new("not_found", "Unknown endpoint"))
}
fn parse_id(s: &str) -> Result<Value> {
    s.parse::<i64>()
        .map(|v| json!(v))
        .map_err(|_| ServiceError::new("invalid", "ID must be an integer"))
}
fn parse_entity_id(service: &Service, table: &str, id: &str) -> Result<Value> {
    if service.column_kind(table, service.primary_key(table)?)? == openb2c_schema::SqlType::Integer
    {
        parse_id(id)
    } else {
        Ok(json!(id))
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
