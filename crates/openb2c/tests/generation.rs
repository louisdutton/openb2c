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

fn evaluate_duchy() -> Vec<u8> {
    let result = Command::new("nix")
        .args([
            "eval",
            "--json",
            "-f",
            "examples/duchyopera/composition.nix",
        ])
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
fn native_cli_generates_duchy_and_runtime_bootstraps_all_fixtures() {
    let input = evaluate_duchy();
    let temp = TempDir::new();
    let output = temp.0.join("app");
    let result = generate(&input, &output, &["--explain=json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(report["relationships"].as_array().unwrap().len() > 20);
    let schema: Schema =
        serde_json::from_slice(&fs::read(output.join("schema.json")).unwrap()).unwrap();
    let service = Service::open(schema, ":memory:", true).unwrap();
    let performances = service
        .list(
            "performance",
            &ListOptions::default(),
            &AuthContext::system(),
        )
        .unwrap();
    assert_eq!(performances["total"], 2);
    let production = service
        .find("production", &json!(1), &AuthContext::system())
        .unwrap();
    assert_eq!(production["name"], "The Magic Flute");
    assert!(output.join("support/runtime/src/web.rs").is_file());
    assert!(!output.join("server.ts").exists());
    let manifest = fs::read_to_string(output.join("support/runtime/Cargo.toml")).unwrap();
    assert!(!manifest.contains("workspace = true"));
    let json: Value =
        serde_json::from_slice(&fs::read(output.join("openapi.json")).unwrap()).unwrap();
    assert!(json["paths"]["/api/productions"].is_object());
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
    let input = evaluate_duchy();
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
