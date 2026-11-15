use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use openb2c_runtime::{AuthContext, Service};
use openb2c_schema::Schema;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock},
};
use tower::ServiceExt;
const PASSWORD: &str = "violet-river-sunrise-47";
fn schema(selection: &str) -> Schema {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Schema>>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(selection.into())
        .or_insert_with(|| {
            let result = Command::new("nix")
                .args([
                    "eval",
                    "--json",
                    "-f",
                    "crates/openb2c-runtime/tests/fixtures/capabilities.nix",
                    "--apply",
                    &format!(
                        "f: f {{ selection = {}; }}",
                        serde_json::to_string(selection).unwrap()
                    ),
                ])
                .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            serde_json::from_slice(&result.stdout).unwrap()
        })
        .clone()
}
fn service() -> Service {
    Service::open(schema("passwords"), ":memory:", false).unwrap()
}
fn signup(service: &Service, email: &str) -> Value {
    service
        .password_signup(&json!({"email":email,"password":PASSWORD}), "127.0.0.1")
        .unwrap()
}
fn token(value: &Value) -> &str {
    value["access_token"].as_str().unwrap()
}

#[test]
fn signup_stores_argon2_and_hashed_sessions_without_claiming_existing_customers() {
    let service = service();
    service.ensure_user("operator@example.test").unwrap();
    assert_eq!(
        service
            .password_signup(
                &json!({"email":"OPERATOR@example.test","password":PASSWORD}),
                "peer"
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    let value = signup(&service, " Alice@Example.Test ");
    assert_eq!(value["user"]["email"], "alice@example.test");
    assert_eq!(value["expires_in"], 3600);
    let hash: String = service
        .db
        .query_row("SELECT password_hash FROM password_credential", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains(PASSWORD));
    let stored: String = service
        .db
        .query_row("SELECT token_hash FROM password_session", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, token(&value));
    assert_eq!(stored.len(), 64);
    assert_eq!(
        service
            .password_signup(
                &json!({"email":"alice@example.test","password":PASSWORD}),
                "peer"
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    for input in [
        json!({"email":"bad@example.test","password":"short"}),
        json!({"email":"bad@example.test","password":PASSWORD,"scopes":["*"]}),
        json!({"email":"bad@example.test","password":PASSWORD,"user_id":1}),
    ] {
        assert_eq!(
            service.password_signup(&input, "peer").unwrap_err().code,
            "invalid"
        );
    }
    for table in [
        "password_credential",
        "password_session",
        "password_attempt",
    ] {
        assert!(
            service
                .list(table, &Default::default(), &AuthContext::system())
                .is_err()
        );
        assert!(
            service
                .create(table, &json!({}), &AuthContext::system())
                .is_err()
        );
    }
}

#[test]
fn signup_is_atomic_and_password_contracts_protect_internal_state() {
    let service = service();
    service.db.execute_batch("CREATE TRIGGER reject_session BEFORE INSERT ON password_session BEGIN SELECT RAISE(ABORT,'session rejected'); END;").unwrap();
    assert!(
        service
            .password_signup(
                &json!({"email":"atomic@example.test","password":PASSWORD}),
                "peer"
            )
            .is_err()
    );
    for table in ["user", "password_credential", "password_session"] {
        assert_eq!(
            service
                .db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    service
        .db
        .execute_batch("DROP TRIGGER reject_session;")
        .unwrap();
    assert!(
        service
            .password_signup(
                &json!({"email":"atomic@example.test","password":PASSWORD}),
                "peer"
            )
            .is_ok()
    );
    let mut invalid = service.schema.clone();
    invalid
        .operations
        .get_mut("password_credential")
        .unwrap()
        .get_mut("create")
        .unwrap()
        .enabled = true;
    assert!(
        openb2c_schema::validate::validate(&invalid)
            .iter()
            .any(|error| error.contains("password_credential.create"))
    );
}

#[test]
fn sessions_expire_revoke_and_password_changes_invalidate_all_previous_sessions() {
    let service = service();
    let first = signup(&service, "alice@example.test");
    let login = service
        .password_login(
            &json!({"email":"ALICE@example.test","password":PASSWORD}),
            "peer",
        )
        .unwrap();
    let wrong = service
        .password_login(
            &json!({"email":"alice@example.test","password":"wrong-password-value"}),
            "peer",
        )
        .unwrap_err();
    let unknown = service
        .password_login(
            &json!({"email":"unknown@example.test","password":"wrong-password-value"}),
            "peer",
        )
        .unwrap_err();
    assert_eq!((wrong.code, wrong.message), (unknown.code, unknown.message));
    let changed = service
        .password_change(
            token(&first),
            &json!({"current_password":PASSWORD,"password":"new-forest-horizon-91"}),
            "peer",
        )
        .unwrap();
    assert!(service.authenticate(token(&first)).is_err());
    assert!(service.authenticate(token(&login)).is_err());
    assert!(
        service
            .password_login(
                &json!({"email":"alice@example.test","password":PASSWORD}),
                "peer"
            )
            .is_err()
    );
    assert!(
        service
            .password_login(
                &json!({"email":"alice@example.test","password":"new-forest-horizon-91"}),
                "peer"
            )
            .is_ok()
    );
    service.password_logout(token(&changed)).unwrap();
    assert!(service.authenticate(token(&changed)).is_err());
    service
        .db
        .execute(
            "UPDATE password_session SET expires_at='2000-01-01T00:00:00Z'",
            [],
        )
        .unwrap();
    let expired = service
        .password_login(
            &json!({"email":"alice@example.test","password":"new-forest-horizon-91"}),
            "another-peer",
        )
        .unwrap();
    service
        .db
        .execute(
            "UPDATE password_session SET expires_at='2000-01-01T00:00:00Z'",
            [],
        )
        .unwrap();
    assert!(service.authenticate(token(&expired)).is_err());
}

#[test]
fn long_unicode_passwords_are_not_truncated_and_failed_logins_are_rate_limited() {
    let service = service();
    let password = format!("{}A", "界".repeat(40));
    let other = format!("{}B", "界".repeat(40));
    let signup = service
        .password_signup(
            &json!({"email":"unicode@example.test","password":password}),
            "peer",
        )
        .unwrap();
    assert!(service.authenticate(token(&signup)).is_ok());
    assert!(
        service
            .password_login(
                &json!({"email":"unicode@example.test","password":other}),
                "peer"
            )
            .is_err()
    );
    assert!(
        service
            .password_login(
                &json!({"email":"unicode@example.test","password":password}),
                "peer"
            )
            .is_ok()
    );
    for _ in 0..7 {
        assert_eq!(
            service
                .password_login(
                    &json!({"email":"unicode@example.test","password":"wrong"}),
                    "rotating-peer"
                )
                .unwrap_err()
                .code,
            "unauthorized"
        );
    }
    assert_eq!(
        service
            .password_login(
                &json!({"email":"unicode@example.test","password":password}),
                "fresh-peer"
            )
            .unwrap_err()
            .code,
        "rate_limited"
    );
    service
        .db
        .execute(
            "UPDATE password_attempt SET created_at='2000-01-01T00:00:00Z'",
            [],
        )
        .unwrap();
    assert!(
        service
            .password_login(
                &json!({"email":"unicode@example.test","password":password}),
                "peer"
            )
            .is_ok()
    );
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    value: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    if response.status().is_success() && path.starts_with("/auth/") && path != "/auth/context" {
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    assert!(!response.headers().contains_key("set-cookie"));
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn browserless_signup_login_checkout_isolation_and_logout_work_over_http() {
    let service = Service::open(schema("password_shop"), ":memory:", false).unwrap();
    let doc = openb2c_schema::contract::openapi(&service.schema);
    assert!(doc["paths"]["/auth/signup"]["post"]["responses"]["201"].is_object());
    assert!(doc["paths"].get("/api/password_credentials").is_none());
    let app = openb2c_runtime::router(service, doc.to_string(), true);
    let (status, alice) = request(
        &app,
        "POST",
        "/auth/signup",
        None,
        json!({"email":"alice@example.test","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, 201);
    let (status, bob) = request(
        &app,
        "POST",
        "/auth/signup",
        None,
        json!({"email":"bob@example.test","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, 201);
    let (status, order) = request(
        &app,
        "POST",
        "/b2c/orders",
        Some(token(&alice)),
        json!({"idempotency_key":"order","items":[{"price_id":1,"quantity":1}]}),
    )
    .await;
    assert_eq!(status, 200);
    let id = order["order"]["id"].as_i64().unwrap();
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/b2c/orders/{id}"),
            Some(token(&bob)),
            json!({})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/b2c/payments/1/simulate",
            Some(token(&alice)),
            json!({"event_id":"paid","outcome":"succeeded"})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(&app, "GET", "/ops/metrics", Some(token(&alice)), json!({}))
            .await
            .0,
        403
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/password_credentials",
            Some(token(&alice)),
            json!({})
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/auth/signup",
            None,
            json!({"email":"mallory@example.test","password":PASSWORD,"scopes":"*"})
        )
        .await
        .0,
        422
    );
    assert_eq!(
        request(&app, "POST", "/auth/logout", Some(token(&alice)), json!({}))
            .await
            .0,
        200
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/b2c/orders/{id}"),
            Some(token(&alice)),
            json!({})
        )
        .await
        .0,
        401
    );
    let (status, login) = request(
        &app,
        "POST",
        "/auth/login",
        None,
        json!({"email":"alice@example.test","password":PASSWORD}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/b2c/orders/{id}"),
            Some(token(&login)),
            json!({})
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn peer_rate_limits_use_the_socket_address_and_passwords_remain_optional() {
    use sha2::{Digest, Sha256};
    let service = service();
    let peer = hex::encode(Sha256::digest(b"192.0.2.10"));
    for _ in 0..100 {
        service.db.execute("INSERT INTO password_attempt (account_key,peer_key) VALUES ('different-account',?)", [&peer]).unwrap();
    }
    let app = openb2c_runtime::router(service, "{}".into(), true);
    let mut request = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "203.0.113.1")
        .header("x-real-ip", "203.0.113.2")
        .header("x-openb2c-peer-ip", "203.0.113.3")
        .body(Body::from(
            json!({"email":"unknown@example.test","password":PASSWORD}).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "192.0.2.10:4242".parse::<std::net::SocketAddr>().unwrap(),
    ));
    assert_eq!(app.oneshot(request).await.unwrap().status(), 429);
    let service = Service::open(schema("customers"), ":memory:", false).unwrap();
    let doc = openb2c_schema::contract::openapi(&service.schema);
    assert!(doc["paths"].get("/auth/signup").is_none());
    assert!(!service.schema.tables.contains_key("password_credential"));
    assert!(
        service
            .password_signup(
                &json!({"email":"x@example.test","password":PASSWORD}),
                "peer"
            )
            .is_err()
    );
}
