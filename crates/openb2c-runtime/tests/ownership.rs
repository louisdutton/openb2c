//! A business must survive configuration changes, corrections, restore and credential loss.
use openb2c_runtime::{AuthContext, Service, operations, transitions};
use openb2c_schema::{Schema, capabilities::entity, sql::quote};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const PASSWORD: &str = "violet-river-sunrise-47";
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "openb2c-ownership-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn schema(selection: &str) -> Schema {
    let output = Command::new("nix-instantiate")
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
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn open(model: Schema, temp: &Temp) -> Service {
    Service::open_at(
        model,
        temp.path("app.sqlite"),
        false,
        &temp.path("migrations"),
    )
    .unwrap()
}
fn admin() -> AuthContext {
    AuthContext::system()
}
fn at(day: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(&format!("{day}T12:00:00Z"))
        .unwrap()
        .with_timezone(&chrono::Utc)
}
fn id(row: &Value) -> i64 {
    row["id"].as_i64().unwrap()
}
fn approve(s: &Service, new: &Schema, dir: &Path) -> Value {
    let p = transitions::plan_at(&s.db, new, Some(dir)).unwrap();
    transitions::approve_at(
        &s.db,
        new,
        Some(dir),
        p["plan_hash"].as_str().unwrap(),
        "Reviewed isolated business upgrade",
    )
    .unwrap()
}
fn order(s: &Service, who: &AuthContext, key: &str, day: &str) -> Value {
    let o = s
        .b2c_create_order_at(
            &json!({"idempotency_key":key,"items":[{"price_id":1,"quantity":1}]}),
            who,
            at(day),
        )
        .unwrap();
    s.b2c_order_payment_at(
        id(&o["order"]),
        &json!({"idempotency_key":key}),
        who,
        at(day),
    )
    .unwrap()
}
fn settle(s: &Service, p: &Value, key: &str, outcome: &str, day: &str) -> Value {
    s.b2c_simulate_payment_at(
        id(p),
        &json!({"event_id":key,"outcome":outcome}),
        &admin(),
        at(day),
    )
    .unwrap()
}
fn range(from: &str, through: &str) -> Value {
    json!({"from":from,"through":through})
}
#[test]
fn staff_identity_customer_membership_and_owner_recovery_are_independent() {
    let temp = Temp::new();
    let s = open(schema("ledger"), &temp);
    assert!(!s.schema.capabilities.contains_key("customers"));
    let doc = openb2c_schema::contract::openapi(&s.schema);
    assert!(doc["paths"].get("/auth/signup").is_none());
    assert!(doc["paths"]["/auth/login"]["post"].is_object());
    s.provision_staff(
        "bookkeeper@example.test",
        PASSWORD,
        "accounting.read,accounting.write",
        &admin(),
    )
    .unwrap();
    let session = s
        .password_login(
            &json!({"email":"bookkeeper@example.test","password":PASSWORD}),
            "peer",
        )
        .unwrap();
    let token = session["access_token"].as_str().unwrap();
    let who = s.authenticate(token).unwrap();
    assert!(who.has_scope("accounting.read"));
    assert!(!who.has_scope("orders.create"));
    assert!(!who.has_scope("*"));
    assert!(
        s.provision_staff("other@example.test", PASSWORD, "*", &who)
            .is_err()
    );
    let key = s
        .create_api_key(
            &json!({"user_id":who.user_id,"name":"Automation","scopes":"accounting.read"}),
            &admin(),
        )
        .unwrap();
    s.recover_password("bookkeeper@example.test", "new-forest-horizon-91", &admin())
        .unwrap();
    assert!(s.authenticate(token).is_err());
    assert!(s.authenticate(key["key"].as_str().unwrap()).is_err());
    assert!(
        s.password_login(
            &json!({"email":"bookkeeper@example.test","password":PASSWORD}),
            "peer"
        )
        .is_err()
    );
    let next = s
        .password_login(
            &json!({"email":"bookkeeper@example.test","password":"new-forest-horizon-91"}),
            "peer",
        )
        .unwrap();
    s.disable_staff("bookkeeper@example.test", &admin())
        .unwrap();
    assert!(
        s.authenticate(next["access_token"].as_str().unwrap())
            .is_err()
    );
    assert!(
        s.password_login(
            &json!({"email":"bookkeeper@example.test","password":"new-forest-horizon-91"}),
            "peer"
        )
        .is_err()
    );
    let before: i64 =
        s.db.query_row("SELECT count(*) FROM user", [], |r| r.get(0))
            .unwrap();
    assert!(
        s.provision_staff("invalid@example.test", "short", "accounting.read", &admin())
            .is_err()
    );
    assert_eq!(
        s.db.query_row("SELECT count(*) FROM user", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        before
    );
}
#[test]
fn populated_capability_changes_require_fresh_review_and_resolve_obligations() {
    let temp = Temp::new();
    let old = schema("office_no_billing");
    let s = open(old, &temp);
    let uid = s.ensure_customer("reader@example.test", &admin()).unwrap();
    let who = AuthContext {
        user_id: Some(uid),
        scopes: s.self_scopes(),
    };
    let p = order(&s, &who, "before-billing", "2026-01-05");
    settle(&s, &p, "before-paid", "succeeded", "2026-01-05");
    let new = schema("office");
    assert!(
        Service::open_at(
            new.clone(),
            temp.path("app.sqlite"),
            false,
            &temp.path("migrations")
        )
        .is_err()
    );
    let plan = approve(&s, &new, &temp.path("migrations"));
    s.ensure_user("changed-after-review@example.test").unwrap();
    assert_ne!(
        plan["plan_hash"],
        transitions::plan_at(&s.db, &new, Some(&temp.path("migrations"))).unwrap()["plan_hash"]
    );
    assert!(
        Service::open_at(
            new.clone(),
            temp.path("app.sqlite"),
            false,
            &temp.path("migrations")
        )
        .is_err()
    );
    approve(&s, &new, &temp.path("migrations"));
    let upgraded = open(new.clone(), &temp);
    assert!(s.assert_current_model().is_err());
    let invoices = upgraded
        .backoffice_list("billing", "invoice", &admin())
        .unwrap();
    assert_eq!(invoices["items"].as_array().unwrap().len(), 1);
    assert_eq!(invoices["items"][0]["tax_code"], "unassessed");
    assert_eq!(
        upgraded.b2c_payment(id(&p), &who).unwrap()["status"],
        "succeeded"
    );
    let mut policy = new.clone();
    policy.capabilities.get_mut("billing").unwrap().settings["defaultTaxCode"] = json!("zero");
    approve(&upgraded, &policy, &temp.path("migrations"));
    let changed = open(policy, &temp);
    assert_eq!(
        changed.backoffice_invoice_view(1, &admin()).unwrap()["tax_code"],
        "unassessed"
    );
    let next = order(&changed, &who, "new-policy", "2026-02-01");
    settle(
        &changed,
        &next,
        "new-policy-paid",
        "succeeded",
        "2026-02-01",
    );
    assert_eq!(
        changed.backoffice_invoice_view(2, &admin()).unwrap()["tax_code"],
        "zero"
    );
    // Retire an integration deliberately. Its history stays available; its provider still works.
    let retired = schema("office_without_sales");
    let retirement_plan =
        transitions::plan_at(&changed.db, &retired, Some(&temp.path("migrations"))).unwrap();
    assert!(retirement_plan["blockers"].as_array().unwrap().is_empty());
    approve(&changed, &retired, &temp.path("migrations"));
    let retired_service = open(retired, &temp);
    assert_eq!(
        retired_service
            .backoffice_list("billing", "invoice", &admin())
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    settle(
        &retired_service,
        &p,
        "old-sale-refund-after-opt-out",
        "refunded",
        "2026-02-02",
    );
    assert!(
        retired_service
            .backoffice_invoice_view(1, &admin())
            .unwrap()["refund"]
            .is_object()
    );
    let pending = order(&retired_service, &who, "unsettled", "2026-02-02");
    let without_payments = schema("orders");
    let blocked = transitions::plan_at(
        &retired_service.db,
        &without_payments,
        Some(&temp.path("migrations")),
    )
    .unwrap();
    assert!(!blocked["blockers"].as_array().unwrap().is_empty());
    assert!(
        transitions::approve_at(
            &retired_service.db,
            &without_payments,
            Some(&temp.path("migrations")),
            blocked["plan_hash"].as_str().unwrap(),
            "Cannot discard settlement"
        )
        .is_err()
    );
    assert_eq!(
        retired_service.b2c_payment(id(&pending), &who).unwrap()["status"],
        "pending"
    );
}
#[test]
fn backup_restore_preserves_wal_data_documents_migrations_and_financial_history() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let model = schema("office");
    let s = open(model.clone(), &temp);
    let owner = s.ensure_customer("reader@example.test", &admin()).unwrap();
    let who = AuthContext {
        user_id: Some(owner),
        scopes: s.self_scopes(),
    };
    let p = order(&s, &who, "backup-sale", "2026-01-05");
    settle(&s, &p, "paid", "succeeded", "2026-01-05");
    let doc=s.backoffice_document(&json!({"idempotency_key":"receipt","name":"receipt.txt","media_type":"text/plain","content_base64":"UmVjZWlwdA=="}),&admin()).unwrap();
    fs::create_dir(temp.path("migrations")).unwrap();
    fs::write(temp.path("migrations/001_owner_note.sql"),"CREATE TABLE owner_note(id INTEGER PRIMARY KEY,note TEXT); INSERT INTO owner_note VALUES (1,'Owned migration preserved');").unwrap();
    approve(&s, &model, &temp.path("migrations"));
    drop(s);
    let s = open(model.clone(), &temp);
    let before = s
        .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
        .unwrap();
    let backup = temp.path("snapshot.sqlite");
    let report = operations::backup(&s.db, &backup, &temp.path("migrations")).unwrap();
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(operations::backup(&s.db, &backup, &temp.path("migrations")).is_err());
    assert!(
        operations::restore(
            &backup,
            "wrong",
            &temp.path("bad.sqlite"),
            &temp.path("bad-migrations")
        )
        .is_err()
    );
    assert!(!temp.path("bad.sqlite").exists());
    let restored = temp.path("restored.sqlite");
    let migrations = temp.path("restored-migrations");
    operations::restore(
        &backup,
        report["sha256"].as_str().unwrap(),
        &restored,
        &migrations,
    )
    .unwrap();
    let restored_service = Service::open_at(model.clone(), &restored, false, &migrations).unwrap();
    assert_eq!(
        before,
        restored_service
            .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()
    );
    assert_eq!(
        doc,
        restored_service
            .backoffice_record("documents", "document", id(&doc), &admin())
            .unwrap()
    );
    assert_eq!(
        restored_service
            .db
            .query_row("SELECT note FROM owner_note WHERE id=1", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "Owned migration preserved"
    );
    assert_eq!(
        fs::read(migrations.join("001_owner_note.sql")).unwrap(),
        fs::read(temp.path("migrations/001_owner_note.sql")).unwrap()
    );
    assert_eq!(
        operations::doctor(&restored_service.db, &model).unwrap()["integrity"],
        "ok"
    );
    assert!(
        operations::restore(
            &backup,
            report["sha256"].as_str().unwrap(),
            &restored,
            &temp.path("another-migrations")
        )
        .is_err()
    );
}
#[test]
fn role_changes_require_data_preserving_sql_and_failed_upgrades_roll_back_everything() {
    let temp = Temp::new();
    let old = schema("ledger");
    let s = open(old, &temp);
    let account_count: i64 =
        s.db.query_row("SELECT count(*) FROM ledger_account", [], |r| r.get(0))
            .unwrap();
    s.backoffice_journal(&json!({"idempotency_key":"funds","date":"2026-01-01","description":"Owner funding","lines":[{"account":"bank","debit":10000,"credit":0},{"account":"equity","debit":0,"credit":10000}]}),&admin()).unwrap();
    let new = schema("office_renamed");
    approve(&s, &new, &temp.path("migrations"));
    assert!(
        Service::open_at(
            new.clone(),
            temp.path("app.sqlite"),
            false,
            &temp.path("migrations")
        )
        .is_err()
    );
    assert_eq!(
        transitions::stored_model(&s.db)
            .unwrap()
            .unwrap()
            .capabilities["accounting"]
            .entities["account"],
        "ledger_account"
    );
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='chart'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    fs::create_dir(temp.path("migrations")).unwrap();
    let mut sql = openb2c_schema::sql::generate(&new).unwrap();
    for role in ["account", "journal", "line", "period"] {
        let from = entity(&s.schema, "accounting", role).unwrap();
        let to = entity(&new, "accounting", role).unwrap();
        let cols = s.schema.tables[from]
            .keys()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(
            "INSERT INTO {} ({cols}) SELECT {cols} FROM {};\n",
            quote(to),
            quote(from)
        ));
    }
    fs::write(temp.path("migrations/001_preserve_roles.sql"), &sql).unwrap();
    approve(&s, &new, &temp.path("migrations"));
    // A changed migration invalidates approval before any SQL can run.
    fs::write(
        temp.path("migrations/001_preserve_roles.sql"),
        format!("{sql}\n-- new reviewed content"),
    )
    .unwrap();
    assert!(
        Service::open_at(
            new.clone(),
            temp.path("app.sqlite"),
            false,
            &temp.path("migrations")
        )
        .is_err()
    );
    approve(&s, &new, &temp.path("migrations"));
    let upgraded = open(new, &temp);
    assert_eq!(
        upgraded
            .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()["balance_sheet"]["assets_minor"],
        10000
    );
    assert_eq!(
        upgraded
            .db
            .query_row("SELECT count(*) FROM ledger_account", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        account_count
    );
}

#[test]
fn a_complete_trading_period_survives_late_events_corrections_and_restore() {
    let temp = Temp::new();
    let model = schema("office");
    let s = open(model.clone(), &temp);
    let uid = s.ensure_customer("reader@example.test", &admin()).unwrap();
    let who = AuthContext {
        user_id: Some(uid),
        scopes: s.self_scopes(),
    };
    s.backoffice_journal(&json!({"idempotency_key":"capital","date":"2026-01-01","description":"Owner funding","lines":[{"account":"bank","debit":100000,"credit":0},{"account":"equity","debit":0,"credit":100000}]}),&admin()).unwrap();
    let book = order(&s, &who, "book", "2026-01-05");
    settle(&s, &book, "book-paid", "succeeded", "2026-01-05");
    let subscription = s
        .b2c_create_subscription_at(
            &json!({"idempotency_key":"membership","price_id":2}),
            &who,
            at("2026-01-10"),
        )
        .unwrap();
    settle(
        &s,
        &subscription["payment"],
        "membership-paid",
        "succeeded",
        "2026-01-10",
    );
    assert_eq!(
        s.b2c_entitlements_at(&who, at("2026-01-12")).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let document=s.backoffice_document(&json!({"idempotency_key":"print-receipt","name":"receipt.txt","media_type":"text/plain","content_base64":"UmVjZWlwdA=="}),&admin()).unwrap();
    let supplier = s
        .backoffice_supplier(
            &json!({"idempotency_key":"printer","name":"Printer","email":"printer@example.test"}),
            &admin(),
        )
        .unwrap();
    let expense=s.backoffice_expense(&json!({"idempotency_key":"printing","supplier_id":id(&supplier),"document_id":id(&document),"supplier_reference":"PRINT-1","description":"Printing","date":"2026-01-15","net_minor":400,"tax_code":"standard","recoverable":true}),&admin()).unwrap();
    let paid = s
        .backoffice_pay_expense(id(&expense), &json!({"date":"2026-01-16"}), &admin())
        .unwrap();
    let journal = s
        .backoffice_journal_view(paid["payment_journal_id"].as_i64().unwrap(), &admin())
        .unwrap();
    let ledger_line = journal["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["account"] == "bank")
        .unwrap();
    let statement=s.backoffice_bank_line(&json!({"idempotency_key":"bank-expense","account":"bank","date":"2026-01-17","amount_minor":-480,"description":"Printer transfer"}),&admin()).unwrap();
    s.backoffice_match(
        id(&statement),
        &json!({"journal_line_id":id(ledger_line)}),
        &admin(),
    )
    .unwrap();
    // Correct an actually refunded sale, rather than treating a credit as a cash movement.
    settle(&s, &book, "book-refunded", "refunded", "2026-01-20");
    settle(&s, &book, "book-refunded", "refunded", "2026-01-20");
    assert_eq!(
        s.backoffice_invoice_view(1, &admin()).unwrap()["refund_due_minor"],
        0
    );
    assert_eq!(
        s.b2c_entitlements_at(&who, at("2026-01-21")).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let recognized = s
        .backoffice_recognize(&json!({"through_date":"2026-01-31"}), &admin())
        .unwrap();
    assert_eq!(recognized["recognized_minor"], 295);
    assert_eq!(
        s.backoffice_recognize(&json!({"through_date":"2026-01-31"}), &admin())
            .unwrap()["recognized_minor"],
        0
    );
    let late = order(&s, &who, "late-settlement", "2026-01-29");
    s.backoffice_close(&json!({"through_date":"2026-01-31"}), &admin())
        .unwrap();
    let january = s
        .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
        .unwrap();
    assert_eq!(january["balanced"], true);
    assert_eq!(january["profit_and_loss"]["income_minor"], 295);
    assert_eq!(january["profit_and_loss"]["expenses_minor"], 400);
    let january_vat = s
        .backoffice_vat_report(&range("2026-01-01", "2026-01-31"), &admin())
        .unwrap();
    assert_eq!(january_vat["boxes"]["1_vat_due_sales_minor"], 83);
    assert_eq!(january_vat["boxes"]["4_vat_reclaimed_minor"], 80);
    // A late provider fact is accepted and grants the purchase, while the closed ledger stays intact.
    settle(&s, &late, "late-paid", "succeeded", "2026-01-30");
    assert_eq!(
        s.b2c_payment(id(&late), &who).unwrap()["status"],
        "succeeded"
    );
    assert_eq!(
        s.backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()["unposted_billing_events"],
        1
    );
    assert!(
        s.backoffice_post_billing_events(&json!({"date":"2026-01-31"}), &admin())
            .is_err()
    );
    assert_eq!(
        s.backoffice_post_billing_events(&json!({"date":"2026-02-05"}), &admin())
            .unwrap()["posted"],
        1
    );
    assert_eq!(
        s.backoffice_post_billing_events(&json!({"date":"2026-02-05"}), &admin())
            .unwrap()["posted"],
        0
    );
    let late_invoice = s.backoffice_invoice_view(3, &admin()).unwrap();
    assert_eq!(late_invoice["date"], "2026-01-30");
    assert_eq!(late_invoice["posting_date"], "2026-02-05");
    let late_expense=s.backoffice_expense(&json!({"idempotency_key":"late-print","supplier_id":id(&supplier),"document_id":id(&document),"supplier_reference":"PRINT-2","description":"Late supplier invoice","date":"2026-01-31","posting_date":"2026-02-06","net_minor":100,"tax_code":"standard","recoverable":true}),&admin()).unwrap();
    assert_eq!(late_expense["posting_date"], "2026-02-06");
    assert_eq!(
        january,
        s.backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()
    );
    assert_eq!(
        january_vat["boxes"],
        s.backoffice_vat_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()["boxes"]
    );
    let renewal = s.b2c_renew_due_at(&admin(), at("2026-02-10")).unwrap();
    let renewal = &renewal["payments"][0];
    settle(&s, renewal, "renewal-failed", "failed", "2026-02-10");
    assert_eq!(
        s.b2c_entitlements_at(&who, at("2026-02-11")).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let retry = s
        .b2c_retry_subscription_at(
            id(&subscription["subscription"]),
            &json!({"idempotency_key":"retry"}),
            &who,
            at("2026-02-11"),
        )
        .unwrap();
    settle(
        &s,
        &retry["payment"],
        "renewal-retried",
        "succeeded",
        "2026-02-11",
    );
    assert_eq!(
        s.b2c_entitlements_at(&who, at("2026-02-12")).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let february = s
        .backoffice_financial_report(&range("2026-02-01", "2026-02-28"), &admin())
        .unwrap();
    assert_eq!(february["balanced"], true);
    let backup =
        operations::backup(&s.db, &temp.path("period.sqlite"), &temp.path("migrations")).unwrap();
    operations::restore(
        &temp.path("period.sqlite"),
        backup["sha256"].as_str().unwrap(),
        &temp.path("restored.sqlite"),
        &temp.path("restored-migrations"),
    )
    .unwrap();
    let restored = Service::open_at(
        model,
        temp.path("restored.sqlite"),
        false,
        &temp.path("restored-migrations"),
    )
    .unwrap();
    assert_eq!(
        january,
        restored
            .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()
    );
    assert_eq!(
        february,
        restored
            .backoffice_financial_report(&range("2026-02-01", "2026-02-28"), &admin())
            .unwrap()
    );
}
#[test]
fn legacy_customer_owned_accounts_upgrade_without_losing_credentials_or_membership() {
    let temp = Temp::new();
    let new = schema("full");
    let mut old = new.clone();
    old.capabilities.remove("accounts");
    old.tables.remove("staff_member");
    old.operations.remove("staff_member");
    old.tables.remove("customer_profile");
    old.operations.remove("customer_profile");
    let customers = old.capabilities.get_mut("customers").unwrap();
    customers.version = 1;
    customers.requires.clear();
    customers.entities = std::collections::BTreeMap::from([
        ("customer".into(), "user".into()),
        ("credential".into(), "api_key".into()),
    ]);
    let passwords = old.capabilities.get_mut("password_auth").unwrap();
    passwords.version = 1;
    passwords.requires = vec!["customers".into()];
    let db = Connection::open(temp.path("app.sqlite")).unwrap();
    db.execute_batch(&openb2c_schema::sql::generate(&old).unwrap())
        .unwrap();
    db.execute_batch("CREATE TABLE openb2c_schema_model(id INTEGER PRIMARY KEY,model_json TEXT NOT NULL);CREATE TABLE openb2c_migration(id TEXT PRIMARY KEY,checksum TEXT NOT NULL,description TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);INSERT INTO user(id,email,name) VALUES (42,'legacy@example.test','Legacy reader');").unwrap();
    db.execute(
        "INSERT INTO openb2c_schema_model VALUES (1,?)",
        [serde_json::to_string(&old).unwrap()],
    )
    .unwrap();
    let key = "do_legacy-retained-credential";
    db.execute("INSERT INTO api_key(user_id,key_hash,key_prefix,name,scopes,active) VALUES (42,?,?,'Legacy','orders.read',1)",rusqlite::params![bcrypt::hash(key,4).unwrap(),key.chars().take(11).collect::<String>()]).unwrap();
    let plan = transitions::plan_at(&db, &new, Some(&temp.path("migrations"))).unwrap();
    assert_eq!(plan["requires_review"], true);
    transitions::approve_at(
        &db,
        &new,
        Some(&temp.path("migrations")),
        plan["plan_hash"].as_str().unwrap(),
        "Preserve legacy identities and keys",
    )
    .unwrap();
    drop(db);
    let s = open(new, &temp);
    assert_eq!(s.authenticate(key).unwrap().user_id, Some(42));
    assert_eq!(
        s.db.query_row(
            "SELECT user_id FROM customer_profile WHERE id=42",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        42
    );
    assert_eq!(s.raw("user", &json!(42)).unwrap()["name"], "Legacy reader");
}

#[test]
fn older_billing_refund_journals_upgrade_without_refunding_cash_twice() {
    let temp = Temp::new();
    let new = schema("office");
    let s = open(new.clone(), &temp);
    let i=s.backoffice_invoice(&json!({"idempotency_key":"old","customer":"Reader","description":"Book","date":"2026-01-01","due_date":"2026-01-01","net_minor":1000,"tax_code":"standard"}),&admin()).unwrap();
    s.backoffice_receive(id(&i), &json!({"date":"2026-01-02"}), &admin())
        .unwrap();
    s.backoffice_credit(
        id(&i),
        &json!({"date":"2026-01-03","reason":"Credit"}),
        &admin(),
    )
    .unwrap();
    s.backoffice_refund(
        id(&i),
        &json!({"date":"2026-01-03","reason":"Refund"}),
        &admin(),
    )
    .unwrap();
    let before = s
        .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
        .unwrap();
    // Version 1 represented an automatic cash refund only by its ledger journal.
    let mut old = new.clone();
    old.capabilities.get_mut("billing").unwrap().version = 1;
    for (role, table) in [("refund", "invoice_refund"), ("event", "billing_event")] {
        old.capabilities
            .get_mut("billing")
            .unwrap()
            .entities
            .remove(role);
        old.tables.remove(table);
        old.operations.remove(table);
        s.db.execute_batch(&format!("DROP TABLE {}", quote(table)))
            .unwrap();
    }
    s.db.execute(
        "UPDATE openb2c_schema_model SET model_json=? WHERE id=1",
        [serde_json::to_string(&old).unwrap()],
    )
    .unwrap();
    s.db.execute("DELETE FROM openb2c_migration WHERE id LIKE 'schema:%'", [])
        .unwrap();
    approve(&s, &new, &temp.path("migrations"));
    drop(s);
    let upgraded = open(new, &temp);
    assert_eq!(
        upgraded.backoffice_invoice_view(1, &admin()).unwrap()["refund_due_minor"],
        0
    );
    assert_eq!(
        before,
        upgraded
            .backoffice_financial_report(&range("2026-01-01", "2026-01-31"), &admin())
            .unwrap()
    );
    let count: i64 = upgraded
        .db
        .query_row("SELECT count(*) FROM invoice_refund", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn migration_transaction_control_cannot_commit_partial_business_changes() {
    let temp = Temp::new();
    let model = schema("ledger");
    let s = open(model.clone(), &temp);
    fs::create_dir(temp.path("migrations")).unwrap();
    fs::write(temp.path("migrations/001_invalid_control.sql"),"INSERT INTO ledger_account(code,name,kind) VALUES ('unsafe','Must roll back','asset'); COMMIT;").unwrap();
    approve(&s, &model, &temp.path("migrations"));
    assert!(
        Service::open_at(
            model,
            temp.path("app.sqlite"),
            false,
            &temp.path("migrations")
        )
        .is_err()
    );
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM ledger_account WHERE code='unsafe'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM openb2c_migration WHERE id='001_invalid_control'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
