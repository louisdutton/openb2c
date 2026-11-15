mod common;
use axum::{body::Body, http::Request};
use common::service;
use openb2c_runtime::{
    identity::Registry,
    web::{Settings, router_with_settings},
};
use serde_json::json;
use tower::ServiceExt;

#[test]
fn openapi_describes_schema_metadata_and_operations() {
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
