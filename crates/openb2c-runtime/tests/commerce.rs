mod common;
use common::service;
use openb2c_runtime::{AuthContext, identity::after};
use serde_json::{Value, json};
fn customer(service: &openb2c_runtime::Service, id: i64) -> AuthContext {
    AuthContext {
        user_id: Some(id),
        scopes: service.self_scopes(),
    }
}
fn checkout(service: &openb2c_runtime::Service, id: i64, quantity: i64) -> Value {
    service.checkout(&json!({"items":[{"item_id":1,"quantity":quantity,"options":{"ticket_type":"standard"}}]}),&customer(service,id)).unwrap()
}
#[test]
fn catalog_infers_roles_and_looks_up_fk_labels() {
    let service = service();
    let catalog = service.catalog().unwrap();
    assert_eq!(catalog["items"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["lookups"]["production_id"]["1"], "The Magic Flute");
    assert_eq!(catalog["lookups"]["venue_id"]["1"], "Hall for Cornwall");
    let c = service.commerce().unwrap();
    assert_eq!(c.order.entity, "booking");
    assert_eq!(c.line_item.entity, "ticket");
}
#[test]
fn checkout_payment_idempotency_receipt_and_ticket_ownership() {
    let service = service();
    let auth = customer(&service, 1);
    let order = checkout(&service, 1, 2);
    assert_eq!(order["amount_pence"], 5000);
    assert_eq!(order["ticket_ids"].as_array().unwrap().len(), 2);
    assert!(
        service
            .payment_session(&order["order_id"], &customer(&service, 2))
            .is_err()
    );
    let payment = service.payment_session(&order["order_id"], &auth).unwrap();
    assert_eq!(
        payment["reference"],
        service.payment_session(&order["order_id"], &auth).unwrap()["reference"]
    );
    let event =
        json!({"event_id":"paid-once","reference":payment["reference"],"status":"authorized"});
    let result = service.payment_webhook(&event).unwrap();
    assert_eq!(result["status"], "paid");
    assert_eq!(service.payment_webhook(&event).unwrap()["idempotent"], true);
    assert!(
        service
            .payment_webhook(
                &json!({"event_id":"paid-once","reference":payment["reference"],"status":"failed"})
            )
            .is_err()
    );
    for id in order["ticket_ids"].as_array().unwrap() {
        assert_eq!(
            service.find("ticket", id, &auth).unwrap()["status"],
            "confirmed"
        );
        assert!(service.find("ticket", id, &customer(&service, 2)).is_err());
    }
    let receipt = service
        .effects(None)
        .unwrap()
        .into_iter()
        .filter(|e| e["effect_type"] == "notify")
        .collect::<Vec<_>>();
    assert_eq!(receipt.len(), 1);
    let payload: Value =
        serde_json::from_str(receipt[0]["payload_json"].as_str().unwrap()).unwrap();
    assert_eq!(payload["customer"]["email"], "ada@example.test");
    assert_eq!(payload["line_items"].as_array().unwrap().len(), 2);
    assert_eq!(
        service
            .payment_webhook(
                &json!({"event_id":"refund","reference":payment["reference"],"status":"refunded"})
            )
            .unwrap()["status"],
        "refunded"
    );
}
#[test]
fn checkout_rejects_overbooking_forged_owners_options_and_bad_quantities() {
    let service = service();
    service
        .db
        .execute("UPDATE venue SET capacity=2 WHERE id=1", [])
        .unwrap();
    checkout(&service, 1, 2);
    assert_eq!(
        service
            .checkout(
                &json!({"items":[{"item_id":1,"quantity":1}]}),
                &customer(&service, 2)
            )
            .unwrap_err()
            .code,
        "bad_state"
    );
    for input in [
        json!({"items":[{"item_id":2,"quantity":1}]}),
        json!({"items":[{"item_id":1,"quantity":-1}]}),
        json!({"items":[{"item_id":1,"quantity":1.5}]}),
        json!({"items":[{"item_id":1,"options":{"price_pence":1}}]}),
        json!({"items":[{"item_id":1,"options":{"ticket_type":"invented"}}]}),
        json!({"user_id":2,"items":[{"item_id":1}]}),
    ] {
        assert!(service.checkout(&input, &customer(&service, 1)).is_err());
    }
    assert!(
        service
            .checkout(&json!({"items":[{"item_id":1}]}), &AuthContext::default())
            .is_err()
    );
    let count: i64 = service
        .db
        .query_row("SELECT count(*) FROM booking", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn late_authorizations_cancel_and_late_captures_require_refunds_after_reallocation() {
    for (status, expected) in [("authorized", "cancelled"), ("captured", "refund_required")] {
        let service = service();
        service
            .db
            .execute("UPDATE venue SET capacity=1 WHERE id=1", [])
            .unwrap();
        let old = checkout(&service, 1, 1);
        let payment = service
            .payment_session(&old["order_id"], &customer(&service, 1))
            .unwrap();
        service
            .db
            .execute(
                "UPDATE booking SET payment_deadline_at=? WHERE id=?",
                rusqlite::params![after(-60), old["order_id"].as_i64()],
            )
            .unwrap();
        let replacement = checkout(&service, 2, 1);
        let result = service
            .payment_webhook(
                &json!({"event_id":status,"reference":payment["reference"],"status":status}),
            )
            .unwrap();
        assert_eq!(result["status"], expected);
        assert_eq!(
            service
                .raw("ticket", &replacement["ticket_ids"][0])
                .unwrap()["status"],
            "reserved"
        );
        assert_eq!(
            service.raw("ticket", &old["ticket_ids"][0]).unwrap()["status"],
            "cancelled"
        );
    }
}
#[test]
fn expiry_releases_capacity_and_cancels_pending_transactions() {
    let service = service();
    service
        .db
        .execute("UPDATE venue SET capacity=1 WHERE id=1", [])
        .unwrap();
    let old = checkout(&service, 1, 1);
    service
        .payment_session(&old["order_id"], &customer(&service, 1))
        .unwrap();
    service
        .db
        .execute("UPDATE booking SET payment_deadline_at=?", [after(-60)])
        .unwrap();
    assert_eq!(service.expire_orders().unwrap()["expired"], 1);
    assert_eq!(
        service.raw("booking", &old["order_id"]).unwrap()["status"],
        "expired"
    );
    checkout(&service, 2, 1);
}
#[test]
fn booking_aliases_use_the_same_commerce_graph() {
    let service = service();
    let result = service
        .reserve_booking(
            &json!({"performance_id":1,"quantity":2,"ticket_type":"patron"}),
            &customer(&service, 1),
        )
        .unwrap();
    assert_eq!(result["booking_id"], result["order_id"]);
    assert_eq!(
        service.raw("ticket", &result["ticket_ids"][0]).unwrap()["ticket_type"],
        "patron"
    );
}
#[test]
fn foreign_key_junction_cascades_follow_schema_without_name_conventions() {
    let service = service();
    let result = checkout(&service, 1, 2);
    let c = service.commerce().unwrap();
    let op = &service.schema.operations[&c.order.entity]["cancel"];
    assert!(!op.cascade.is_empty());
    service
        .mutate(
            "booking",
            &result["booking_id"],
            "cancel",
            &json!({}),
            &AuthContext::system(),
            None,
        )
        .unwrap();
    for id in result["ticket_ids"].as_array().unwrap() {
        assert_eq!(service.raw("ticket", id).unwrap()["status"], "cancelled");
    }
}
