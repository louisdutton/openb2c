use openb2c_runtime::{AuthContext, Service, service::ListOptions};
use openb2c_schema::Schema;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("openb2c-native-{}-{unique}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn evaluate(path: &str) -> Vec<u8> {
    let result = Command::new("nix")
        .args(["eval", "--json", "-f", path])
        .current_dir(root())
        .output()
        .expect("Nix is required; run tests in devenv shell");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

fn generate(input: &[u8], output: &Path, extra: &[&str]) -> std::process::Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_openb2c"))
        .arg("-")
        .arg("--out")
        .arg(output)
        .args(extra)
        .env_remove("OPENB2C_EXPLAIN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Argument errors can close stdin before the test finishes writing JSON.
    // Preserve the exit status and diagnostic so the caller can assert them.
    if let Err(error) = process.stdin.take().unwrap().write_all(input) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    process.wait_with_output().unwrap()
}

#[test]
fn native_cli_generates_modular_b2c_backend() {
    let temp = TempDir::new();
    // A model is portable: no repository-relative imports or companion files.
    let model = temp.0.join("publisher ${1 + 1}.nix");
    fs::copy(root().join("examples/publisher.nix"), &model).unwrap();
    let output = temp.0.join("app");
    for directory in [
        "support/runtime/src",
        "support/runtime/assets",
        "assets",
        "migrations",
    ] {
        fs::create_dir_all(output.join(directory)).unwrap();
    }
    for file in [
        "support/runtime/src/ui.rs",
        "support/runtime/src/mcp.rs",
        "support/runtime/assets/app.css",
        "assets/logo.svg",
    ] {
        fs::write(output.join(file), "obsolete generated artifact").unwrap();
    }
    fs::write(output.join("migrations/001_operator.sql"), "SELECT 1;").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_openb2c"))
        .arg(&model)
        .args(["--out", output.to_str().unwrap(), "--explain=json"])
        .current_dir(&temp.0)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["capabilities"].as_object().unwrap().len(), 21);
    assert!(report.get("surfaces").is_none());
    let schema: Schema =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    let service = Service::open(schema, ":memory:", true).unwrap();
    assert_eq!(service.schema.tables.len(), 34);
    assert_eq!(
        service
            .list("price", &ListOptions::default(), &AuthContext::system())
            .unwrap()["total"],
        3
    );
    assert!(output.join("support/runtime/src/web.rs").is_file());
    assert!(!output.join("server.ts").exists());
    let manifest = fs::read_to_string(output.join("support/runtime/Cargo.toml")).unwrap();
    assert!(!manifest.contains("workspace = true"));
    let json: Value =
        serde_json::from_slice(&fs::read(output.join("openapi.json")).unwrap()).unwrap();
    assert!(json["paths"]["/b2c/orders"].is_object());
    assert!(json["paths"]["/api/payments/{id}"].get("patch").is_none());
    assert!(json["paths"].get("/mcp").is_none());
    assert!(!output.join("support/runtime/src/ui.rs").exists());
    assert!(!output.join("support/runtime/src/mcp.rs").exists());
    assert!(!output.join("assets/logo.svg").exists());
    assert!(!output.join("support/runtime/assets/app.css").exists());
    assert_eq!(
        fs::read_to_string(output.join("migrations/001_operator.sql")).unwrap(),
        "SELECT 1;"
    );
}

#[test]
fn application_modules_support_imports_overrides_and_dependency_errors() {
    let temp = TempDir::new();
    fs::copy(
        root().join("examples/publisher.nix"),
        temp.0.join("publisher.nix"),
    )
    .unwrap();
    let model = temp.0.join("model.nix");
    fs::write(
        &model,
        r#"{ lib, config, ... }: {
          imports = [ ./publisher.nix ];
          organization.name = lib.mkForce "Custom publisher";
          organization.description = lib.mkForce config.organization.name;
          currency = lib.mkForce "EUR";
          accounting.enable = lib.mkForce false;
          billing.enable = lib.mkForce false;
          documents.enable = lib.mkForce false;
          purchasing.enable = lib.mkForce false;
          banking.enable = lib.mkForce false;
          tax.enable = lib.mkForce false;
          capabilities.catalog.entities = { product = "publication"; price = "offer"; };
        }"#,
    )
    .unwrap();
    let output = temp.0.join("generated");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_openb2c"))
            .arg(&model)
            .arg("--out")
            .arg(&output)
            .current_dir(&temp.0)
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let schema = fs::read(output.join("schema.json")).unwrap();
    let json: Value = serde_json::from_slice(&schema).unwrap();
    assert_eq!(json["organization"]["name"], "Custom publisher");
    assert_eq!(json["organization"]["description"], "Custom publisher");
    assert_eq!(json["currency"], "EUR");
    assert_eq!(json["tables"]["offer"]["currency"]["default"], "'EUR'");
    let evaluated: Schema = serde_json::from_slice(&schema).unwrap();
    assert!(!evaluated.seed.reference.contains_key("publication"));
    assert!(evaluated.seed.apply_fixtures_by_default);
    let service = Service::open(evaluated, ":memory:", false).unwrap();
    assert_eq!(service.b2c_catalog().unwrap()["items"], json!([]));
    // Existing IDs and operator edits must survive demo initialization.
    service.db.execute_batch(
        "INSERT INTO publication (id, code, name) VALUES (1, 'operator-product', 'Operator product'), (42, 'digital-book', 'Edited title');
         INSERT INTO offer (id, product_id, amount_minor, billing_interval) VALUES (1, 1, 1500, 'one_off');"
    ).unwrap();
    let demo = openb2c_schema::sql::seed_sql(&service.schema, true).unwrap();
    service.db.execute_batch(&demo).unwrap();
    service.db.execute_batch(&demo).unwrap();
    let items = service.b2c_catalog().unwrap();
    assert_eq!(items["items"].as_array().unwrap().len(), 3);
    let book = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["product"]["code"] == "digital-book")
        .unwrap();
    assert_eq!(book["product"]["id"], 42);
    assert_eq!(book["product"]["name"], "Edited title");
    assert_eq!(book["prices"].as_array().unwrap().len(), 1);
    assert_eq!(book["prices"][0]["product_id"], 42);
    assert_eq!(book["prices"][0]["amount_minor"], 1500);
    assert_eq!(book["prices"][0]["currency"], "EUR");
    assert_eq!(
        service
            .db
            .query_row("SELECT count(*) FROM offer", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        4
    );

    fs::write(&model, "{...}: { orders.enable = true; }").unwrap();
    let result = run();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("orders requires customers"));
    assert_eq!(fs::read(output.join("schema.json")).unwrap(), schema);

    fs::write(
        &model,
        r#"{ lib, ... }: {
          imports = [ ./publisher.nix ];
          orders.enable = lib.mkForce false;
        }"#,
    )
    .unwrap();
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let reduced: Value =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    assert!(reduced["capabilities"].get("orders").is_none());
    assert!(reduced["tables"].get("purchase_order").is_none());
    assert!(reduced["tables"].get("order_payment").is_none());
    assert!(reduced["tables"].get("subscription").is_some());
}

#[test]
fn validation_errors_do_not_touch_existing_output() {
    let temp = TempDir::new();
    let marker = temp.0.join("schema.sql");
    fs::write(&marker, "keep this").unwrap();
    let result = generate(br#"{"tables":{"broken":{"id":{"type":"integer","pk":true},"fk":{"type":"integer","references":"missing(id)"}}}}"#, &temp.0, &[]);
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(marker).unwrap(), "keep this");
    assert!(!temp.0.join("Cargo.toml").exists());
}

#[test]
fn invalid_arguments_fail_before_creating_output() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let result = generate(b"{}", &output, &["--explain=yaml"]);
    assert!(!result.status.success());
    assert!(!output.exists());
    let result = generate(b"{}", &output, &["--typo"]);
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
fn generated_records_execute_missing_null_defaults_and_noninteger_keys() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let model = json!({"tables":{"_1":{
        "id":{"type":"integer","pk":true,"auto":true},
        "type":{"type":"text"},
        "self":{"type":"text","required":true,"default":"'default'"}
    },"named":{"key":{"type":"text","pk":true},"value":{"type":"text"}}},"operations":{"_1":{"read":{},"create":{},"update":{}},"named":{"read":{},"create":{},"update":{}}}});
    let result = generate(model.to_string().as_bytes(), &output, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.join("src/models.rs").exists());
    compiled_probe(
        &output,
        r###"
use openb2c_runtime::{Service,AuthContext};
use serde_json::json;
#[test]
fn typed_storage() {
    let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();
    let s=Service::open(model,":memory:",false).unwrap();let owner=AuthContext::system();
    let row=s.create("_1",&json!({}),&owner).unwrap();let id=&row["id"];
    assert_eq!(row["self"],"default");assert_eq!(row["type"],json!(null));
    assert_eq!(s.mutate("_1",id,"update",&json!({"type":"ticket"}),&owner,None).unwrap()["type"],"ticket");
    assert_eq!(s.mutate("_1",id,"update",&json!({}),&owner,None).unwrap()["type"],"ticket");
    assert_eq!(s.mutate("_1",id,"update",&json!({"type":null}),&owner,None).unwrap()["type"],json!(null));
    assert!(s.mutate("_1",id,"update",&json!({"self":null}),&owner,None).is_err());
    assert!(s.mutate("_1",id,"update",&json!({"id":2}),&owner,None).is_err());
    s.create("named",&json!({"key":"book","value":"before"}),&owner).unwrap();
    assert_eq!(s.mutate("named",&json!("book"),"update",&json!({"value":"after"}),&owner,None).unwrap()["value"],"after");
    assert!(s.mutate("named",&json!("book"),"update",&json!({"key":"other"}),&owner,None).is_err());
    assert_eq!(s.find("named",&json!("book"),&owner).unwrap()["value"],"after");
}
"###,
    );
}

#[test]
fn generated_duchy_sql_matches_existing_tables_keys_indexes_and_seed_counts() {
    // Differential test against the captured pre-migration baseline. This checks
    // actual SQLite structures and data, independent of SQL formatting.
    let input = evaluate("crates/openb2c-runtime/tests/fixtures/composition.nix");
    let model: Schema = serde_json::from_slice(&input).unwrap();
    let legacy: Value =
        serde_json::from_str(include_str!("fixtures/duchy-typescript-baseline.json")).unwrap();
    let old = Connection::open_in_memory().unwrap();
    old.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    for key in ["sql", "reference", "fixtures"] {
        old.execute_batch(legacy[key].as_str().unwrap()).unwrap();
    }
    let native = Service::open(model.clone(), ":memory:", true).unwrap();
    for table in model.tables.keys() {
        assert_eq!(
            pragma_rows(&old, "table_info", table),
            pragma_rows(&native.db, "table_info", table),
            "columns in {table}"
        );
        assert_eq!(
            pragma_rows(&old, "foreign_key_list", table),
            pragma_rows(&native.db, "foreign_key_list", table),
            "foreign keys in {table}"
        );
        assert_eq!(
            pragma_rows(&old, "index_list", table),
            pragma_rows(&native.db, "index_list", table),
            "indexes in {table}"
        );
        let query = format!("SELECT count(*) FROM {}", openb2c_schema::sql::quote(table));
        let before: i64 = old.query_row(&query, [], |r| r.get(0)).unwrap();
        let after: i64 = native.db.query_row(&query, [], |r| r.get(0)).unwrap();
        assert_eq!(before, after, "seed counts in {table}");
    }
}

fn pragma_rows(connection: &Connection, pragma: &str, table: &str) -> Vec<Vec<String>> {
    use rusqlite::types::ValueRef;
    let query = format!("PRAGMA {pragma}({})", openb2c_schema::sql::quote(table));
    let mut statement = connection.prepare(&query).unwrap();
    let count = statement.column_count();
    let mut rows = statement
        .query_map([], |r| {
            // Ignore SQLite's enumeration sequence: FK/index creation order can
            // change while the structure and enforcement remain equivalent.
            let start = usize::from(pragma == "foreign_key_list" || pragma == "index_list");
            (start..count)
                .map(|i| {
                    r.get_ref(i).map(|v| match v {
                        ValueRef::Null => "NULL".into(),
                        ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned(),
                        ValueRef::Integer(v) => v.to_string(),
                        ValueRef::Real(v) => v.to_string(),
                        ValueRef::Blob(_) => "blob".into(),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.sort();
    rows
}

#[test]
fn application_owned_workflows_survive_regeneration_and_populated_upgrades() {
    let temp = TempDir::new();
    fs::copy(
        root().join("crates/openb2c/tests/fixtures/editorial.nix"),
        temp.0.join("editorial.nix"),
    )
    .unwrap();
    let model = temp.0.join("model.nix");
    fs::write(&model, "{...}: { imports = [ ./editorial.nix ]; }").unwrap();
    let output = temp.0.join("generated");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_openb2c"))
            .arg(&model)
            .arg("--out")
            .arg(&output)
            .current_dir(&temp.0)
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let first: Schema =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    let db = temp.0.join("owned.sqlite");
    let migrations = temp.0.join("operator-migrations");
    fs::create_dir(&migrations).unwrap();
    let s = Service::open_at(first, &db, false, &migrations).unwrap();
    let operator = AuthContext {
        user_id: Some(1),
        scopes: vec![
            "editorial.write".into(),
            "editorial.read".into(),
            "editorial.approve".into(),
        ],
    };
    let draft = s
        .create(
            "editorial_job",
            &json!({"title":"First edition","status":"approved"}),
            &operator,
        )
        .unwrap();
    assert_eq!(draft["status"], "draft");
    assert!(
        s.mutate(
            "editorial_job",
            &draft["id"],
            "update",
            &json!({"status":"approved"}),
            &operator,
            None
        )
        .is_err()
    );
    fs::write(&model,"{...}: { imports = [ ./editorial.nix ]; tables.editorial_job.operator_note.type = \"text\"; }").unwrap();
    let sql = "CREATE TABLE operator_note(id INTEGER PRIMARY KEY,note TEXT);INSERT INTO operator_note VALUES (1,'Business-owned SQL survives');";
    fs::write(migrations.join("001_owner_note.sql"), sql).unwrap();
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(migrations.join("001_owner_note.sql")).unwrap(),
        sql
    );
    let next: Schema =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    let plan = openb2c_runtime::transitions::plan_at(&s.db, &next, Some(&migrations)).unwrap();
    openb2c_runtime::transitions::approve_at(
        &s.db,
        &next,
        Some(&migrations),
        plan["plan_hash"].as_str().unwrap(),
        "Retain editorial policy and operator SQL",
    )
    .unwrap();
    let upgraded = Service::open_at(next, &db, false, &migrations).unwrap();
    let approved = upgraded
        .mutate(
            "editorial_job",
            &draft["id"],
            "approve",
            &json!({}),
            &operator,
            None,
        )
        .unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["title"], "First edition");
    assert!(
        upgraded
            .mutate(
                "editorial_job",
                &draft["id"],
                "approve",
                &json!({}),
                &operator,
                None
            )
            .is_err()
    );
    assert_eq!(
        upgraded
            .db
            .query_row("SELECT note FROM operator_note", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "Business-owned SQL survives"
    );
    assert_eq!(
        fs::read_to_string(temp.0.join("editorial.nix")).unwrap(),
        fs::read_to_string(root().join("crates/openb2c/tests/fixtures/editorial.nix")).unwrap()
    );
}

fn compiled_probe(output: &Path, source: &str) {
    compiled_probe_with_env(output, source, &[]);
}

fn compiled_probe_with_env(output: &Path, source: &str, environment: &[(&str, &str)]) {
    let probe = output.parent().unwrap().join("compiled-probe");
    fs::create_dir_all(&probe).unwrap();
    fs::write(probe.join("Cargo.toml"),format!("[package]\nname=\"compiled-probe\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[dependencies]\nserde_json=\"1\"\nrusqlite=\"0.37\"\nopenb2c-runtime={{path={:?}}}\nopenb2c-schema={{path={:?}}}\n[lib]\npath=\"probe.rs\"\n",output.join("support/runtime"),output.join("support/schema"))).unwrap();
    fs::write(probe.join("probe.rs"), source).unwrap();
    let result = Command::new("cargo")
        .args(["test", "--offline", "--quiet", "--manifest-path"])
        .arg(probe.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", root().join("target/compiled-probes"))
        .envs(environment.iter().copied())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Compiled application probe failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn compiled_bank_transfers_preserve_money_access_and_subscription_boundaries() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let result = Command::new(env!("CARGO_BIN_EXE_openb2c"))
        .arg(root().join("examples/publisher.nix"))
        .arg("--out")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.join("support/runtime/src/b2c/starling.rs").exists());
    assert!(
        !output
            .join("support/runtime/src/b2c/open_banking.rs")
            .exists()
    );
    assert!(!output.join("support/runtime/src/b2c/bank_feed.rs").exists());
    assert!(
        !fs::read_to_string(output.join("support/runtime/src/b2c/payments.rs"))
            .unwrap()
            .contains("simulate_payment")
    );
    compiled_probe(&output, include_str!("fixtures/transfers.rs"));
    let invalid_dependency = Command::new("nix-instantiate")
        .args([
            "--eval",
            "--strict",
            "--json",
            "crates/openb2c-runtime/tests/fixtures/capabilities.nix",
            "--argstr",
            "selection",
            "broken_billing_payments",
        ])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(!invalid_dependency.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_dependency.stderr)
            .contains("billing_payments requires payments")
    );
    let mut model: Value =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    model["workflows"]["allocated_cash"]["steps"][0]["steps"][0]["steps"][0]["amount"] =
        json!({"value":"not money"});
    let before = fs::read(output.join("src/main.rs")).unwrap();
    let invalid = generate(model.to_string().as_bytes(), &output, &[]);
    assert!(!invalid.status.success());
    assert_eq!(fs::read(output.join("src/main.rs")).unwrap(), before);
}

#[test]
fn reviewed_billing_activation_uses_bank_allocation_history() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let model = temp.0.join("model.nix");
    fs::copy(
        root().join("examples/publisher.nix"),
        temp.0.join("publisher.nix"),
    )
    .unwrap();
    fs::write(
        &model,
        "{lib,...}:{imports=[./publisher.nix];billing.enable=lib.mkForce false;}",
    )
    .unwrap();
    let run = || {
        let result = Command::new(env!("CARGO_BIN_EXE_openb2c"))
            .arg(&model)
            .arg("--out")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run();
    let database = format!("{:?}", temp.0.join("owned.sqlite"));
    let migrations = format!("{:?}", temp.0.join("migrations"));
    compiled_probe(&output,&r#"
use openb2c_runtime::{Service,AuthContext};use serde_json::{Value,json};
#[test] fn prepare_bank_history(){
 let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();let s=Service::open_at(model,__DATABASE__,true,std::path::Path::new(__MIGRATIONS__)).unwrap();let owner=AuthContext::system();let user=s.ensure_customer("history@example.test",&owner).unwrap();let auth=AuthContext{user_id:Some(user),scopes:s.self_scopes()};
 for i in 1..=2{
  let order=s.b2c_create_order(&json!({"idempotency_key":format!("order-{i}"),"items":[{"price_id":1,"quantity":1}]}),&auth).unwrap();
  let payment=s.b2c_order_payment(order["order"]["id"].as_i64().unwrap(),&json!({"idempotency_key":"payment"}),&auth).unwrap();
  let mut fact=json!({"transaction_id":format!("bank-{i}"),"account":"simulated","amount_minor":1500,"currency":"GBP","direction":"in","status":"settled","reference":payment["bank_reference"],"settled_at":"2025-01-01T12:00:00Z","updated_at":"2025-01-01T12:00:00Z"});
  assert_eq!(s.import_simulated_transfer(&fact,&owner).unwrap()["unallocated_minor"],0);
  if i==2{fact["status"]=json!("reversed");fact["updated_at"]=json!("2025-02-01T12:00:00Z");let result=s.import_simulated_transfer(&fact,&owner).unwrap();assert!(result["review_reason"].is_null(),"{result}");}
 }
 assert_eq!(s.db.query_row("SELECT count(*) FROM payment_event",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
"#.replace("__DATABASE__",&database).replace("__MIGRATIONS__",&migrations));
    fs::write(&model, "{...}:{imports=[./publisher.nix];}").unwrap();
    run();
    compiled_probe(&output,&r#"
use openb2c_runtime::{Service,AuthContext,transitions};use serde_json::json;
#[test] fn reconstructs_sales_and_reversals_from_bank_facts(){
 let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();let dir=std::path::Path::new(__MIGRATIONS__);let db=rusqlite::Connection::open(__DATABASE__).unwrap();
 let plan=transitions::plan_at(&db,&model,Some(dir)).unwrap();assert_eq!(plan["requires_review"],true);
 transitions::approve_at(&db,&model,Some(dir),plan["plan_hash"].as_str().unwrap(),"Preserve bank evidence during reviewed billing activation").unwrap();drop(db);
 let s=Service::open_at(model,__DATABASE__,false,dir).unwrap();let auth=AuthContext::system();
 assert_eq!(s.backoffice_list("billing","invoice",&auth).unwrap()["items"].as_array().unwrap().len(),2);
 assert_eq!(s.backoffice_list("billing","credit",&auth).unwrap()["items"].as_array().unwrap().len(),1);
 let report=s.backoffice_financial_report(&json!({"from":"2025-01-01","through":"2025-12-31"}),&auth).unwrap();assert_eq!(report["balanced"],true);
 let bank=report["trial_balance"].as_array().unwrap().iter().find(|r|r["code"]=="bank").unwrap();assert_eq!(bank["balance_minor"],1500);
 let invoice=s.backoffice_invoice_view(1,&auth).unwrap();assert_eq!(invoice["date"],"2025-01-01");assert_eq!(invoice["tax_code"],"unassessed");
}
"#.replace("__DATABASE__",&database).replace("__MIGRATIONS__",&migrations));
}

#[test]
fn generated_execution_obeys_nix_composition_and_custom_operation_rules() {
    let temp = TempDir::new();
    fs::copy(
        root().join("examples/publisher.nix"),
        temp.0.join("publisher.nix"),
    )
    .unwrap();
    fs::copy(
        root().join("crates/openb2c/tests/fixtures/editorial.nix"),
        temp.0.join("editorial.nix"),
    )
    .unwrap();
    let model = temp.0.join("model.nix");
    fs::write(&model,r#"{config,lib,expr,...}: {
      imports=[./publisher.nix ./editorial.nix];
      name=lib.mkForce "Compiled publisher";
      description=lib.mkForce "Compiled operations";
      workflows.purchased_access=lib.mkForce {event="order";order=30;steps=[];};
      tables.purchase_order.blocked={type="integer";required=true;default="0";};
      constraints.purchase_order.check.accepted_sale={
        enforcement="application";
        fields=[config.refs.purchase_order.status config.refs.purchase_order.blocked];
        condition=expr.eq (expr.f config.refs.purchase_order.status) (expr.lit "paid");
        expression=expr.eq (expr.f config.refs.purchase_order.blocked) (expr.lit 0);
      };
      derived.purchase_order.units={type="integer";expression=expr.sum config.entityRefs.order_item config.refs.order_item.quantity;};
      tables.editorial_job.score={type="integer";required=true;default="0";};
      tables.editorial_part={
        id={type="integer";pk=true;auto=true;};
        job_id={type="integer";required=true;references="editorial_job(id)";};
        amount={type="integer";required=true;};
      };
      constraints.editorial_part.check.non_negative={
        enforcement="application";
        fields=[config.refs.editorial_part.amount];
        expression=expr.gte (expr.f config.refs.editorial_part.amount) (expr.lit 0);
      };
      derived.editorial_job.total={type="integer";expression=expr.sum config.entityRefs.editorial_part config.refs.editorial_part.amount;};
      operations.editorial_job.bump={
        scope="editorial.approve";
        set.score.expression=expr.sum config.entityRefs.editorial_part config.refs.editorial_part.amount;
        cascade=[{entity=config.entityRefs.editorial_part;set.amount.expression=expr.add (expr.f config.refs.editorial_part.amount) (expr.lit 1);}];
      };
      workflows.fail_after_access={event="order";order=35;steps=[{
        _t="if";
        condition={op="and";left={op="eq";left.path="event.status";right.value="paid";};right={op="eq";left.path="event.amount_minor";right.value=3000;};};
        steps=[
          {_t="grant";user.path="event.user_id";resource.value="rollback";source.value="rollback";end=null;}
          {_t="require";condition.value=false;message="Deliberate business rejection after access";}
        ];
      }];};
    }"#).unwrap();
    let output = temp.0.join("app");
    let result = Command::new(env!("CARGO_BIN_EXE_openb2c"))
        .arg(&model)
        .arg("--out")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let composition =
        fs::read_to_string(output.join("support/runtime/src/composition.rs")).unwrap();
    assert!(!composition.contains("Consumer"));
    assert!(!composition.contains("capabilities::endpoints"));
    assert!(!output.join("support/runtime/src/b2c/rest.rs").exists());
    let operations =
        fs::read_to_string(output.join("support/runtime/src/compiled_operations.rs")).unwrap();
    assert!(!operations.contains("self.evaluate("));
    compiled_probe(
        &output,
        r#"
use openb2c_runtime::{Service,AuthContext};
use serde_json::{Value,json};
fn service()->Service {let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();Service::open(model,":memory:",true).unwrap()}
fn settle(s:&Service,payment:i64,event:&str)->Value{
 let p=s.b2c_payment(payment,&AuthContext::system()).unwrap();
 s.import_simulated_transfer(&json!({"transaction_id":event,"account":"simulated","amount_minor":p["amount_minor"],"currency":"GBP","direction":"in","status":"settled","reference":p["bank_reference"],"settled_at":p["created_at"],"updated_at":p["created_at"]}),&AuthContext::system()).unwrap()
}
#[test]
fn compiled_purchase_uses_the_nix_override(){
 let s=service();let owner=AuthContext::system();let id=s.ensure_customer("reader@example.test",&owner).unwrap();let auth=AuthContext{user_id:Some(id),scopes:s.self_scopes()};
 let order=s.b2c_create_order(&json!({"idempotency_key":"book","items":[{"price_id":1,"quantity":1}]}),&auth).unwrap();
 let p=s.b2c_order_payment(order["order"]["id"].as_i64().unwrap(),&json!({"idempotency_key":"pay"}),&auth).unwrap();
 settle(&s,p["id"].as_i64().unwrap(),"paid");
 assert_eq!(s.b2c_order(1,&auth).unwrap()["order"]["status"],"paid");
 // The capability is selected, but Nix replaced its business reaction.
 assert!(s.schema.capabilities.contains_key("order_entitlements"));
 assert_eq!(s.b2c_entitlements(&auth).unwrap()["items"],json!([]));
 assert_eq!(s.backoffice_list("billing","invoice",&owner).unwrap()["items"].as_array().unwrap().len(),1);
}
#[test]
fn compiled_application_owned_operations_preserve_guards_and_authorization(){
 let s=service();let writer=AuthContext{user_id:Some(1),scopes:vec!["editorial.write".into(),"editorial.read".into(),"editorial.approve".into()]};
 let row=s.create("editorial_job",&json!({"title":"A book","status":"approved"}),&writer).unwrap();assert_eq!(row["status"],"draft");
 assert!(s.mutate("editorial_job",&row["id"],"update",&json!({"status":"approved"}),&writer,None).is_err());
 assert!(s.mutate("editorial_job",&row["id"],"approve",&json!({}),&AuthContext::default(),None).is_err());
 let approved=s.mutate("editorial_job",&row["id"],"approve",&json!({}),&writer,None).unwrap();assert_eq!(approved["status"],"approved");
 assert!(s.mutate("editorial_job",&row["id"],"approve",&json!({}),&writer,None).is_err());
 assert_eq!(s.find("editorial_job",&row["id"],&writer).unwrap()["title"],"A book");
}
#[test]
fn compiled_reactions_roll_back_the_whole_payment_transaction(){
 let s=service();let owner=AuthContext::system();let id=s.ensure_customer("rollback@example.test",&owner).unwrap();let auth=AuthContext{user_id:Some(id),scopes:s.self_scopes()};
 let o=s.b2c_create_order(&json!({"idempotency_key":"rollback","items":[{"price_id":1,"quantity":2}]}),&auth).unwrap();
 let p=s.b2c_order_payment(o["order"]["id"].as_i64().unwrap(),&json!({"idempotency_key":"pay"}),&auth).unwrap();
 let result=settle(&s,p["id"].as_i64().unwrap(),"paid");
 assert!(result["review_reason"].is_string());
 assert_eq!(result["unallocated_minor"],3000);
 assert_eq!(s.b2c_payment(1,&auth).unwrap()["status"],"pending");
 assert_eq!(s.b2c_order(1,&auth).unwrap()["order"]["status"],"pending");
 assert_eq!(s.b2c_entitlements(&auth).unwrap()["items"],json!([]));
 assert_eq!(s.backoffice_list("billing","invoice",&owner).unwrap()["items"],json!([]));
}
#[test]
fn typed_writes_enforce_application_constraints_and_keep_failed_events_retryable(){
 let s=service();let owner=AuthContext::system();let id=s.ensure_customer("blocked@example.test",&owner).unwrap();let auth=AuthContext{user_id:Some(id),scopes:s.self_scopes()};
 let o=s.b2c_create_order(&json!({"idempotency_key":"blocked","items":[{"price_id":1,"quantity":1}]}),&auth).unwrap();
 assert_eq!(o["order"]["units"],1);assert_eq!(o["order"]["blocked"],0);
 let order=o["order"]["id"].as_i64().unwrap();
 let p=s.b2c_order_payment(order,&json!({"idempotency_key":"pay"}),&auth).unwrap();let payment=p["id"].as_i64().unwrap();
 s.db.execute("UPDATE purchase_order SET blocked=1 WHERE id=?",[order]).unwrap();
 assert!(settle(&s,payment,"settlement")["review_reason"].is_string());
 assert_eq!(s.b2c_payment(payment,&auth).unwrap()["status"],"pending");assert_eq!(s.b2c_order(order,&auth).unwrap()["order"]["status"],"pending");
 assert_eq!(s.db.query_row("SELECT count(*) FROM bank_transaction",[],|r|r.get::<_,i64>(0)).unwrap(),1);
 assert_eq!(s.db.query_row("SELECT count(*) FROM payment_allocation",[],|r|r.get::<_,i64>(0)).unwrap(),0);
 s.db.execute("UPDATE purchase_order SET blocked=0 WHERE id=?",[order]).unwrap();
 let response=settle(&s,payment,"settlement");
 assert_eq!(response["unallocated_minor"],0);
 assert_eq!(s.b2c_payment(payment,&owner).unwrap()["status"],"succeeded");
 assert_eq!(settle(&s,payment,"settlement"),response);
}
#[test]
fn a_compiled_service_rejects_a_different_model_and_stale_revision(){
 let mut schema:openb2c_schema::Schema=serde_json::from_str(include_str!("../app/schema.json")).unwrap();
 schema.workflows.remove("fail_after_access");assert!(Service::open(schema,":memory:",true).is_err());
 let s=service();s.assert_current_model().unwrap();
 s.db.execute("UPDATE openb2c_application_revision SET sha256='different-application'",[]).unwrap();assert!(s.assert_current_model().is_err());
}
#[test]
fn compiled_aggregates_constraints_and_cascades_preserve_record_semantics(){
 let s=service();let owner=AuthContext::system();
 let job=s.create("editorial_job",&json!({"title":"Collections"}),&owner).unwrap();
 assert!(s.create("editorial_part",&json!({"job_id":job["id"],"amount":-1}),&owner).is_err());
 for amount in [2,3]{s.create("editorial_part",&json!({"job_id":job["id"],"amount":amount}),&owner).unwrap();}
 let bumped=s.mutate("editorial_job",&job["id"],"bump",&json!({}),&owner,None).unwrap();
 assert_eq!(bumped["score"],5);assert_eq!(bumped["total"],7);
}
"#,
    );
    let mut schema: Value =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    schema["workflows"]["bad"] = json!({"event":"order","steps":[{"_t":"grant","user":{"path":"event.currency"},"resource":{"value":"book"},"source":{"value":"bad"},"end":null}]});
    let before = fs::read(output.join("src/main.rs")).unwrap();
    let result = generate(schema.to_string().as_bytes(), &output, &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("requires Int"));
    assert_eq!(fs::read(output.join("src/main.rs")).unwrap(), before);
}

#[test]
fn bank_reference_mapping_is_compiled_and_invalid_mappings_preserve_output() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let evaluated = Command::new("nix-instantiate")
        .args([
            "--eval",
            "--strict",
            "--json",
            "crates/openb2c-runtime/tests/fixtures/capabilities.nix",
            "--argstr",
            "selection",
            "open_banking",
        ])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        evaluated.status.success(),
        "{}",
        String::from_utf8_lossy(&evaluated.stderr)
    );
    let mut model: Value = serde_json::from_slice(&evaluated.stdout).unwrap();
    for mapping in ["none", "transaction_information", "transaction_reference"] {
        model["capabilities"]["payments"]["settings"]["reference_field"] = json!(mapping);
        let result = generate(model.to_string().as_bytes(), &output, &[]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let modules = fs::read_to_string(output.join("support/runtime/src/b2c/mod.rs")).unwrap();
        assert!(modules.contains(&format!(
            "const OPEN_BANKING_REFERENCE_FIELD: &str = \"{mapping}\";"
        )));
        assert!(modules.contains("use open_banking::OpenBanking as SelectedBankFeed"));
    }
    let previous = fs::read(output.join("src/main.rs")).unwrap();
    model["capabilities"]["payments"]["settings"]["reference_field"] =
        json!("guess_from_description");
    assert!(
        !generate(model.to_string().as_bytes(), &output, &[])
            .status
            .success()
    );
    assert_eq!(fs::read(output.join("src/main.rs")).unwrap(), previous);
}

#[test]
fn compiled_partial_compositions_include_only_selected_algorithms() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    for selection in [
        "catalog",
        "downloads",
        "billing",
        "billing_payments",
        "purchasing",
        "banking",
        "tax",
        "subscriptions",
        "payments",
        "starling",
        "open_banking",
        "renamed",
    ] {
        let evaluated = Command::new("nix-instantiate")
            .args([
                "--eval",
                "--strict",
                "--json",
                "crates/openb2c-runtime/tests/fixtures/capabilities.nix",
                "--argstr",
                "selection",
                selection,
            ])
            .current_dir(root())
            .output()
            .unwrap();
        assert!(
            evaluated.status.success(),
            "{}",
            String::from_utf8_lossy(&evaluated.stderr)
        );
        let result = generate(&evaluated.stdout, &output, &[]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let schema: Schema = serde_json::from_slice(&evaluated.stdout).unwrap();
        for adapter in ["starling", "open_banking"] {
            assert_eq!(
                output
                    .join(format!("support/runtime/src/b2c/{adapter}.rs"))
                    .exists(),
                selection == adapter
            );
        }
        let paths: Value =
            serde_json::from_slice(&fs::read(output.join("openapi.json")).unwrap()).unwrap();
        assert_eq!(
            !paths["paths"]["/backoffice/transfers/sync"].is_null(),
            ["starling", "open_banking"].contains(&selection)
        );
        assert_eq!(
            !paths["paths"]["/backoffice/transfers/feed-item"].is_null(),
            selection == "starling"
        );
        if ["starling", "open_banking"].contains(&selection) {
            assert!(paths["paths"]["/backoffice/transfers"]["post"].is_null());
            assert_eq!(
                paths["paths"]["/backoffice/transfers/sync"]["post"]["x-openb2c-scope"],
                "payments.import"
            );
        }
        for (cap, path) in [
            ("subscriptions", "src/b2c/subscriptions.rs"),
            ("billing", "src/backoffice/billing.rs"),
            ("purchasing", "src/backoffice/purchasing.rs"),
            ("downloads", "src/b2c/downloads.rs"),
        ] {
            assert_eq!(
                output.join("support/runtime").join(path).exists(),
                schema.capabilities.contains_key(cap),
                "{selection} includes unexpected {cap} implementation"
            );
        }
        compiled_probe(&output,r#"
use openb2c_runtime::{Service,AuthContext};
#[test] fn starts_selected_application(){let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();let s=Service::open(model,":memory:",true).unwrap();assert!(!s.self_scopes().iter().any(|s|s=="*"));if s.schema.capabilities.contains_key("catalog"){s.b2c_catalog().unwrap();}}
"#.replace("if s.schema.capabilities.contains_key(\"catalog\"){s.b2c_catalog().unwrap();}",if schema.capabilities.contains_key("catalog"){ "s.b2c_catalog().unwrap();" }else{ "" }).as_str());
        if ["starling", "open_banking"].contains(&selection) {
            let result = Command::new("cargo")
                .args(["test", "--offline", "--quiet", "--manifest-path"])
                .arg(output.join("Cargo.toml"))
                .args(["-p", "openb2c-runtime", "--lib"])
                .env("BANK_ACCOUNT_ID", "account")
                .env("BANK_ACCOUNT_NAME", "Local test bank")
                .env("BANK_SORT_CODE", "000000")
                .env("BANK_ACCOUNT_NUMBER", "00000000")
                .env("CARGO_TARGET_DIR", root().join("target/compiled-probes"))
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "Bank protocol tests failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}

#[test]
fn generated_publisher_runs_end_to_end_against_local_ais() {
    let temp = TempDir::new();
    fs::copy(
        root().join("examples/publisher.nix"),
        temp.0.join("publisher.nix"),
    )
    .unwrap();
    let model = temp.0.join("model.nix");
    fs::write(&model, r#"{ lib, ... }: {
      imports = [ ./publisher.nix ];
      payments.feed = lib.mkForce "open_banking";
      # This local bank explicitly returns the payer reference as TransactionInformation.
      payments.openBanking.referenceField = "transaction_information";
      workflows.reject_test_order = { event = "order"; order = 35; steps = [{
        _t = "if";
        condition = { op = "and";
          left = { op = "eq"; left.path = "event.status"; right.value = "paid"; };
          right = { op = "eq"; left.path = "event.amount_minor"; right.value = 3000; };
        };
        steps = [
          { _t = "grant"; user.path = "event.user_id"; resource.value = "rollback"; source.value = "rollback"; end = null; }
          { _t = "require"; condition.value = false; message = "Test publisher rejects this order"; }
        ];
      }]; };
    }"#).unwrap();
    let output = temp.0.join("app");
    let result = Command::new(env!("CARGO_BIN_EXE_openb2c"))
        .arg(&model)
        .arg("--out")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest = fs::read_to_string(output.join("support/runtime/Cargo.toml")).unwrap();
    assert!(!manifest.contains("default = [\"reference\"]"));
    let target = root().join("target/ais-publisher");
    let build = Command::new("cargo")
        .args(["build", "--offline", "--quiet", "--manifest-path"])
        .arg(output.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("python3")
        .arg(root().join("crates/openb2c/tests/fixtures/ais.py"))
        .arg(target.join("debug/openb2c-app"))
        .arg(&temp.0)
        .output()
        .expect("Python and OpenSSL are required; run in devenv shell");
    assert!(
        run.status.success(),
        "Local AIS trial failed:\n{}\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
        fs::read_to_string(temp.0.join("app.log")).unwrap_or_default()
    );
    compiled_probe_with_env(
        &output,
        &r#"
use openb2c_runtime::{Service, AuthContext};
use serde_json::{Value, json};
#[test]
fn paid_membership_renews_and_expires_at_the_compiled_boundary() {
    let model = serde_json::from_str(include_str!("../app/schema.json")).unwrap();
    let state: Value = serde_json::from_str(include_str!("../trial-state.json")).unwrap();
    let s = Service::open_at(model, __DATABASE__, false, std::path::Path::new(__MIGRATIONS__)).unwrap();
    let owner = AuthContext::system();
    let user = s.ensure_customer("stranger@example.test", &owner).unwrap();
    let auth = AuthContext { user_id: Some(user), scopes: s.self_scopes() };
    let sid = state["subscription_id"].as_i64().unwrap();
    let file = state["file_id"].as_i64().unwrap();
    let view = s.b2c_subscription(sid, &auth).unwrap();
    assert_eq!(view["subscription"]["status"], "active");
    let end = view["subscription"]["current_period_end"].as_str().unwrap().parse().unwrap();
    assert!(s.b2c_download_at(file, &auth, end).is_err());
    let before = end - std::time::Duration::from_secs(1);
    assert!(s.b2c_download_at(file, &auth, before).is_ok());
    assert_eq!(s.b2c_renew_due_at(&owner, before).unwrap()["payments"], json!([]));
    let next = s.b2c_renew_due_at(&owner, end).unwrap();
    assert_eq!(next["payments"].as_array().unwrap().len(), 1);
    assert_eq!(next["payments"][0]["bank_reference"], state["bank_reference"]);
    assert_ne!(next["payments"][0]["id"], view["payment"]["id"]);
    assert_eq!(next["payments"][0]["status"], "pending");
    assert_eq!(s.b2c_subscription(sid, &auth).unwrap()["subscription"]["status"], "past_due");
    assert!(s.b2c_download_at(file, &auth, end).is_err());
    assert_eq!(s.b2c_renew_due_at(&owner, end).unwrap()["payments"], json!([]));
}
"#
        .replace("__DATABASE__", &format!("{:?}", temp.0.join("app.sqlite")))
        .replace("__MIGRATIONS__", &format!("{:?}", output.join("migrations"))),
        &[
            ("BANK_ACCOUNT_ID", "local-account"),
            ("BANK_ACCOUNT_NAME", "Simulated publisher"),
            ("BANK_SORT_CODE", "000000"),
            ("BANK_ACCOUNT_NUMBER", "00000000"),
        ],
    );
}

#[test]
fn reviewed_cash_activation_preserves_legacy_journals_and_deferred_commands() {
    let temp = TempDir::new();
    fs::copy(
        root().join("examples/publisher.nix"),
        temp.0.join("publisher.nix"),
    )
    .unwrap();
    let model = temp.0.join("model.nix");
    let output = temp.0.join("app");
    fs::write(
        &model,
        r#"{lib,config,...}:{imports=[./publisher.nix];billing.payments.enable=lib.mkForce false;
 seed.reference.${config.capabilities.accounting.entities.account}=lib.mkForce (lib.imap0 (i: a: {id = i + 1;code=builtins.elemAt a 0;name=builtins.elemAt a 1;kind=builtins.elemAt a 2;}) [
 ["bank" "Bank" "asset"] ["clearing" "Payment clearing" "asset"] ["receivables" "Trade receivables" "asset"] ["tax_input" "Recoverable tax" "asset"] ["payables" "Trade payables" "liability"] ["tax_output" "Output tax" "liability"] ["deferred" "Deferred revenue" "liability"] ["sales" "Sales revenue" "income"] ["expenses" "Operating expenses" "expense"] ["equity" "Opening equity" "equity"] ]);}"#,
    )
    .unwrap();
    let run = || {
        let result = Command::new(env!("CARGO_BIN_EXE_openb2c"))
            .arg(&model)
            .arg("--out")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run();
    let database = format!("{:?}", temp.0.join("owned.sqlite"));
    let migrations = format!("{:?}", output.join("migrations"));
    compiled_probe(&output, &r#"
use openb2c_runtime::{Service,AuthContext};use serde_json::{json,Value};
#[test]fn prepare_old_cash_and_closed_period_commands(){
 let s=Service::open_at(serde_json::from_str(include_str!("../app/schema.json")).unwrap(),__DATABASE__,true,std::path::Path::new(__MIGRATIONS__)).unwrap();
 let owner=AuthContext::system();let user=s.ensure_customer("upgrade@example.test",&owner).unwrap();let a=AuthContext{user_id:Some(user),scopes:s.self_scopes()};
 let account=s.backoffice_account(&json!({"code":"owner_savings","name":"Owner savings","kind":"asset"}),&owner).unwrap();assert_eq!(account["id"],11);
 for (key,amount,day) in [("paid",1500,"2025-01-01"),("partial",400,"2025-01-01"),("deferred",1500,"2025-01-01")]{
  if key=="deferred"{s.backoffice_close(&json!({"through_date":"2025-01-02"}),&owner).unwrap();}
  let o=s.b2c_create_order(&json!({"idempotency_key":key,"items":[{"price_id":1,"quantity":1}]}),&a).unwrap();
  let p=s.b2c_order_payment(o["order"]["id"].as_i64().unwrap(),&json!({"idempotency_key":key}),&a).unwrap();
  let t=s.import_simulated_transfer(&json!({"transaction_id":key,"account":"simulated","amount_minor":amount,"currency":"GBP","direction":"in","status":"settled","reference":p["bank_reference"],"settled_at":format!("{day}T12:00:00+00:00"),"updated_at":format!("{day}T12:00:00+00:00")}),&owner).unwrap();
  if amount==400{s.allocate_transfer(t["id"].as_i64().unwrap(),&json!({"payment_id":p["id"],"amount_minor":amount}),&owner).unwrap();}
 }
 let t=s.import_simulated_transfer(&json!({"transaction_id":"old-refund","account":"simulated","amount_minor":1500,"currency":"GBP","direction":"out","status":"settled","reference":"refund","settled_at":"2025-01-02T12:00:00+00:00","updated_at":"2025-01-02T12:00:00+00:00"}),&owner).unwrap();
 s.allocate_transfer(t["id"].as_i64().unwrap(),&json!({"payment_id":1,"amount_minor":1500}),&owner).unwrap();
 assert_eq!(s.db.query_row("SELECT count(*) FROM billing_event WHERE status='pending'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
 assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='bank'",[],|r|r.get::<_,i64>(0)).unwrap(),1500);
}
"#.replace("__DATABASE__", &database).replace("__MIGRATIONS__", &migrations));
    fs::write(&model, "{...}:{imports=[./publisher.nix];}").unwrap();
    run();
    compiled_probe(&output, &r#"
use openb2c_runtime::{Service,AuthContext,transitions};use serde_json::json;
#[test]fn reviewed_upgrade_posts_each_source_once(){
 let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();let dir=std::path::Path::new(__MIGRATIONS__);let db=rusqlite::Connection::open(__DATABASE__).unwrap();
 let plan=transitions::plan_at(&db,&model,Some(dir)).unwrap();assert_eq!(plan["requires_review"],true);
 transitions::approve_at(&db,&model,Some(dir),plan["plan_hash"].as_str().unwrap(),"Preserve posted bank cash and reconstruct allocated deposits").unwrap();drop(db);
 let s=Service::open_at(model,__DATABASE__,false,dir).unwrap();let owner=AuthContext::system();
 assert_eq!(s.db.query_row("SELECT code FROM ledger_account WHERE id=11",[],|r|r.get::<_,String>(0)).unwrap(),"owner_savings");
 assert_eq!(s.db.query_row("SELECT count(*) FROM journal WHERE source_key='cash:allocation:1'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
 assert_eq!(s.backoffice_post_billing_events(&json!({"date":"2025-01-03"}),&owner).unwrap()["posted"],5);
 assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='bank'",[],|r|r.get::<_,i64>(0)).unwrap(),1900);
 assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='customer_deposits'",[],|r|r.get::<_,i64>(0)).unwrap(),-400);
 let p=s.b2c_payment(2,&owner).unwrap();let t=s.import_simulated_transfer(&json!({"transaction_id":"complete-partial","account":"simulated","amount_minor":1100,"currency":"GBP","direction":"in","status":"settled","reference":p["bank_reference"],"settled_at":"2025-01-04T12:00:00+00:00","updated_at":"2025-01-04T12:00:00+00:00"}),&owner).unwrap();
 s.allocate_transfer(t["id"].as_i64().unwrap(),&json!({"payment_id":2,"amount_minor":1100}),&owner).unwrap();
 assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='bank'",[],|r|r.get::<_,i64>(0)).unwrap(),3000);
 assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='customer_deposits'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
 assert_eq!(s.backoffice_post_billing_events(&json!({"date":"2025-01-04"}),&owner).unwrap()["posted"],0);
}
"#.replace("__DATABASE__", &database).replace("__MIGRATIONS__", &migrations));

    fs::write(
        &model,
        "{lib,...}:{imports=[./publisher.nix];billing.payments.enable=lib.mkForce false;}",
    )
    .unwrap();
    run();
    compiled_probe(&output, &r#"
use openb2c_runtime::{Service,AuthContext,transitions};use serde_json::json;
#[test]fn opting_out_preserves_corrections_to_deposit_funded_invoices(){
 let model=serde_json::from_str(include_str!("../app/schema.json")).unwrap();let dir=std::path::Path::new(__MIGRATIONS__);let db=rusqlite::Connection::open(__DATABASE__).unwrap();
 let plan=transitions::plan_at(&db,&model,Some(dir)).unwrap();assert!(plan["blockers"].as_array().unwrap().is_empty(),"{plan}");
 transitions::approve_at(&db,&model,Some(dir),plan["plan_hash"].as_str().unwrap(),"Stop future cash integration while retaining invoice corrections").unwrap();drop(db);
 let s=Service::open_at(model,__DATABASE__,false,dir).unwrap();let owner=AuthContext::system();
 let t=s.import_simulated_transfer(&json!({"transaction_id":"refund-after-optout","account":"simulated","amount_minor":1500,"currency":"GBP","direction":"out","status":"settled","reference":"REFUND","settled_at":"2025-01-05T12:00:00+00:00","updated_at":"2025-01-05T12:00:00+00:00"}),&owner).unwrap();
 let input=json!({"payment_id":2,"amount_minor":1500});s.allocate_transfer(t["id"].as_i64().unwrap(),&input,&owner).unwrap();s.allocate_transfer(t["id"].as_i64().unwrap(),&input,&owner).unwrap();
 assert_eq!(s.b2c_payment(2,&owner).unwrap()["status"],"refunded");
 assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='bank'",[],|r|r.get::<_,i64>(0)).unwrap(),1500);
 let p=s.b2c_payment(3,&owner).unwrap();let input=json!({"transaction_id":"deferred","account":"simulated","amount_minor":1500,"currency":"GBP","direction":"in","status":"reversed","reference":p["bank_reference"],"settled_at":"2025-01-01T12:00:00+00:00","updated_at":"2025-01-06T12:00:00+00:00"});
 let reversed=s.import_simulated_transfer(&input,&owner).unwrap();assert!(reversed["review_reason"].is_null(),"{reversed}");s.import_simulated_transfer(&input,&owner).unwrap();
 assert_eq!(s.b2c_payment(3,&owner).unwrap()["status"],"refunded");
 for code in ["bank","customer_deposits"]{assert_eq!(s.db.query_row("SELECT sum(l.debit-l.credit) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code=?",[code],|r|r.get::<_,i64>(0)).unwrap(),0);}
}
"#.replace("__DATABASE__", &database).replace("__MIGRATIONS__", &migrations));
}
