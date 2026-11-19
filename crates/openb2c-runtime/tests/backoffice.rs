use openb2c_runtime::{AuthContext, Service};
use openb2c_schema::{Schema, capabilities::entity};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Output},
};
fn evaluated(selection: &str) -> Output {
    Command::new("nix-instantiate")
        .args([
            "--eval",
            "--strict",
            "--json",
            "crates/openb2c-runtime/tests/fixtures/capabilities.nix",
            "--argstr",
            "selection",
            selection,
        ])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap()
}
fn schema(selection: &str) -> Schema {
    let out = evaluated(selection);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
fn service(selection: &str) -> Service {
    Service::open(schema(selection), ":memory:", true).unwrap()
}
fn admin() -> AuthContext {
    AuthContext::system()
}
fn count(s: &Service, cap: &str, role: &str) -> i64 {
    s.db.query_row(
        &format!(
            "SELECT count(*) FROM \"{}\"",
            entity(&s.schema, cap, role).unwrap()
        ),
        [],
        |r| r.get(0),
    )
    .unwrap()
}
fn today() -> String {
    chrono::Utc::now().date_naive().to_string()
}
fn journal(key: &str, date: &str, amount: i64) -> Value {
    json!({"idempotency_key":key,"date":date,"description":"Opening funds","lines":[{"account":"bank","debit":amount,"credit":0},{"account":"equity","debit":0,"credit":amount}]})
}
fn invoice(key: &str, date: &str) -> Value {
    json!({"idempotency_key":key,"customer":"Reader <reader@example.test>","description":"Publication","date":date,"due_date":date,"net_minor":1000,"tax_code":"standard"})
}
fn expense(s: &Service, date: &str) -> Value {
    let d=s.backoffice_document(&json!({"idempotency_key":"receipt","name":"receipt.txt","media_type":"text/plain","content_base64":"UmVjZWlwdA=="}),&admin()).unwrap();
    let supplier=s.backoffice_supplier(&json!({"idempotency_key":"supplier","name":"Print supplier","email":"supplier@example.test"}),&admin()).unwrap();
    s.backoffice_expense(&json!({"idempotency_key":"expense","supplier_id":supplier["id"],"document_id":d["id"],"supplier_reference":"SUP-1","description":"Printing","date":date,"net_minor":400,"tax_code":"standard","recoverable":true}),&admin()).unwrap()
}
#[test]
fn partial_compositions_dependencies_contracts_and_consumer_opt_outs() {
    for (selection, tables) in [
        ("ledger", 10),
        ("billing", 15),
        ("purchasing", 13),
        ("banking", 11),
        ("tax", 10),
    ] {
        let s = service(selection);
        assert_eq!(s.schema.tables.len(), tables);
        assert!(!s.schema.capabilities.contains_key("catalog"));
    }
    for (selection, message) in [
        ("broken_billing", "billing requires accounting"),
        ("broken_purchasing", "purchasing requires documents"),
    ] {
        let result = evaluated(selection);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(message));
    }
    let disabled = schema("office_without_sales");
    assert!(!disabled.capabilities.contains_key("billing_orders"));
    assert!(!disabled.capabilities.contains_key("billing_subscriptions"));
    let mut broken = schema("office");
    broken.capabilities.remove("accounting");
    assert!(!openb2c_schema::validate::validate(&broken).is_empty());
    let mut broken = schema("office");
    let table = entity(&broken, "billing", "invoice").unwrap().to_string();
    broken
        .operations
        .get_mut(&table)
        .unwrap()
        .get_mut("update")
        .unwrap()
        .enabled = true;
    assert!(!openb2c_schema::validate::validate(&broken).is_empty());
    let s = service("office_renamed");
    s.backoffice_invoice(&invoice("renamed", "2026-01-01"), &admin())
        .unwrap();
    assert_eq!(count(&s, "accounting", "journal"), 1);
}
#[test]
fn journals_balance_replay_reverse_and_lock_periods() {
    let s = service("ledger");
    let j = journal("opening", "2026-01-01", 10000);
    let posted = s.backoffice_journal(&j, &admin()).unwrap();
    assert_eq!(posted, s.backoffice_journal(&j, &admin()).unwrap());
    let mut wrong = j.clone();
    wrong["lines"][1]["credit"] = json!(9999);
    assert!(s.backoffice_journal(&wrong, &admin()).is_err());
    wrong["idempotency_key"] = json!("unbalanced");
    assert!(s.backoffice_journal(&wrong, &admin()).is_err());
    assert_eq!(count(&s, "accounting", "journal"), 1);
    s.backoffice_close(&json!({"through_date":"2026-01-31"}), &admin())
        .unwrap();
    assert!(
        s.backoffice_journal(&journal("late", "2026-01-15", 100), &admin())
            .is_err()
    );
    assert_eq!(posted, s.backoffice_journal(&j, &admin()).unwrap());
    let reverse = json!({"date":"2026-02-01","reason":"Correct opening balance"});
    s.backoffice_reverse(
        posted["journal"]["id"].as_i64().unwrap(),
        &reverse,
        &admin(),
    )
    .unwrap();
    s.backoffice_reverse(
        posted["journal"]["id"].as_i64().unwrap(),
        &reverse,
        &admin(),
    )
    .unwrap();
    let report = s
        .backoffice_financial_report(
            &json!({"from":"2026-01-01","through":"2026-02-28"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(report["balanced"], true);
    assert_eq!(report["balance_sheet"]["assets_minor"], 0);
    let mut foreign = journal("eur", "2026-02-01", 100);
    foreign["currency"] = json!("EUR");
    assert!(s.backoffice_journal(&foreign, &admin()).is_err());
}
#[test]
fn invoices_expenses_reports_reconciliation_and_corrections_keep_evidence() {
    let s = service("office");
    let i = s
        .backoffice_invoice(&invoice("sale", "2026-01-01"), &admin())
        .unwrap();
    let id = i["id"].as_i64().unwrap();
    assert_eq!(i["tax_minor"], 200);
    assert_eq!(
        s.backoffice_invoice_view(id, &admin()).unwrap()["outstanding_minor"],
        1200
    );
    s.backoffice_receive(id, &json!({"date":"2026-01-02"}), &admin())
        .unwrap();
    let e = expense(&s, "2026-01-03");
    s.backoffice_pay_expense(
        e["id"].as_i64().unwrap(),
        &json!({"date":"2026-01-04"}),
        &admin(),
    )
    .unwrap();
    let report = s
        .backoffice_financial_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(report["profit_and_loss"]["profit_minor"], 600);
    assert_eq!(report["balanced"], true);
    let vat = s
        .backoffice_vat_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(vat["boxes"]["1_vat_due_sales_minor"], 200);
    assert_eq!(vat["boxes"]["4_vat_reclaimed_minor"], 80);
    assert_eq!(vat["boxes"]["5_net_vat_minor"], 120);
    assert_eq!(vat["boxes"]["6_sales_whole_pounds"], 10);
    assert_eq!(vat["blockers"], json!([]));
    assert_eq!(vat["filing_supported"], false);
    let bank_line:i64=s.db.query_row("SELECT l.id FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='bank' AND credit=480",[],|r|r.get(0)).unwrap();
    let b=s.backoffice_bank_line(&json!({"idempotency_key":"bank-1","account":"bank","date":"2026-01-05","amount_minor":-480,"description":"Supplier transfer"}),&admin()).unwrap();
    let bid = b["id"].as_i64().unwrap();
    assert!(
        s.backoffice_match(bid, &json!({"journal_line_id":bank_line+100}), &admin())
            .is_err()
    );
    s.backoffice_match(bid, &json!({"journal_line_id":bank_line}), &admin())
        .unwrap();
    s.backoffice_match(bid, &json!({"journal_line_id":bank_line}), &admin())
        .unwrap();
    let b2=s.backoffice_bank_line(&json!({"idempotency_key":"bank-2","account":"bank","date":"2026-01-05","amount_minor":-480,"description":"Duplicate amount"}),&admin()).unwrap();
    assert!(
        s.backoffice_match(
            b2["id"].as_i64().unwrap(),
            &json!({"journal_line_id":bank_line}),
            &admin()
        )
        .is_err()
    );
    assert!(
        s.backoffice_void_expense(
            e["id"].as_i64().unwrap(),
            &json!({"date":"2026-02-01","reason":"Cannot void a paid expense"}),
            &admin()
        )
        .is_err()
    );
    let credit = json!({"date":"2026-02-01","reason":"Refunded reader"});
    s.backoffice_credit(id, &credit, &admin()).unwrap();
    s.backoffice_credit(id, &credit, &admin()).unwrap();
    let jan = s
        .backoffice_vat_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(vat, jan);
    let feb = s
        .backoffice_vat_report(
            &json!({"from":"2026-02-01","through":"2026-02-28"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(feb["boxes"]["1_vat_due_sales_minor"], -200);
    assert_eq!(count(&s, "documents", "document"), 1);
    assert_eq!(count(&s, "billing", "credit"), 1);
}
#[test]
fn subscription_revenue_is_deferred_recognised_once_and_refunded() {
    let s = service("office");
    let mut i = invoice("membership", "2026-01-01");
    i["net_minor"] = json!(3100);
    i["period_start"] = json!("2026-01-01");
    i["period_end"] = json!("2026-02-01");
    let row = s.backoffice_invoice(&i, &admin()).unwrap();
    let report = s
        .backoffice_financial_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(report["profit_and_loss"]["income_minor"], 0);
    assert_eq!(
        s.backoffice_recognize(&json!({"through_date":"2026-01-15"}), &admin())
            .unwrap()["recognized_minor"],
        1500
    );
    assert_eq!(
        s.backoffice_recognize(&json!({"through_date":"2026-01-15"}), &admin())
            .unwrap()["recognized_minor"],
        0
    );
    s.backoffice_credit(
        row["id"].as_i64().unwrap(),
        &json!({"date":"2026-01-20","reason":"Service refunded"}),
        &admin(),
    )
    .unwrap();
    assert_eq!(
        s.backoffice_recognize(&json!({"through_date":"2026-01-31"}), &admin())
            .unwrap()["recognized_minor"],
        0
    );
    let report = s
        .backoffice_financial_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(report["profit_and_loss"]["income_minor"], 0);
    assert_eq!(report["balanced"], true);
}
#[test]
fn sales_consumers_account_atomically_and_replays_do_not_duplicate() {
    let s = service("office");
    let id = s
        .ensure_customer("reader@example.test", &AuthContext::system())
        .unwrap();
    let customer = AuthContext {
        user_id: Some(id),
        scopes: openb2c_schema::capabilities::customer_scopes(&s.schema),
    };
    let order = s
        .b2c_create_order(
            &json!({"idempotency_key":"order","items":[{"price_id":1,"quantity":2}]}),
            &customer,
        )
        .unwrap();
    let p = s
        .b2c_order_payment(
            order["order"]["id"].as_i64().unwrap(),
            &json!({"idempotency_key":"pay"}),
            &customer,
        )
        .unwrap();
    let pid = p["id"].as_i64().unwrap();
    let success = json!({"event_id":"paid","outcome":"succeeded"});
    s.b2c_simulate_payment(pid, &success, &admin()).unwrap();
    s.b2c_simulate_payment(pid, &success, &admin()).unwrap();
    assert_eq!(count(&s, "billing", "invoice"), 1);
    assert_eq!(count(&s, "billing", "receipt"), 1);
    assert_eq!(count(&s, "accounting", "journal"), 2);
    assert!(
        s.backoffice_credit(
            1,
            &json!({"date":today(),"reason":"Bypass payment"}),
            &admin()
        )
        .is_err()
    );
    s.b2c_simulate_payment(
        pid,
        &json!({"event_id":"refund","outcome":"refunded"}),
        &admin(),
    )
    .unwrap();
    assert_eq!(count(&s, "billing", "credit"), 1);
    let sub = s
        .b2c_create_subscription(
            &json!({"idempotency_key":"membership","price_id":2}),
            &customer,
        )
        .unwrap();
    s.b2c_simulate_payment(
        sub["payment"]["id"].as_i64().unwrap(),
        &json!({"event_id":"membership-paid","outcome":"succeeded"}),
        &admin(),
    )
    .unwrap();
    let invoice = s.backoffice_invoice_view(2, &admin()).unwrap();
    assert!(invoice["period_start"].is_string());
    assert_eq!(invoice["recognized_minor"], 0);
    let journals = count(&s, "accounting", "journal");
    // A ledger failure must roll back payment state, invoice, entitlement and event receipt.
    let sub = s
        .b2c_create_subscription(&json!({"idempotency_key":"broken","price_id":2}), &customer)
        .unwrap();
    s.db.execute_batch("CREATE TRIGGER fail_post BEFORE INSERT ON journal BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(
        s.b2c_simulate_payment(
            sub["payment"]["id"].as_i64().unwrap(),
            &json!({"event_id":"broken-paid","outcome":"succeeded"}),
            &admin()
        )
        .is_err()
    );
    assert_eq!(
        s.b2c_payment(sub["payment"]["id"].as_i64().unwrap(), &customer)
            .unwrap()["status"],
        "pending"
    );
    assert_eq!(count(&s, "accounting", "journal"), journals);
    assert_eq!(count(&s, "billing", "invoice"), 2);
}
#[test]
fn documents_expenses_and_vat_reject_unsafe_or_incomplete_operations() {
    let s = service("office");
    assert!(s.backoffice_document(&json!({"idempotency_key":"bad","name":"bad","media_type":"text/plain","content_base64":"not base64"}),&admin()).is_err());
    let mut i = invoice("unknown", "2026-01-01");
    i["tax_code"] = json!("unassessed");
    s.backoffice_invoice(&i, &admin()).unwrap();
    s.backoffice_journal(&json!({"idempotency_key":"adjustment","date":"2026-01-02","description":"Manual tax adjustment","lines":[{"account":"bank","debit":100,"credit":0},{"account":"tax_output","debit":0,"credit":100}]}),&admin()).unwrap();
    let report = s
        .backoffice_vat_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(report["blockers"].as_array().unwrap().len(), 2);
    let e = expense(&s, "2026-01-03");
    let void = json!({"date":"2026-02-01","reason":"Invoice cancelled by supplier"});
    s.backoffice_void_expense(e["id"].as_i64().unwrap(), &void, &admin())
        .unwrap();
    let report = s
        .backoffice_vat_report(
            &json!({"from":"2026-02-01","through":"2026-02-28"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(report["boxes"]["4_vat_reclaimed_minor"], -80);
    let before = count(&s, "accounting", "journal");
    let mut duplicate = invoice("duplicate", "2026-01-01");
    duplicate["net_minor"] = json!(i64::MAX);
    assert!(s.backoffice_invoice(&duplicate, &admin()).is_err());
    assert_eq!(count(&s, "accounting", "journal"), before);
}

async fn http(
    app: &axum::Router,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (u16, Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(key) = key {
        req = req.header("authorization", format!("Bearer {key}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn rest_requires_explicit_internal_permissions_and_blocks_workflow_bypasses() {
    let s = service("office");
    let customer = s
        .ensure_customer("reader@example.test", &AuthContext::system())
        .unwrap();
    let staff = s.ensure_user("staff@example.test").unwrap();
    let mut keys = vec![];
    for (user,scopes) in [(customer,s.self_scopes().join(",")),(staff,"accounting.read,billing.read,tax.read".into()),(staff,"accounting.read,accounting.write,accounting.close,billing.read,billing.write,documents.read,documents.write,purchasing.read,purchasing.write,banking.read,banking.write,tax.read".into())] {
        keys.push(s.create_api_key(&json!({"user_id":user,"name":"Test","scopes":scopes}),&admin()).unwrap()["key"].as_str().unwrap().to_owned());
    }
    let doc = openb2c_schema::contract::openapi(&s.schema);
    assert_eq!(
        doc["paths"]["/backoffice/reports/vat"]["post"]["x-openb2c-scope"],
        "tax.read"
    );
    assert!(doc["paths"].get("/api/journals").is_none());
    let app = openb2c_runtime::router(s, doc.to_string(), true);
    assert_eq!(
        http(&app, "GET", "/backoffice/accounts", None, json!({}))
            .await
            .0,
        401
    );
    for path in [
        "/backoffice/accounts",
        "/backoffice/invoices",
        "/backoffice/expenses",
        "/backoffice/bank-lines",
        "/backoffice/documents/1",
    ] {
        assert_eq!(
            http(&app, "GET", path, Some(&keys[0]), json!({})).await.0,
            403
        );
    }
    assert_eq!(
        http(
            &app,
            "GET",
            "/backoffice/accounts",
            Some(&keys[1]),
            json!({})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http(
            &app,
            "POST",
            "/backoffice/journals",
            Some(&keys[1]),
            journal("denied", "2026-01-01", 100)
        )
        .await
        .0,
        403
    );
    let (status, created_invoice) = http(
        &app,
        "POST",
        "/backoffice/invoices",
        Some(&keys[2]),
        invoice("http", "2026-01-01"),
    )
    .await;
    assert_eq!(status, 200);
    let path = format!("/backoffice/invoices/{}/receive", created_invoice["id"]);
    assert_eq!(
        http(
            &app,
            "POST",
            &path,
            Some(&keys[2]),
            json!({"date":"2025-12-31"})
        )
        .await
        .0,
        422
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &path,
            Some(&keys[2]),
            json!({"date":"2026-01-02"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http(
            &app,
            "PATCH",
            "/api/invoices/1",
            Some(&keys[2]),
            json!({"tax_minor":0})
        )
        .await
        .0,
        404
    );
    let path = format!(
        "/backoffice/journals/{}/reverse",
        created_invoice["journal_id"]
    );
    assert_eq!(
        http(
            &app,
            "POST",
            &path,
            Some(&keys[2]),
            json!({"date":"2026-01-02","reason":"Bypass invoice workflow"})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        http(
            &app,
            "POST",
            "/backoffice/periods/close",
            Some(&keys[2]),
            json!({"through_date":"2026-01-31"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http(
            &app,
            "POST",
            "/backoffice/invoices",
            Some(&keys[2]),
            invoice_json("closed")
        )
        .await
        .0,
        409
    );
    let (status, report) = http(
        &app,
        "POST",
        "/backoffice/reports/vat",
        Some(&keys[1]),
        json!({"from":"2026-01-01","through":"2026-01-31"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(report["boxes"]["1_vat_due_sales_minor"], 200);
    let s = service("ledger");
    let doc = openb2c_schema::contract::openapi(&s.schema);
    let app = openb2c_runtime::router(s, doc.to_string(), false);
    assert_eq!(
        http(&app, "GET", "/backoffice/invoices", None, json!({}))
            .await
            .0,
        404
    );
}
fn invoice_json(key: &str) -> Value {
    invoice(key, "2026-01-01")
}

#[test]
fn inclusive_penny_prices_and_closed_period_settlement_are_safe() {
    let s = service("office");
    let id = s
        .ensure_customer("reader@example.test", &AuthContext::system())
        .unwrap();
    let customer = AuthContext {
        user_id: Some(id),
        scopes: s.self_scopes(),
    };
    s.db.execute("UPDATE price SET amount_minor=3 WHERE id=1", [])
        .unwrap();
    let order = s
        .b2c_create_order(
            &json!({"idempotency_key":"penny","items":[{"price_id":1,"quantity":1}]}),
            &customer,
        )
        .unwrap();
    let payment = s
        .b2c_order_payment(
            order["order"]["id"].as_i64().unwrap(),
            &json!({"idempotency_key":"penny"}),
            &customer,
        )
        .unwrap();
    let pid = payment["id"].as_i64().unwrap();
    s.b2c_simulate_payment(
        pid,
        &json!({"event_id":"penny","outcome":"succeeded"}),
        &admin(),
    )
    .unwrap();
    let i = s.backoffice_invoice_view(1, &admin()).unwrap();
    assert_eq!(i["net_minor"], 2);
    assert_eq!(i["tax_minor"], 1);
    s.backoffice_close(&json!({"through_date":today()}), &admin())
        .unwrap();
    let journals = count(&s, "accounting", "journal");
    s.b2c_simulate_payment(
        pid,
        &json!({"event_id":"closed-refund","outcome":"refunded"}),
        &admin(),
    )
    .unwrap();
    assert_eq!(s.b2c_payment(pid, &customer).unwrap()["status"], "refunded");
    assert_eq!(count(&s, "billing", "credit"), 0);
    assert_eq!(count(&s, "accounting", "journal"), journals);
}

#[test]
fn credit_notes_preserve_cash_until_a_separately_authorized_refund() {
    let s = service("office");
    let i = s
        .backoffice_invoice(&invoice("credit-only", "2026-01-01"), &admin())
        .unwrap();
    let id = i["id"].as_i64().unwrap();
    assert!(
        s.backoffice_refund(
            id,
            &json!({"date":"2026-01-02","reason":"No credit yet"}),
            &admin()
        )
        .is_err()
    );
    s.backoffice_receive(id, &json!({"date":"2026-01-02"}), &admin())
        .unwrap();
    s.backoffice_credit(
        id,
        &json!({"date":"2026-01-03","reason":"Sale cancelled"}),
        &admin(),
    )
    .unwrap();
    let i = s.backoffice_invoice_view(id, &admin()).unwrap();
    assert_eq!(i["refund_due_minor"], 1200);
    assert!(i["refund"].is_null());
    let report = s
        .backoffice_financial_report(
            &json!({"from":"2026-01-01","through":"2026-01-31"}),
            &admin(),
        )
        .unwrap();
    assert_eq!(
        report["trial_balance"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["code"] == "clearing")
            .unwrap()["balance_minor"],
        1200
    );
    let writer = AuthContext {
        user_id: Some(1),
        scopes: vec!["billing.write".into()],
    };
    assert_eq!(
        s.backoffice_refund(
            id,
            &json!({"date":"2026-01-04","reason":"Confirmed refund"}),
            &writer
        )
        .unwrap_err()
        .code,
        "forbidden"
    );
    let refund = json!({"date":"2026-01-04","reason":"Confirmed refund"});
    s.backoffice_refund(id, &refund, &admin()).unwrap();
    s.backoffice_refund(id, &refund, &admin()).unwrap();
    assert_eq!(
        s.backoffice_invoice_view(id, &admin()).unwrap()["refund_due_minor"],
        0
    );
    assert_eq!(count(&s, "billing", "refund"), 1);
}
