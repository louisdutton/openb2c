use axum::{Router, body::Body, http::Request};
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use openb2c_runtime::{AuthContext, Service};
use openb2c_schema::{Schema, capabilities::entity, sql::quote};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};
use tower::ServiceExt;

fn evaluated(selection: &str) -> std::process::Output {
    Command::new("nix")
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
        .unwrap()
}
fn schema(selection: &str) -> Schema {
    static MODELS: OnceLock<Mutex<BTreeMap<String, Schema>>> = OnceLock::new();
    let mut cache = MODELS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache
        .entry(selection.into())
        .or_insert_with(|| {
            let output = evaluated(selection);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice(&output.stdout).unwrap()
        })
        .clone()
}
fn service(selection: &str) -> Service {
    let service = Service::open(schema(selection), ":memory:", false).unwrap();
    if service.schema.capabilities.contains_key("customers") {
        service.ensure_user("alice@example.test").unwrap();
        service.ensure_user("bob@example.test").unwrap();
    }
    service
}
fn customer(service: &Service, id: i64) -> AuthContext {
    AuthContext {
        user_id: Some(id),
        scopes: service.self_scopes(),
    }
}
fn table<'a>(service: &'a Service, cap: &str, role: &str) -> &'a str {
    entity(&service.schema, cap, role).unwrap()
}
fn count(service: &Service, cap: &str, role: &str) -> i64 {
    service
        .db
        .query_row(
            &format!("SELECT count(*) FROM {}", quote(table(service, cap, role))),
            [],
            |r| r.get(0),
        )
        .unwrap()
}
fn make_order(service: &Service, auth: &AuthContext, key: &str) -> Value {
    service
        .b2c_create_order(
            &json!({"idempotency_key":key,"items":[{"price_id":1,"quantity":2}]}),
            auth,
        )
        .unwrap()
}
fn make_payment(service: &Service, auth: &AuthContext, order: &Value, key: &str) -> Value {
    service
        .b2c_order_payment(
            order["order"]["id"].as_i64().unwrap(),
            &json!({"idempotency_key":key}),
            auth,
        )
        .unwrap()
}
fn simulate(service: &Service, payment: &Value, event: &str, outcome: &str) -> Value {
    service
        .b2c_simulate_payment(
            payment["id"].as_i64().unwrap(),
            &json!({"event_id":event,"outcome":outcome}),
            &AuthContext::system(),
        )
        .unwrap()
}
fn subscribe(service: &Service, auth: &AuthContext, key: &str, price: i64) -> Value {
    service
        .b2c_create_subscription(&json!({"idempotency_key":key,"price_id":price}), auth)
        .unwrap()
}
fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn capability_dependencies_and_managed_write_contracts_fail_early() {
    let broken = evaluated("broken");
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stderr).contains("orders requires customers"));
    let broken = evaluated("broken_integration");
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stderr).contains("order_payments requires payments"));
    let valid = schema("full");
    for (mutation, message) in [
        ("missing", "missing dependency"),
        ("version", "unsupported contract version"),
        ("provider", "only provider = fake"),
        ("write", "must be disabled"),
        ("unique", "database uniqueness"),
    ] {
        let mut model = valid.clone();
        match mutation {
            "missing" => {
                model.capabilities.remove("payments");
            }
            "version" => {
                model.capabilities.get_mut("orders").unwrap().version = 2;
            }
            "provider" => {
                model.capabilities.get_mut("payments").unwrap().settings =
                    json!({"provider":"live"});
            }
            "write" => {
                model
                    .operations
                    .get_mut("payment")
                    .unwrap()
                    .get_mut("update")
                    .unwrap()
                    .enabled = true;
            }
            _ => {
                model
                    .constraints
                    .get_mut("purchase_order")
                    .unwrap()
                    .unique
                    .clear();
            }
        }
        assert!(
            openb2c_schema::validate::validate(&model)
                .join("\n")
                .contains(message),
            "{mutation}"
        );
    }
}

#[test]
fn capabilities_work_in_partial_compositions_without_unrelated_tables_or_routes() {
    for (selection, tables) in [
        ("customers", 2),
        ("catalog", 2),
        ("orders", 6),
        ("payments", 4),
        ("subscriptions", 8),
        ("entitlements", 3),
    ] {
        let service = service(selection);
        assert_eq!(service.schema.tables.len(), tables, "{selection}");
        let doc = openb2c_schema::contract::openapi(&service.schema);
        assert_eq!(
            doc["paths"].get("/b2c/orders").is_some(),
            selection == "orders"
        );
        assert_eq!(
            doc["paths"].get("/b2c/subscriptions").is_some(),
            selection == "subscriptions"
        );
        assert!(doc["paths"].get("/b2c/orders/{id}/payments").is_none());
        let auth = customer(&service, 1);
        match selection {
            "catalog" => assert_eq!(
                service.b2c_catalog().unwrap()["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            ),
            "orders" => {
                let order = make_order(&service, &auth, "standalone");
                assert_eq!(order["order"]["status"], "pending");
                assert!(
                    service
                        .b2c_order_payment(1, &json!({"idempotency_key":"x"}), &auth)
                        .is_err()
                );
            }
            "payments" => {
                let p=service.b2c_create_payment(&json!({"idempotency_key":"standalone","amount_minor":100,"currency":"GBP"}),&auth).unwrap();
                assert_eq!(
                    simulate(&service, &p, "paid", "succeeded")["status"],
                    "succeeded"
                );
            }
            "subscriptions" => {
                let s = subscribe(&service, &auth, "alone", 2);
                simulate(&service, &s["payment"], "active", "succeeded");
                assert_eq!(
                    service.b2c_subscription(1, &auth).unwrap()["subscription"]["status"],
                    "active"
                );
            }
            "entitlements" => {
                service
                    .b2c_grant_entitlement(
                        &json!({"idempotency_key":"manual","user_id":1,"resource":"club"}),
                        &AuthContext::system(),
                    )
                    .unwrap();
                assert_eq!(
                    service.b2c_entitlements(&auth).unwrap()["items"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
            }
            _ => assert!(!auth.scopes.iter().any(|s| s == "payments.simulate")),
        }
    }
}

#[test]
fn integrations_default_to_enabled_pairs_and_respect_consumer_opt_outs() {
    let full = schema("full");
    for integration in [
        "order_payments",
        "order_entitlements",
        "subscription_entitlements",
    ] {
        assert!(full.capabilities.contains_key(integration));
    }
    for selection in ["without_access", "without_entitlements"] {
        let service = service(selection);
        assert!(
            !service
                .schema
                .capabilities
                .contains_key("order_entitlements")
        );
        assert!(
            !service
                .schema
                .capabilities
                .contains_key("subscription_entitlements")
        );
        assert_eq!(
            service.schema.tables.contains_key("entitlement"),
            selection == "without_access"
        );
        let auth = customer(&service, 1);
        let order = make_order(&service, &auth, "checkout");
        let payment = make_payment(&service, &auth, &order, "pay");
        simulate(&service, &payment, "paid", "succeeded");
        assert_eq!(
            service
                .b2c_order(order["order"]["id"].as_i64().unwrap(), &auth)
                .unwrap()["order"]["status"],
            "paid"
        );
        let subscription = subscribe(&service, &auth, "membership", 2);
        simulate(&service, &subscription["payment"], "active", "succeeded");
        if selection == "without_access" {
            assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
            service
                .b2c_grant_entitlement(
                    &json!({"idempotency_key":"manual","user_id":1,"resource":"handbook"}),
                    &AuthContext::system(),
                )
                .unwrap();
            simulate(&service, &payment, "refund", "refunded");
            assert_eq!(
                service.b2c_entitlements(&auth).unwrap()["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
    }
    let service = service("without_order_payments");
    assert!(!service.schema.capabilities.contains_key("order_payments"));
    assert!(!service.schema.tables.contains_key("order_payment"));
    assert!(
        openb2c_schema::contract::openapi(&service.schema)["paths"]
            .get("/b2c/orders/{id}/payments")
            .is_none()
    );
    let auth = customer(&service, 1);
    let order = make_order(&service, &auth, "unpaid");
    assert_eq!(order["order"]["status"], "pending");
    assert!(
        service
            .b2c_order_payment(
                order["order"]["id"].as_i64().unwrap(),
                &json!({"idempotency_key":"disabled"}),
                &auth
            )
            .is_err()
    );
}

#[test]
fn explicit_role_bindings_work_without_conventional_entity_names() {
    let service = service("renamed");
    let auth = customer(&service, 1);
    let order = make_order(&service, &auth, "renamed");
    let payment = make_payment(&service, &auth, &order, "attempt");
    simulate(&service, &payment, "renamed-paid", "succeeded");
    assert_eq!(
        service.b2c_order(1, &auth).unwrap()["order"]["status"],
        "paid"
    );
    let sub = subscribe(&service, &auth, "renamed-sub", 2);
    simulate(&service, &sub["payment"], "renamed-active", "succeeded");
    assert_eq!(
        service.b2c_entitlements(&auth).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!service.schema.tables.contains_key("payment"));
}

#[test]
fn order_totals_and_resources_are_snapshotted_and_payment_replays_do_not_regrant_refunds() {
    let service = service("full");
    let auth = customer(&service, 1);
    let order = make_order(&service, &auth, "purchase");
    assert_eq!(order["order"]["amount_minor"], 3000);
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    service
        .db
        .execute("UPDATE price SET amount_minor=9999,active=0 WHERE id=1", [])
        .unwrap();
    service
        .db
        .execute(
            "UPDATE product SET code='new-code',name='Renamed' WHERE id=1",
            [],
        )
        .unwrap();
    let repeated = make_order(&service, &auth, "purchase");
    assert_eq!(repeated, order);
    let p = make_payment(&service, &auth, &order, "pay");
    assert_eq!(p["amount_minor"], 3000);
    let result = simulate(&service, &p, "paid", "succeeded");
    let audit = service
        .query("SELECT * FROM openb2c_rust_audit", &[])
        .unwrap()
        .len();
    assert_eq!(simulate(&service, &p, "paid", "succeeded"), result);
    assert_eq!(
        service
            .query("SELECT * FROM openb2c_rust_audit", &[])
            .unwrap()
            .len(),
        audit
    );
    assert_eq!(
        service.b2c_entitlements(&auth).unwrap()["items"][0]["resource"],
        "handbook"
    );
    assert_eq!(count(&service, "entitlements", "entitlement"), 1);
    assert!(service.b2c_cancel_order(1, &auth).is_err());
    simulate(&service, &p, "refund", "refunded");
    assert_eq!(
        service.b2c_order(1, &auth).unwrap()["order"]["status"],
        "refunded"
    );
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    simulate(&service, &p, "paid", "succeeded");
    assert_eq!(service.b2c_payment(1, &auth).unwrap()["status"], "refunded");
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    assert_eq!(
        service
            .b2c_simulate_payment(
                1,
                &json!({"event_id":"paid","outcome":"failed"}),
                &AuthContext::system()
            )
            .unwrap_err()
            .code,
        "conflict"
    );
}

#[test]
fn failed_payments_can_retry_and_cancelled_orders_reject_late_settlement() {
    let service = service("full");
    let auth = customer(&service, 1);
    let order = make_order(&service, &auth, "retry");
    let first = make_payment(&service, &auth, &order, "first");
    assert!(
        service
            .b2c_order_payment(1, &json!({"idempotency_key":"parallel"}), &auth)
            .is_err()
    );
    simulate(&service, &first, "failed", "failed");
    assert_eq!(
        service.b2c_order(1, &auth).unwrap()["order"]["status"],
        "pending"
    );
    let next = make_payment(&service, &auth, &order, "second");
    simulate(&service, &next, "retry-paid", "succeeded");
    let other = make_order(&service, &auth, "cancel");
    let pending = make_payment(&service, &auth, &other, "cancel");
    let id = other["order"]["id"].as_i64().unwrap();
    service.b2c_cancel_order(id, &auth).unwrap();
    assert_eq!(
        service
            .b2c_payment(pending["id"].as_i64().unwrap(), &auth)
            .unwrap()["status"],
        "cancelled"
    );
    assert!(
        service
            .b2c_simulate_payment(
                pending["id"].as_i64().unwrap(),
                &json!({"event_id":"late","outcome":"succeeded"}),
                &AuthContext::system()
            )
            .is_err()
    );
    assert_eq!(
        service.b2c_entitlements(&auth).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn subscription_renewals_are_due_once_retry_failures_and_preserve_month_end_anchor() {
    let service = service("full");
    let auth = customer(&service, 1);
    let sub = subscribe(&service, &auth, "monthly", 2);
    service
        .db
        .execute(
            "UPDATE price SET amount_minor=9999,currency='USD',billing_interval='year' WHERE id=2",
            [],
        )
        .unwrap();
    assert_eq!(sub["subscription"]["status"], "pending");
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    let operator = AuthContext::system();
    let anchor = at("2030-01-31T12:00:00Z");
    service
        .b2c_simulate_payment_at(
            1,
            &json!({"event_id":"start","outcome":"succeeded"}),
            &operator,
            anchor,
        )
        .unwrap();
    let end = at("2030-02-28T12:00:00Z");
    assert_eq!(
        service.b2c_subscription(1, &auth).unwrap()["subscription"]["current_period_end"],
        end.to_rfc3339()
    );
    assert_eq!(
        service
            .b2c_renew_due_at(&operator, end - chrono::Duration::seconds(1))
            .unwrap()["payments"],
        json!([])
    );
    let due = service.b2c_renew_due_at(&operator, end).unwrap();
    let payment = due["payments"][0].clone();
    assert_eq!(payment["amount_minor"], 500);
    assert_eq!(payment["currency"], "GBP");
    assert_eq!(due["payments"].as_array().unwrap().len(), 1);
    assert_eq!(
        service.b2c_renew_due_at(&operator, end).unwrap()["payments"],
        json!([])
    );
    assert_eq!(
        service.b2c_entitlements_at(&auth, end).unwrap()["items"],
        json!([])
    );
    service
        .b2c_simulate_payment_at(
            payment["id"].as_i64().unwrap(),
            &json!({"event_id":"renew-failed","outcome":"failed"}),
            &operator,
            end,
        )
        .unwrap();
    assert_eq!(
        service.b2c_subscription(1, &auth).unwrap()["subscription"]["status"],
        "past_due"
    );
    let retry = service
        .b2c_retry_subscription(1, &json!({"idempotency_key":"retry"}), &auth)
        .unwrap();
    assert_eq!(
        service
            .b2c_retry_subscription(1, &json!({"idempotency_key":"retry"}), &auth)
            .unwrap(),
        retry
    );
    service
        .b2c_simulate_payment_at(
            retry["payment"]["id"].as_i64().unwrap(),
            &json!({"event_id":"renewed","outcome":"succeeded"}),
            &operator,
            end + chrono::Duration::seconds(1),
        )
        .unwrap();
    let current = service.b2c_subscription(1, &auth).unwrap();
    assert_eq!(current["subscription"]["cycle"], 1);
    assert_eq!(
        current["subscription"]["current_period_end"],
        at("2030-03-31T12:00:00Z").to_rfc3339()
    );
    assert_eq!(
        service
            .b2c_entitlements_at(&auth, end + chrono::Duration::seconds(1))
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(count(&service, "entitlements", "entitlement"), 1);
}

#[test]
fn yearly_billing_preserves_leap_day_anchor() {
    let service = service("subscriptions");
    let auth = customer(&service, 1);
    let operator = AuthContext::system();
    let sub = subscribe(&service, &auth, "yearly", 3);
    service
        .b2c_simulate_payment_at(
            sub["payment"]["id"].as_i64().unwrap(),
            &json!({"event_id":"year-0","outcome":"succeeded"}),
            &operator,
            at("2028-02-29T10:00:00Z"),
        )
        .unwrap();
    for (cycle, boundary) in [
        (1, "2029-02-28T10:00:00Z"),
        (2, "2030-02-28T10:00:00Z"),
        (3, "2031-02-28T10:00:00Z"),
    ] {
        let due = service.b2c_renew_due_at(&operator, at(boundary)).unwrap();
        service
            .b2c_simulate_payment_at(
                due["payments"][0]["id"].as_i64().unwrap(),
                &json!({"event_id":format!("year-{cycle}"),"outcome":"succeeded"}),
                &operator,
                at(boundary),
            )
            .unwrap();
    }
    assert_eq!(
        service.b2c_subscription(1, &auth).unwrap()["subscription"]["current_period_end"],
        at("2032-02-29T10:00:00Z").to_rfc3339()
    );
}

#[test]
fn pending_renewals_do_not_starve_later_due_subscriptions() {
    let service = service("subscriptions");
    let auth = customer(&service, 1);
    let admin = AuthContext::system();
    let anchor = at("2030-01-01T00:00:00Z");
    for index in 0..101 {
        let sub = subscribe(&service, &auth, &format!("batch-{index}"), 2);
        service
            .b2c_simulate_payment_at(
                sub["payment"]["id"].as_i64().unwrap(),
                &json!({"event_id":format!("batch-{index}"),"outcome":"succeeded"}),
                &admin,
                anchor,
            )
            .unwrap();
    }
    let due = at("2030-02-01T00:00:00Z");
    assert_eq!(
        service.b2c_renew_due_at(&admin, due).unwrap()["payments"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
    assert_eq!(
        service.b2c_renew_due_at(&admin, due).unwrap()["payments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        service.b2c_renew_due_at(&admin, due).unwrap()["payments"],
        json!([])
    );
}

#[test]
fn subscription_cancellation_and_refunds_remove_access_at_the_correct_boundary() {
    let service = service("full");
    let auth = customer(&service, 1);
    let operator = AuthContext::system();
    let sub = subscribe(&service, &auth, "period-end", 2);
    simulate(&service, &sub["payment"], "first", "succeeded");
    let cancelled = service
        .b2c_cancel_subscription(1, &json!({}), &auth)
        .unwrap();
    assert_eq!(cancelled["subscription"]["status"], "active");
    assert_eq!(cancelled["subscription"]["cancel_at_period_end"], 1);
    assert_eq!(
        service.b2c_entitlements(&auth).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let end = at(cancelled["subscription"]["current_period_end"]
        .as_str()
        .unwrap());
    let due = service.b2c_renew_due_at(&operator, end).unwrap();
    assert_eq!(due["cancelled"], 1);
    assert_eq!(due["payments"], json!([]));
    assert_eq!(
        service.b2c_entitlements_at(&auth, end).unwrap()["items"],
        json!([])
    );
    let sub = subscribe(&service, &auth, "immediate", 2);
    simulate(&service, &sub["payment"], "second", "succeeded");
    service
        .b2c_cancel_subscription(2, &json!({"at_period_end":false}), &auth)
        .unwrap();
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    let sub = subscribe(&service, &auth, "refund", 2);
    simulate(&service, &sub["payment"], "third", "succeeded");
    simulate(&service, &sub["payment"], "refund", "refunded");
    assert_eq!(
        service.b2c_subscription(3, &auth).unwrap()["subscription"]["status"],
        "cancelled"
    );
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    let sub = subscribe(&service, &auth, "pending", 2);
    service
        .b2c_cancel_subscription(4, &json!({}), &auth)
        .unwrap();
    assert!(
        service
            .b2c_simulate_payment(
                sub["payment"]["id"].as_i64().unwrap(),
                &json!({"event_id":"late","outcome":"succeeded"}),
                &operator
            )
            .is_err()
    );
}

#[test]
fn customers_cannot_forge_prices_settle_payments_reassign_records_or_grant_access() {
    let service = service("full");
    let alice = customer(&service, 1);
    let bob = customer(&service, 2);
    assert!(!alice.has_scope("payments.simulate"));
    assert!(!alice.has_scope("subscriptions.renew"));
    assert!(!alice.has_scope("entitlements.grant"));
    let o = make_order(&service, &alice, "owned");
    let p = make_payment(&service, &alice, &o, "owned");
    let sub = subscribe(&service, &alice, "owned", 2);
    for result in [
        service.b2c_order(1, &bob),
        service.b2c_cancel_order(1, &bob),
        service.b2c_payment(1, &bob),
        service.b2c_subscription(1, &bob),
        service.b2c_cancel_subscription(1, &json!({}), &bob),
        service.b2c_retry_subscription(1, &json!({"idempotency_key":"steal"}), &bob),
    ] {
        assert_eq!(result.unwrap_err().code, "forbidden");
    }
    assert_eq!(
        service
            .b2c_simulate_payment(
                p["id"].as_i64().unwrap(),
                &json!({"event_id":"forged","outcome":"succeeded"}),
                &alice
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert!(service.b2c_create_order(&json!({"idempotency_key":"forged","items":[{"price_id":1,"quantity":1}],"amount_minor":1}),&alice).is_err());
    assert!(
        service
            .b2c_create_subscription(
                &json!({"idempotency_key":"forged","price_id":2,"status":"active"}),
                &alice
            )
            .is_err()
    );
    for (cap, role) in [
        ("orders", "order"),
        ("payments", "payment"),
        ("subscriptions", "subscription"),
        ("entitlements", "entitlement"),
    ] {
        for action in ["create", "update", "delete"] {
            assert_eq!(
                service
                    .authorize(
                        table(&service, cap, role),
                        action,
                        &AuthContext::system(),
                        None
                    )
                    .unwrap_err()
                    .code,
                "not_found"
            );
        }
    }
    assert!(
        service
            .b2c_grant_entitlement(
                &json!({"idempotency_key":"x","user_id":1,"resource":"membership"}),
                &alice
            )
            .is_err()
    );
    assert_eq!(count(&service, "payments", "payment"), 2);
    assert_eq!(sub["subscription"]["status"], "pending");
}

#[test]
fn totals_currency_intervals_and_idempotency_payloads_are_validated() {
    let service = service("full");
    let auth = customer(&service, 1);
    for items in [
        json!([]),
        json!([{"price_id":1,"quantity":0}]),
        json!([{"price_id":1,"quantity":101}]),
        json!([{"price_id":1,"quantity":1},{"price_id":4,"quantity":1}]),
        json!([{"price_id":2,"quantity":1}]),
        json!([{"price_id":1,"quantity":1},{"price_id":1,"quantity":1}]),
    ] {
        assert!(
            service
                .b2c_create_order(&json!({"idempotency_key":"invalid","items":items}), &auth)
                .is_err()
        );
    }
    assert_eq!(count(&service, "orders", "order"), 0);
    assert!(
        service
            .b2c_create_subscription(&json!({"idempotency_key":"one-off","price_id":1}), &auth)
            .is_err()
    );
    make_order(&service, &auth, "key");
    assert_eq!(
        service
            .b2c_create_order(
                &json!({"idempotency_key":"key","items":[{"price_id":1,"quantity":1}]}),
                &auth
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    service
        .db
        .execute("UPDATE price SET amount_minor=? WHERE id=1", [i64::MAX])
        .unwrap();
    assert!(
        service
            .b2c_create_order(
                &json!({"idempotency_key":"overflow","items":[{"price_id":1,"quantity":2}]}),
                &auth
            )
            .is_err()
    );
    assert_eq!(count(&service, "orders", "order"), 1);
}

#[test]
fn settlement_and_entitlement_failures_roll_back_all_participants_and_event_receipts() {
    let service = service("full");
    let auth = customer(&service, 1);
    let o = make_order(&service, &auth, "atomic");
    let p = make_payment(&service, &auth, &o, "atomic");
    let audits = service
        .query("SELECT * FROM openb2c_rust_audit", &[])
        .unwrap()
        .len();
    service.db.execute_batch("CREATE TRIGGER reject_grant BEFORE INSERT ON entitlement BEGIN SELECT RAISE(ABORT,'test delivery failure'); END;").unwrap();
    assert!(
        service
            .b2c_simulate_payment(
                1,
                &json!({"event_id":"atomic","outcome":"succeeded"}),
                &AuthContext::system()
            )
            .is_err()
    );
    assert_eq!(service.b2c_payment(1, &auth).unwrap()["status"], "pending");
    assert_eq!(
        service.b2c_order(1, &auth).unwrap()["order"]["status"],
        "pending"
    );
    assert_eq!(count(&service, "payments", "event"), 0);
    assert_eq!(
        service
            .query("SELECT * FROM openb2c_rust_audit", &[])
            .unwrap()
            .len(),
        audits
    );
    service
        .db
        .execute_batch("DROP TRIGGER reject_grant;")
        .unwrap();
    simulate(&service, &p, "atomic", "succeeded");
    assert_eq!(count(&service, "entitlements", "entitlement"), 1);
}

#[test]
fn standalone_entitlements_are_idempotent_and_revoked_grants_stay_revoked() {
    let service = service("entitlements");
    let auth = customer(&service, 1);
    let admin = AuthContext::system();
    let input = json!({"idempotency_key":"manual","user_id":1,"resource":"external-service"});
    let grant = service.b2c_grant_entitlement(&input, &admin).unwrap();
    assert_eq!(
        service.b2c_grant_entitlement(&input, &admin).unwrap(),
        grant
    );
    assert_eq!(
        service.b2c_entitlements(&customer(&service, 2)).unwrap()["items"],
        json!([])
    );
    service.b2c_revoke_entitlement(1, &admin).unwrap();
    assert_eq!(
        service.b2c_grant_entitlement(&input, &admin).unwrap()["status"],
        "revoked"
    );
    assert_eq!(service.b2c_entitlements(&auth).unwrap()["items"], json!([]));
    assert_eq!(
        service
            .b2c_grant_entitlement(
                &json!({"idempotency_key":"manual","user_id":2,"resource":"external-service"}),
                &admin
            )
            .unwrap_err()
            .code,
        "conflict"
    );
}

#[test]
fn capability_responses_preserve_extension_defaults_redaction_and_derived_fields() {
    let mut model = schema("full");
    model.tables.get_mut("payment").unwrap().insert("operator_note".into(),serde_json::from_value(json!({"type":"text","default":"'internal-only'","metadata":{"privacy":"secret","redact":true}})).unwrap());
    model.derived.entry("purchase_order".into()).or_default().insert("total_label".into(),serde_json::from_value(json!({"type":"text","template":"{currency} {amount_minor}","dependencies":[{"table":"purchase_order","field":"currency"},{"table":"purchase_order","field":"amount_minor"}]})).unwrap());
    let service = Service::open(model, ":memory:", false).unwrap();
    service.ensure_user("alice@example.test").unwrap();
    let auth = customer(&service, 1);
    let order = make_order(&service, &auth, "extension");
    assert_eq!(order["order"]["total_label"], "GBP 3000");
    let payment = make_payment(&service, &auth, &order, "extension");
    assert!(payment.get("operator_note").is_none());
    assert_eq!(
        service.raw("payment", &payment["id"]).unwrap()["operator_note"],
        "internal-only"
    );
    let result = simulate(&service, &payment, "extension-paid", "succeeded");
    assert!(result.get("operator_note").is_none());
    assert_eq!(
        simulate(&service, &payment, "extension-paid", "succeeded"),
        result
    );
    assert!(
        service
            .query("SELECT * FROM payment_event", &[])
            .unwrap()
            .iter()
            .all(|row| !row.to_string().contains("internal-only"))
    );
    let subscription = subscribe(&service, &auth, "extension", 2);
    assert!(subscription["payment"].get("operator_note").is_none());
    assert!(
        !service
            .b2c_pending_payments(&AuthContext::system())
            .unwrap()
            .to_string()
            .contains("internal-only")
    );
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "b2c-concurrency-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn concurrent_requests_and_settlement_are_idempotent_on_separate_connections() {
    let temp = Temp::new();
    let db = temp.0.join("app.sqlite");
    let model = schema("full");
    let a = Service::open(model.clone(), &db, false).unwrap();
    a.ensure_user("alice@example.test").unwrap();
    let auth = customer(&a, 1);
    let b = Service::open(model.clone(), &db, false).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs = [a, b]
        .into_iter()
        .map(|service| {
            let barrier = barrier.clone();
            let auth = auth.clone();
            std::thread::spawn(move || {
                barrier.wait();
                make_order(&service, &auth, "same")["order"]["id"].clone()
            })
        })
        .collect::<Vec<_>>();
    for job in jobs {
        assert_eq!(job.join().unwrap(), 1);
    }
    let a = Service::open(model.clone(), &db, false).unwrap();
    let p = make_payment(&a, &auth, &a.b2c_order(1, &auth).unwrap(), "payment");
    let b = Service::open(model.clone(), &db, false).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs = [a, b]
        .into_iter()
        .map(|service| {
            let barrier = barrier.clone();
            let p = p.clone();
            std::thread::spawn(move || {
                barrier.wait();
                simulate(&service, &p, "event", "succeeded")
            })
        })
        .collect::<Vec<_>>();
    for job in jobs {
        assert_eq!(job.join().unwrap()["status"], "succeeded");
    }
    let service = Service::open(model, &db, false).unwrap();
    assert_eq!(count(&service, "orders", "order"), 1);
    assert_eq!(count(&service, "payments", "event"), 1);
    assert_eq!(count(&service, "entitlements", "entitlement"), 1);
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        builder = builder.header("authorization", format!("Bearer {key}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(
                    body.map(|v| Body::from(v.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        expected,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn rest_pilot_exercises_one_off_recurring_access_and_separate_simulator_permissions() {
    let service = service("full");
    let mut keys = Vec::new();
    for (user, scopes) in [
        (1, service.self_scopes().join(",")),
        (2, "payments.simulate,subscriptions.renew".to_string()),
    ] {
        keys.push(
            service
                .create_api_key(
                    &json!({"user_id":user,"name":"Test","scopes":scopes}),
                    &AuthContext::system(),
                )
                .unwrap()["key"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let doc = openb2c_schema::contract::openapi(&service.schema);
    assert!(doc["paths"]["/api/payments/{id}"].get("patch").is_none());
    assert_eq!(
        doc["paths"]["/b2c/payments/{id}/simulate"]["post"]["x-openb2c-scope"],
        "payments.simulate"
    );
    let app = openb2c_runtime::router(service, doc.to_string(), true);
    request(&app, "GET", "/b2c/catalog", None, None, 200).await;
    let input = json!({"idempotency_key":"rest-order","items":[{"price_id":1,"quantity":1}]});
    request(&app, "POST", "/b2c/orders", None, Some(input.clone()), 401).await;
    let order = request(
        &app,
        "POST",
        "/b2c/orders",
        Some(&keys[0]),
        Some(input),
        200,
    )
    .await;
    assert_eq!(order["order"]["amount_minor"], 1500);
    let p = request(
        &app,
        "POST",
        "/b2c/orders/1/payments",
        Some(&keys[0]),
        Some(json!({"idempotency_key":"pay"})),
        200,
    )
    .await;
    let path = format!("/b2c/payments/{}/simulate", p["id"]);
    request(
        &app,
        "GET",
        "/b2c/payments/pending",
        Some(&keys[0]),
        None,
        403,
    )
    .await;
    assert_eq!(
        request(
            &app,
            "GET",
            "/b2c/payments/pending",
            Some(&keys[1]),
            None,
            200
        )
        .await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let event = json!({"event_id":"rest-paid","outcome":"succeeded"});
    request(
        &app,
        "POST",
        &path,
        Some(&keys[0]),
        Some(event.clone()),
        403,
    )
    .await;
    request(&app, "POST", &path, Some(&keys[1]), Some(event), 200).await;
    assert_eq!(
        request(&app, "GET", "/b2c/entitlements", Some(&keys[0]), None, 200).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let sub = request(
        &app,
        "POST",
        "/b2c/subscriptions",
        Some(&keys[0]),
        Some(json!({"idempotency_key":"rest-sub","price_id":2})),
        200,
    )
    .await;
    request(
        &app,
        "POST",
        &format!("/b2c/payments/{}/simulate", sub["payment"]["id"]),
        Some(&keys[1]),
        Some(json!({"event_id":"rest-active","outcome":"succeeded"})),
        200,
    )
    .await;
    assert_eq!(
        request(&app, "GET", "/b2c/entitlements", Some(&keys[0]), None, 200).await["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    request(
        &app,
        "POST",
        "/b2c/subscriptions/1/cancel",
        Some(&keys[0]),
        Some(json!({"at_period_end":false})),
        200,
    )
    .await;
    assert_eq!(
        request(&app, "GET", "/b2c/entitlements", Some(&keys[0]), None, 200).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    request(
        &app,
        "PATCH",
        "/api/payments/1",
        Some(&keys[0]),
        Some(json!({"status":"succeeded"})),
        404,
    )
    .await;
    for path in ["/", "/admin", "/mcp", "/commerce/catalog"] {
        request(&app, "GET", path, None, None, 404).await;
    }
}
