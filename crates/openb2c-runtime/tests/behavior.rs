use openb2c_runtime::{
    AuthContext, Service,
    service::{ListOptions, etag},
};
use openb2c_schema::Schema;
use serde_json::{Value, json};

fn schema() -> Schema {
    serde_json::from_value(json!({
        "organization":{"name":"Example <Opera>","description":"Schema driven"},
        "tables":{
            "user":{"id":{"type":"integer","pk":true,"auto":true},"name":{"type":"text","required":true}},
            "ticket":{
                "id":{"type":"integer","pk":true,"auto":true},
                "user_id":{"type":"integer","required":true,"references":"user(id)"},
                "name":{"type":"text","required":true,"constraints":{"length":{"min":1,"max":30},"pattern":"^[A-Z]"}},
                "status":{"type":"text","required":true,"default":"'reserved'","constraints":{"enum":["reserved","confirmed","cancelled"]}},
                "secret":{"type":"text","metadata":{"privacy":"secret","redact":true}},
                "price":{"type":"integer","required":true,"constraints":{"range":{"min":0,"max":10000}}}
            },
            "line":{"id":{"type":"integer","pk":true,"auto":true},"ticket_id":{"type":"integer","required":true,"references":"ticket(id)"},"status":{"type":"text","default":"'reserved'","constraints":{"enum":["reserved","confirmed"]}}}
        },
        "operations":{
            "user":{"read":{"public":true}},
            "ticket":{
                "read":{"relationships":[{"field":{"table":"ticket","field":"user_id","references":"user(id)"}}]},
                "create":{"relationships":[{"field":{"table":"ticket","field":"user_id","references":"user(id)"}}]},
                "update":{"relationships":[{"field":{"table":"ticket","field":"user_id","references":"user(id)"}}]},
                "delete":{"relationships":[{"field":{"table":"ticket","field":"user_id","references":"user(id)"}}]},
                "confirm":{
                    "relationships":[{"field":{"table":"ticket","field":"user_id","references":"user(id)"}}],
                    "guard":{"_t":"bin","op":"==","left":{"_t":"field","name":"status"},"right":{"_t":"lit","value":"reserved"}},
                    "set":{"status":{"value":"confirmed"}},
                    "cascade":[{"entity":{"table":"line"},"via":null,"set":{"status":{"value":"confirmed"}}}],
                    "effects":[{"emit":{"event":"ticket_confirmed"}}],
                    "audit":{"required":true,"summary":"Confirmed ticket"}
                }
            }
        }
    })).unwrap()
}

fn setup() -> Service {
    let service = Service::open(schema(), ":memory:", false).unwrap();
    service
        .create("user", &json!({"name":"Ada"}), &AuthContext::system())
        .unwrap();
    service
        .create("user", &json!({"name":"Morgan"}), &AuthContext::system())
        .unwrap();
    service
}

fn customer(id: i64) -> AuthContext {
    AuthContext {
        user_id: Some(id),
        scopes: ["read", "create", "update", "delete", "confirm"]
            .map(|op| format!("ticket.{op}"))
            .to_vec(),
    }
}
fn ticket(service: &Service, owner: i64) -> Value {
    service
        .create(
            "ticket",
            &json!({"name":"Balcony", "price":2500, "secret":"Never expose this"}),
            &customer(owner),
        )
        .unwrap()
}

#[test]
fn collections_and_records_enforce_ownership() {
    let service = setup();
    let first = ticket(&service, 1);
    ticket(&service, 2);
    let list = service
        .list("ticket", &ListOptions::default(), &customer(1))
        .unwrap();
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["id"], first["id"]);
    assert_eq!(
        service
            .find("ticket", &first["id"], &customer(2))
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .list("ticket", &ListOptions::default(), &AuthContext::default())
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(
        service
            .list("user", &ListOptions::default(), &AuthContext::default())
            .unwrap()["total"],
        2
    );
}

#[test]
fn ownership_cannot_be_forged_or_reassigned() {
    let service = setup();
    assert_eq!(
        service
            .create(
                "ticket",
                &json!({"name":"Balcony","price":2500,"user_id":2}),
                &customer(1)
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
    let row = ticket(&service, 1);
    assert_eq!(
        service
            .mutate(
                "ticket",
                &row["id"],
                "update",
                &json!({"user_id":2}),
                &customer(1),
                None
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service.find("ticket", &row["id"], &customer(1)).unwrap()["user_id"],
        1
    );
}

#[test]
fn constraints_reject_invalid_writes_without_partial_rows() {
    let service = setup();
    for input in [
        json!({"name":"balcony","price":2500}),
        json!({"name":"Balcony","price":-1}),
        json!({"name":"Balcony","price":2.5}),
        json!({"name":"Balcony","price":2500,"status":"invented"}),
        json!({"name":"Balcony","price":2500,"id":42}),
        json!({"name":"Balcony","price":2500,"injected":true}),
    ] {
        assert!(service.create("ticket", &input, &customer(1)).is_err());
    }
    assert_eq!(
        service
            .list("ticket", &ListOptions::default(), &customer(1))
            .unwrap()["total"],
        0
    );
    assert!(
        service
            .create("line", &json!({"ticket_id":999}), &AuthContext::system())
            .is_err()
    );
}

#[test]
fn guards_cascades_audit_and_effects_share_a_transaction() {
    let service = setup();
    let row = ticket(&service, 1);
    let line = service
        .create(
            "line",
            &json!({"ticket_id":row["id"]}),
            &AuthContext::system(),
        )
        .unwrap();
    let confirmed = service
        .mutate(
            "ticket",
            &row["id"],
            "confirm",
            &json!({}),
            &customer(1),
            None,
        )
        .unwrap();
    assert_eq!(confirmed["status"], "confirmed");
    assert_eq!(
        service
            .find("line", &line["id"], &AuthContext::system())
            .unwrap()["status"],
        "confirmed"
    );
    assert_eq!(
        service
            .mutate(
                "ticket",
                &row["id"],
                "confirm",
                &json!({}),
                &customer(1),
                None
            )
            .unwrap_err()
            .code,
        "bad_state"
    );
    let count: i64 = service
        .db
        .query_row(
            "SELECT count(*) FROM openb2c_rust_audit WHERE operation = 'confirm'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let count: i64 = service
        .db
        .query_row("SELECT count(*) FROM openb2c_rust_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn failed_cascade_rolls_back_parent_audit_and_outbox() {
    let mut model = schema();
    model
        .operations
        .get_mut("ticket")
        .unwrap()
        .get_mut("confirm")
        .unwrap()
        .cascade[0]
        .set
        .get_mut("status")
        .unwrap()
        .value = Some(json!("invalid"));
    let service = Service::open(model, ":memory:", false).unwrap();
    service
        .create("user", &json!({"name":"Ada"}), &AuthContext::system())
        .unwrap();
    let row = ticket(&service, 1);
    service
        .create(
            "line",
            &json!({"ticket_id":row["id"]}),
            &AuthContext::system(),
        )
        .unwrap();
    assert!(
        service
            .mutate(
                "ticket",
                &row["id"],
                "confirm",
                &json!({}),
                &customer(1),
                None
            )
            .is_err()
    );
    assert_eq!(
        service.find("ticket", &row["id"], &customer(1)).unwrap()["status"],
        "reserved"
    );
    let count: i64 = service
        .db
        .query_row("SELECT count(*) FROM openb2c_rust_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn returned_etags_work_and_stale_writes_fail() {
    let service = setup();
    let row = ticket(&service, 1);
    let tag = etag("ticket", &row);
    assert!(
        service
            .mutate(
                "ticket",
                &row["id"],
                "update",
                &json!({"name":"Stalls"}),
                &customer(1),
                Some(&tag)
            )
            .is_ok()
    );
    assert_eq!(
        service
            .mutate(
                "ticket",
                &row["id"],
                "update",
                &json!({"name":"Circle"}),
                &customer(1),
                Some(&tag)
            )
            .unwrap_err()
            .code,
        "conflict"
    );
}

#[test]
fn secrets_are_absent_from_reads_audits_and_effect_payloads() {
    let service = setup();
    let row = ticket(&service, 1);
    assert!(row.get("secret").is_none());
    service
        .mutate(
            "ticket",
            &row["id"],
            "confirm",
            &json!({}),
            &customer(1),
            None,
        )
        .unwrap();
    let data: String = service
        .db
        .query_row(
            "SELECT after_json FROM openb2c_rust_audit WHERE entity = 'ticket' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!data.contains("Never expose"));
    let data: String = service
        .db
        .query_row(
            "SELECT data_json FROM openb2c_rust_outbox LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!data.contains("Never expose"));
    let options = ListOptions {
        sort: Some("secret".into()),
        ..Default::default()
    };
    assert!(service.list("ticket", &options, &customer(1)).is_err());
}

#[test]
fn untrusted_sort_and_filter_fields_cannot_become_sql() {
    let service = setup();
    ticket(&service, 1);
    let options = ListOptions {
        sort: Some("id; DROP TABLE user;--".into()),
        ..Default::default()
    };
    assert!(service.list("ticket", &options, &customer(1)).is_err());
    let options = ListOptions {
        filter: [("name".into(), json!("' OR 1=1--"))].into(),
        ..Default::default()
    };
    assert_eq!(
        service.list("ticket", &options, &customer(1)).unwrap()["total"],
        0
    );
}

#[test]
fn schema_validation_catches_multiple_assignment_sources_and_dangling_fks() {
    let mut model = schema();
    model
        .operations
        .get_mut("ticket")
        .unwrap()
        .get_mut("confirm")
        .unwrap()
        .set
        .get_mut("status")
        .unwrap()
        .now = Some(true);
    model
        .tables
        .get_mut("line")
        .unwrap()
        .get_mut("ticket_id")
        .unwrap()
        .references = Some("absent(id)".into());
    let diagnostics = openb2c_schema::validate::validate(&model).join("\n");
    assert!(diagnostics.contains("exactly one assignment source"));
    assert!(diagnostics.contains("foreign key target absent(id) does not exist"));
}

#[tokio::test]
async fn http_contract_and_html_escaping() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let app = openb2c_runtime::router(setup(), "{}".into(), false);
    let response = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("Example &lt;Opera&gt;"));
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/tickets")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Balcony","price":2500,"user_id":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let response = app
        .clone()
        .oneshot(Request::get("/api/tickets/1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let tag = response.headers().get("etag").unwrap().clone();
    let response = app
        .clone()
        .oneshot(
            Request::put("/api/tickets/1")
                .header("content-type", "application/json")
                .header("if-match", tag)
                .body(Body::from(r#"{"name":"Stalls"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = app
        .oneshot(
            Request::post("/api/tickets")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn http_denies_anonymous_access_to_owned_records() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let app = openb2c_runtime::router(setup(), "{}".into(), true);
    let response = app
        .oneshot(Request::get("/api/tickets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

#[test]
fn aggregates_preserve_real_values_and_derived_fields_use_stored_facts() {
    let model: Schema = serde_json::from_value(json!({
        "tables": {
            "project":{"id":{"type":"integer","pk":true,"auto":true},"name":{"type":"text","required":true}},
            "entry":{"id":{"type":"integer","pk":true,"auto":true},"project_id":{"type":"integer","references":"project(id)","required":true},"amount":{"type":"real","required":true}}
        },
        "derived":{"project":{
            "summary":{"type":"text","dependencies":[{"table":"project","field":"name"}],"template":"Project: {name}"},
            "total":{"type":"real","expression":{"_t":"agg","op":"sum","rel":{"table":"entry"},"field":"amount"}}
        }}
    })).unwrap();
    let service = Service::open(model, ":memory:", false).unwrap();
    let auth = AuthContext::system();
    service
        .create("project", &json!({"name":"Opera"}), &auth)
        .unwrap();
    for amount in [1.25, 2.5] {
        service
            .create("entry", &json!({"project_id":1,"amount":amount}), &auth)
            .unwrap();
    }
    let row = service.find("project", &json!(1), &auth).unwrap();
    assert_eq!(row["summary"], "Project: Opera");
    assert_eq!(row["total"], 3.75);
    let expr =
        serde_json::from_value(json!({"_t":"agg","op":"count","rel":{"table":"entry"}})).unwrap();
    assert_eq!(service.evaluate("project", &row, &expr).unwrap(), 2);
    assert!(
        service
            .create("project", &json!({"name":"Invalid", "total":12}), &auth)
            .is_err()
    );
}
