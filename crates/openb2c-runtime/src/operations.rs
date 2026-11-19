//! Local operator commands. They require filesystem ownership, never a public recovery endpoint.
use crate::{AuthContext, Service, transitions};
use anyhow::{Context, Result, ensure};
use openb2c_schema::Schema;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path},
    time::Duration,
};

pub const USAGE: &str = "Usage: openb2c-app [COMMAND]\n  --create-api-key EMAIL [SCOPES]\n  --create-staff EMAIL SCOPES --password-stdin\n  --reset-password EMAIL --password-stdin\n  --disable-staff EMAIL\n  --backup NEW_BACKUP_FILE\n  --restore BACKUP_FILE SHA256  (DB_PATH and MIGRATIONS_DIR must be new paths)\n  --doctor\n  --plan-upgrade\n  --approve-upgrade PLAN_HASH REVIEW_REASON\n  --upgrade\nWith no command, serve REST. Stop the service before recovery, restore or upgrade.";
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn open_existing(path: &Path, write: bool) -> Result<Connection> {
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = Connection::open_with_flags(path, flags)
        .with_context(|| format!("Opening existing database {}", path.display()))?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("PRAGMA foreign_keys=ON")?;
    Ok(db)
}
fn reserve(path: &Path) -> Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("Destination must be a new file: {}", path.display()))
}
fn check_database(db: &Connection) -> Result<()> {
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(
        integrity == "ok",
        "SQLite integrity check failed: {integrity}"
    );
    let bad: i64 = db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
        r.get(0)
    })?;
    ensure!(bad == 0, "Backup contains {bad} broken foreign keys");
    Ok(())
}
fn migration_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().is_some_and(|e| e == "sql") {
            let name = p
                .file_name()
                .context("Missing migration filename")?
                .to_str()
                .context("Migration filename must be UTF-8")?
                .to_owned();
            files.insert(name, fs::read_to_string(&p)?);
        }
    }
    Ok(files)
}
pub fn backup(db: &Connection, target: &Path, migrations: &Path) -> Result<Value> {
    let file = reserve(target)?;
    let outcome = (|| -> Result<Value> {
        // SQLite takes a consistent snapshot, including committed WAL changes.
        db.execute(
            "VACUUM main INTO ?",
            [target.to_str().context("Backup path must be UTF-8")?],
        )?;
        let copy = Connection::open(target)?;
        check_database(&copy)?;
        let model =
            transitions::stored_model(&copy)?.context("Database has no application model")?;
        let files = migration_files(migrations)?;
        for (name, sql) in &files {
            let id = name.trim_end_matches(".sql");
            let applied: Option<String> = copy
                .query_row(
                    "SELECT checksum FROM openb2c_migration WHERE id=?",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(applied) = applied {
                ensure!(
                    applied == crate::migrations::checksum(sql),
                    "Applied migration {name} differs from its source file"
                );
            }
        }
        let manifest = json!({"format":1,"created_at":chrono::Utc::now().to_rfc3339(),"model_hash":transitions::model_hash(&model),"migrations":files});
        copy.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_backup_manifest(id INTEGER PRIMARY KEY CHECK(id=1),manifest_json TEXT NOT NULL)")?;
        copy.execute("INSERT INTO openb2c_backup_manifest VALUES (1,?) ON CONFLICT(id) DO UPDATE SET manifest_json=excluded.manifest_json",[manifest.to_string()])?;
        copy.close().map_err(|(_, e)| e)?;
        file.sync_all()?;
        Ok(
            json!({"backup":target,"sha256":hash(&fs::read(target)?),"model_hash":manifest["model_hash"],"migration_files":files.len()}),
        )
    })();
    if outcome.is_err() {
        drop(file);
        let _ = fs::remove_file(target);
    }
    outcome
}
pub fn restore(
    source: &Path,
    expected_sha256: &str,
    target: &Path,
    migrations: &Path,
) -> Result<Value> {
    ensure!(
        hash(&fs::read(source)?) == expected_sha256,
        "Backup SHA-256 does not match the recorded value"
    );
    let input = open_existing(source, false)?;
    check_database(&input)?;
    let manifest: String = input
        .query_row(
            "SELECT manifest_json FROM openb2c_backup_manifest WHERE id=1",
            [],
            |r| r.get(0),
        )
        .context("Backup manifest is missing")?;
    let manifest: Value = serde_json::from_str(&manifest)?;
    ensure!(manifest["format"] == 1, "Unsupported backup format");
    let model = transitions::stored_model(&input)?.context("Backup application model missing")?;
    ensure!(
        manifest["model_hash"] == transitions::model_hash(&model),
        "Backup model fingerprint mismatch"
    );
    let files: BTreeMap<String, String> = serde_json::from_value(manifest["migrations"].clone())?;
    for name in files.keys() {
        let p = Path::new(name);
        ensure!(
            p.components().count() == 1
                && matches!(p.components().next(), Some(Component::Normal(_)))
                && p.extension().is_some_and(|x| x == "sql"),
            "Invalid migration filename in backup"
        );
    }
    ensure!(
        !migrations.exists(),
        "Restore migrations directory must be a new path"
    );
    let file = reserve(target)?;
    let mut made_dir = false;
    let mut written = vec![];
    let result = (|| -> Result<Value> {
        input.execute(
            "VACUUM main INTO ?",
            [target.to_str().context("Restore path must be UTF-8")?],
        )?;
        fs::create_dir(migrations)?;
        made_dir = true;
        for (name, source) in &files {
            let path = migrations.join(name);
            let mut f = reserve(&path)?;
            written.push(path);
            f.write_all(source.as_bytes())?;
            f.sync_all()?;
        }
        file.sync_all()?;
        let db = open_existing(target, false)?;
        check_database(&db)?;
        Ok(
            json!({"restored":target,"migrations":migrations,"model_hash":manifest["model_hash"],"requires_upgrade_review":true}),
        )
    })();
    if result.is_err() {
        drop(file);
        for p in written {
            let _ = fs::remove_file(p);
        }
        if made_dir {
            let _ = fs::remove_dir(migrations);
        }
        let _ = fs::remove_file(target);
    }
    result
}
pub fn doctor(db: &Connection, target: &Schema) -> Result<Value> {
    check_database(db)?;
    let model = transitions::stored_model(db)?.context("Application model is missing")?;
    let migrations: i64 =
        db.query_row("SELECT count(*) FROM openb2c_migration", [], |r| r.get(0))?;
    let pending = if let Some(t) = openb2c_schema::capabilities::entity(&model, "billing", "event")
    {
        db.query_row(
            &format!(
                "SELECT count(*) FROM {} WHERE status='pending'",
                openb2c_schema::sql::quote(t)
            ),
            [],
            |r| r.get::<_, i64>(0),
        )?
    } else {
        0
    };
    Ok(
        json!({"integrity":"ok","foreign_keys":"ok","stored_model_hash":transitions::model_hash(&model),"target_model_hash":transitions::model_hash(target),"model_matches":transitions::model_hash(&model)==transitions::model_hash(target),"migration_records":migrations,"unposted_billing_events":pending}),
    )
}
fn password_stdin() -> Result<String> {
    let mut password = String::new();
    std::io::stdin().take(515).read_to_string(&mut password)?;
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    ensure!(password.len() <= 512, "Password input is too long");
    Ok(password)
}
pub fn command(model: &str, root: &Path, args: &[String]) -> Result<bool> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(false);
    };
    if ![
        "--backup",
        "--restore",
        "--doctor",
        "--plan-upgrade",
        "--approve-upgrade",
        "--upgrade",
        "--create-staff",
        "--reset-password",
        "--disable-staff",
    ]
    .contains(&command)
    {
        return Ok(false);
    }
    let schema: Schema = serde_json::from_str(model)?;
    let path = std::path::PathBuf::from(crate::database_path(&schema));
    let migrations = std::env::var_os("MIGRATIONS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("migrations"));
    let output = match (command, args.len()) {
        ("--backup", 2) => backup(
            &open_existing(&path, false)?,
            Path::new(&args[1]),
            &migrations,
        )?,
        ("--restore", 3) => restore(Path::new(&args[1]), &args[2], &path, &migrations)?,
        ("--doctor", 1) => doctor(&open_existing(&path, false)?, &schema)?,
        ("--plan-upgrade", 1) => {
            transitions::plan_at(&open_existing(&path, false)?, &schema, Some(&migrations))?
        }
        ("--approve-upgrade", 3) => transitions::approve_at(
            &open_existing(&path, true)?,
            &schema,
            Some(&migrations),
            &args[1],
            &args[2],
        )?,
        ("--upgrade", 1) => {
            let service = Service::open_at(schema, &path, false, &migrations)?;
            doctor(&service.db, &service.schema)?
        }
        ("--create-staff", 4) if args[3] == "--password-stdin" => {
            let password = password_stdin()?;
            let service = Service::open_at(schema, &path, false, &migrations)?;
            service.provision_staff(&args[1], &password, &args[2], &AuthContext::system())?
        }
        ("--reset-password", 3) if args[2] == "--password-stdin" => {
            let password = password_stdin()?;
            let service = Service::open_at(schema, &path, false, &migrations)?;
            service.recover_password(&args[1], &password, &AuthContext::system())?
        }
        ("--disable-staff", 2) => {
            let service = Service::open_at(schema, &path, false, &migrations)?;
            service.disable_staff(&args[1], &AuthContext::system())?
        }
        _ => anyhow::bail!("Invalid operator command arguments. {USAGE}"),
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(true)
}
