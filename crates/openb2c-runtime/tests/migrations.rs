mod common;
use openb2c_runtime::{AuthContext, Service, migrations};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "openb2c-migrations-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn numbered_migrations_apply_atomically_and_checksums_are_immutable() {
    let temp = Temp::new();
    let db = temp.0.join("app.sqlite");
    let dir = temp.0.join("migrations");
    fs::create_dir(&dir).unwrap();
    let service = Service::open_at(common::schema(), &db, true, &dir).unwrap();
    drop(service);
    fs::write(dir.join("001_note.sql"),"CREATE TABLE operator_note(id INTEGER PRIMARY KEY,note TEXT); INSERT INTO operator_note VALUES (1,'preserved; semicolon');").unwrap();
    let service = Service::open_at(common::schema(), &db, true, &dir).unwrap();
    let note: String = service
        .db
        .query_row("SELECT note FROM operator_note", [], |r| r.get(0))
        .unwrap();
    assert_eq!(note, "preserved; semicolon");
    drop(service);
    fs::write(dir.join("001_note.sql"), "SELECT 1;").unwrap();
    assert!(Service::open_at(common::schema(), &db, true, &dir).is_err());
}
#[test]
fn fresh_baselines_include_old_migrations_and_existing_data_survives_reopens() {
    let temp = Temp::new();
    let dir = temp.0.join("migrations");
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("001_historical.sql"),
        "ALTER TABLE absent ADD COLUMN impossible TEXT;",
    )
    .unwrap();
    let db = temp.0.join("app.sqlite");
    let service = Service::open_at(common::schema(), &db, true, &dir).unwrap();
    service
        .mutate(
            "production",
            &json!(1),
            "update",
            &json!({"name":"Preserved name"}),
            &AuthContext::system(),
            None,
        )
        .unwrap();
    drop(service);
    let service = Service::open_at(common::schema(), &db, true, &dir).unwrap();
    assert_eq!(
        service.raw("production", &json!(1)).unwrap()["name"],
        "Preserved name"
    );
}
#[test]
fn additive_columns_are_safe_and_unsafe_changes_are_refused() {
    let temp = Temp::new();
    let db = temp.0.join("app.sqlite");
    let service =
        Service::open_at(common::schema(), &db, true, &temp.0.join("migrations")).unwrap();
    drop(service);
    let mut schema = common::schema();
    schema
        .tables
        .get_mut("production")
        .unwrap()
        .insert("optional_note".into(), openb2c_schema::Column::default());
    let service = Service::open_at(schema.clone(), &db, false, &temp.0.join("migrations")).unwrap();
    assert!(service.raw("production", &json!(1)).unwrap()["optional_note"].is_null());
    drop(service);
    schema
        .tables
        .get_mut("production")
        .unwrap()
        .get_mut("optional_note")
        .unwrap()
        .required = true;
    assert!(Service::open_at(schema, &db, false, &temp.0.join("migrations")).is_err());
    assert_eq!(migrations::checksum("abc"), "1a47e90b");
}
#[test]
fn migration_plans_preserve_destructive_changes_for_manual_review() {
    let old = common::schema();
    let mut new = old.clone();
    new.tables
        .get_mut("production")
        .unwrap()
        .remove("description");
    let plan = openb2c_schema::migration::plan(&old, &new).unwrap();
    assert!(plan.steps.iter().any(|s| s.kind == "manual"));
    assert!(!openb2c_schema::migration::stub(&plan).contains("DROP TABLE"));
}
#[test]
fn concurrent_checkouts_on_separate_connections_cannot_oversell() {
    let temp = Temp::new();
    let db = temp.0.join("app.sqlite");
    let a = Service::open(common::schema(), &db, true).unwrap();
    let b = Service::open(common::schema(), &db, true).unwrap();
    a.db.execute("UPDATE venue SET capacity=1 WHERE id=1", [])
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs = [a, b]
        .into_iter()
        .enumerate()
        .map(|(i, service)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let auth = AuthContext {
                    user_id: Some(i as i64 + 1),
                    scopes: service.self_scopes(),
                };
                service
                    .checkout(&json!({"items":[{"item_id":1,"quantity":1}]}), &auth)
                    .is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        jobs.into_iter()
            .map(|j| j.join().unwrap())
            .filter(|v| *v)
            .count(),
        1
    );
    let service = Service::open(common::schema(), &db, false).unwrap();
    let count: i64 = service
        .db
        .query_row("SELECT count(*) FROM ticket", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let _: Value = json!({});
}

#[test]
fn existing_typescript_databases_upgrade_without_losing_records_or_history() {
    let temp = Temp::new();
    let path = temp.0.join("legacy.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    let baseline: Value = serde_json::from_str(include_str!(
        "../../openb2c/tests/fixtures/duchy-typescript-baseline.json"
    ))
    .unwrap();
    for field in ["sql", "reference", "fixtures"] {
        db.execute_batch(baseline[field].as_str().unwrap()).unwrap();
    }
    db.execute_batch("CREATE TABLE openb2c_migration(id TEXT PRIMARY KEY,checksum TEXT NOT NULL,description TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); INSERT INTO openb2c_migration(id,checksum,description) VALUES ('001_legacy','12345678','legacy history'); UPDATE production SET name='Production kept from legacy' WHERE id=1;").unwrap();
    drop(db);
    let service =
        Service::open_at(common::schema(), &path, false, &temp.0.join("migrations")).unwrap();
    assert_eq!(
        service.raw("production", &json!(1)).unwrap()["name"],
        "Production kept from legacy"
    );
    let checksum: String = service
        .db
        .query_row(
            "SELECT checksum FROM openb2c_migration WHERE id='001_legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(checksum, "12345678");
}
#[test]
fn noop_migrations_cannot_silently_leave_wrong_check_constraints() {
    let temp = Temp::new();
    let path = temp.0.join("changes.sqlite");
    let dir = temp.0.join("migrations");
    fs::create_dir(&dir).unwrap();
    let mut model = common::schema();
    let service = Service::open_at(model.clone(), &path, true, &dir).unwrap();
    drop(service);
    fs::write(dir.join("001_noop.sql"), "SELECT 1;").unwrap();
    model
        .tables
        .get_mut("venue")
        .unwrap()
        .get_mut("name")
        .unwrap()
        .constraints
        .length = Some(openb2c_schema::Bounds {
        min: Some(1.0),
        max: Some(300.0),
    });
    assert!(Service::open_at(model, &path, false, &dir).is_err());
}
