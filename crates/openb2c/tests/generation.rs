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
    assert_eq!(report["capabilities"].as_object().unwrap().len(), 10);
    assert!(report.get("surfaces").is_none());
    let schema: Schema =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    let service = Service::open(schema, ":memory:", true).unwrap();
    assert_eq!(service.schema.tables.len(), 15);
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
    assert!(evaluated.seed.reference.is_empty());
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
fn generated_models_compile_and_preserve_missing_null_and_default_semantics() {
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let model = json!({"tables":{"_1":{
        "id":{"type":"integer","pk":true,"auto":true},
        "type":{"type":"text"},
        "self":{"type":"text","required":true,"default":"'default'"}
    }}});
    let result = generate(model.to_string().as_bytes(), &output, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let probe = temp.0.join("probe");
    fs::create_dir(&probe).unwrap();
    fs::write(probe.join("Cargo.toml"), "[package]\nname=\"model-probe\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[dependencies]\nserde={version=\"1\",features=[\"derive\"]}\nserde_json=\"1\"\n[lib]\npath=\"probe.rs\"\n").unwrap();
    fs::write(
        probe.join("probe.rs"),
        r###"
#[allow(dead_code)]
#[path = "../app/src/models.rs"] mod models;
#[test]
fn roundtrip() {
    let missing: models::Entity1Patch = serde_json::from_str("{}").unwrap();
    let null: models::Entity1Patch = serde_json::from_str(r#"{"type":null}"#).unwrap();
    let present: models::Entity1Patch = serde_json::from_str(r#"{"type":"ticket"}"#).unwrap();
    assert_eq!(missing.type_, None);
    assert_eq!(null.type_, Some(None));
    assert_eq!(present.type_, Some(Some("ticket".into())));
    assert_eq!(serde_json::to_value(missing).unwrap(), serde_json::json!({}));
    assert_eq!(serde_json::to_value(null).unwrap(), serde_json::json!({"type":null}));
    let input: models::Entity1Input = serde_json::from_str("{}").unwrap();
    assert_eq!(serde_json::to_value(input).unwrap(), serde_json::json!({}));
}
"###,
    )
    .unwrap();
    let result = Command::new("cargo")
        .args(["test", "--offline", "--quiet", "--manifest-path"])
        .arg(probe.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
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
