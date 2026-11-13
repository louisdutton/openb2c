mod common;
use axum::{body::Body, http::Request};
use common::service;
use http_body_util::BodyExt;
use openb2c_runtime::{
    AuthContext,
    identity::Registry,
    web::{Settings, router_with_settings},
};
use serde_json::{Value, json};
use tower::ServiceExt;

#[test]
fn openapi_and_mcp_describe_same_schema_metadata_and_operations() {
    let model = common::schema();
    let document = openb2c_schema::contract::openapi(&model);
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(
        document["components"]["schemas"]["Ticket"]["properties"]["performance_id"]["x-openb2c-relationship"]
            ["targetEntity"],
        "performance"
    );
    assert!(
        document["components"]["schemas"]["ApiKey"]["properties"]
            .get("key_hash")
            .is_none()
    );
    assert!(
        document["paths"]
            .get("/api/performances/{id}/cancel")
            .is_some()
    );
    assert_eq!(document["x-openb2c-commerce"]["order"]["entity"], "booking");
    let tools = openb2c_runtime::mcp::tools(&model);
    let cancel = tools
        .iter()
        .find(|t| t["name"] == "cancel_performance")
        .unwrap();
    assert_eq!(cancel["annotations"]["destructiveHint"], true);
    assert_eq!(cancel["_meta"]["openb2c/confirmation"]["required"], true);
    assert!(
        tools
            .iter()
            .find(|t| t["name"] == "create_api_key")
            .unwrap()["inputSchema"]["properties"]
            .get("key_hash")
            .is_none()
    );
}
#[test]
fn mcp_dispatch_uses_authorization_and_the_same_commerce_services() {
    let service = service();
    let initialized = openb2c_runtime::mcp::respond(
        &service,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        &AuthContext::default(),
    );
    assert_eq!(initialized["result"]["serverInfo"]["name"], "duchy-opera");
    let auth = AuthContext {
        user_id: Some(1),
        scopes: service.self_scopes(),
    };
    let response = openb2c_runtime::mcp::respond(
        &service,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"checkout_cart","arguments":{"items":[{"item_id":1,"quantity":2}]}}}),
        &auth,
    );
    assert_eq!(
        response["result"]["structuredContent"]["amount_pence"],
        5000
    );
    let denied = openb2c_runtime::mcp::respond(
        &service,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_tickets","arguments":{}}}),
        &AuthContext::default(),
    );
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        openb2c_runtime::mcp::respond(&service, &json!({"id":4,"method":"unknown"}), &auth)["error"]
            ["code"],
        -32601
    );
}
#[test]
fn effect_dispatch_is_persistent_idempotent_and_captures_fake_deliveries() {
    let service = service();
    service
        .queue_effect(
            "notification-once",
            "notify",
            &json!({"channel":"email","template":"ticket_confirmation","to":"customer"}),
            &json!({}),
        )
        .unwrap();
    service
        .queue_effect("notification-once", "notify", &json!({}), &json!({}))
        .unwrap();
    assert_eq!(service.dispatch_effects(false).unwrap()["succeeded"], 1);
    assert_eq!(service.dispatch_effects(false).unwrap()["attempted"], 0);
    assert_eq!(service.fake_emails().unwrap().len(), 1);
    let attempts = service.effects(None).unwrap();
    assert_eq!(attempts[0]["status"], "succeeded");
    assert_eq!(attempts[0]["attempts"], 1);
}
#[test]
fn webhook_hmac_rejects_tampering_wrong_keys_and_old_timestamps() {
    use openb2c_runtime::effects::*;
    let ts = chrono::Utc::now().timestamp().to_string();
    let body = b"{\"status\":\"paid\"}";
    let signature = hmac(
        "secret",
        [format!("{ts}.").as_bytes(), body].concat().as_slice(),
    );
    assert!(verify_webhook("secret", body, &ts, &signature, 300));
    assert!(!verify_webhook("wrong", body, &ts, &signature, 300));
    assert!(!verify_webhook("secret", b"changed", &ts, &signature, 300));
    assert!(!verify_webhook("secret", body, "0", &signature, 300));
    assert!(!verify_webhook("secret", body, &ts, "not-hex", 300));
}
#[tokio::test]
async fn rest_enforces_body_types_limits_versions_cors_and_tracing() {
    let app = router_with_settings(
        service(),
        "{}".into(),
        Settings {
            auth_enabled: false,
            max_body: 128,
            cors: vec!["https://allowed.test".into()],
            ..Default::default()
        },
        Registry::ephemeral().unwrap(),
    );
    let response = app
        .clone()
        .oneshot(
            Request::get("/health")
                .header("origin", "https://allowed.test")
                .header("x-request-id", "check-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-request-id"], "check-123");
    assert_eq!(response.headers()["x-openb2c-api-version"], "0.1.0");
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://allowed.test"
    );
    let response = app
        .clone()
        .oneshot(
            Request::get("/health")
                .header("x-openb2c-api-version", "future")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    for (content, body, status) in [
        ("text/plain", "{}".to_string(), 415),
        ("application/json", "[1]".into(), 422),
        ("application/json", "{".into(), 400),
        ("application/json", "x".repeat(129), 413),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/venues")
                    .header("content-type", content)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let response = app
        .oneshot(
            Request::get("/api/venues?sort=id%3BDROP%20TABLE%20user")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
}
#[tokio::test]
async fn html_forms_require_csrf_and_admin_authentication() {
    let app = openb2c_runtime::router(service(), "{}".into(), true);
    let response = app
        .clone()
        .oneshot(Request::get("/admin").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    assert!(
        response.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with("/login")
    );
    let response = app
        .oneshot(
            Request::post("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=attacker%40example.test&_csrf=forged"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}
#[tokio::test]
async fn payment_webhooks_require_valid_signatures_before_parsing() {
    let app = openb2c_runtime::router(service(), "{}".into(), false);
    let response = app
        .oneshot(
            Request::post("/commerce/payments/webhook")
                .header("content-type", "application/json")
                .header("x-payment-signature", "wrong")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value["code"], "unauthorized");
}

#[tokio::test]
async fn mcp_http_sessions_require_initialization_and_authentication() {
    let app = openb2c_runtime::router(service(), "{}".into(), true);
    let request = || {
        Request::post("/mcp")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string(),
            ))
            .unwrap()
    };
    assert_eq!(app.clone().oneshot(request()).await.unwrap().status(), 400);
    let initialized = app
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let session = initialized.headers()["mcp-session-id"].clone();
    let response = app
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("mcp-session-id", session)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

#[test]
fn mcp_listing_exposes_only_authorized_operations() {
    let service = service();
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let anonymous = openb2c_runtime::mcp::respond(&service, &request, &AuthContext::default());
    let tools = anonymous["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "list_performances"));
    assert!(
        !tools
            .iter()
            .any(|t| t["name"] == "list_tickets" || t["name"] == "delete_production")
    );
    let customer = AuthContext {
        user_id: Some(1),
        scopes: service.self_scopes(),
    };
    let listing = openb2c_runtime::mcp::respond(&service, &request, &customer);
    assert!(
        listing["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "list_tickets")
    );
    assert!(
        !listing["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "delete_production")
    );
}

#[test]
fn background_worker_delivers_committed_effects_without_an_http_request() {
    let path = std::env::temp_dir().join(format!(
        "openb2c-worker-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let service = openb2c_runtime::Service::open(common::schema(), &path, false).unwrap();
        service
            .queue_effect(
                "worker-email",
                "notify",
                &json!({"channel":"email","to":"worker@example.test","template":"test"}),
                &json!({}),
            )
            .unwrap();
        let worker =
            openb2c_runtime::effects::Worker::start(service.schema.clone(), path.to_str().unwrap())
                .unwrap();
        for _ in 0..100 {
            if !service.fake_emails().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        drop(worker);
        assert_eq!(service.fake_emails().unwrap().len(), 1);
        assert_eq!(service.effects(None).unwrap()[0]["status"], "succeeded");
    }
    std::fs::remove_file(path).unwrap();
}
