// Reference execution begins.
#[cfg(feature = "reference")]
#[path = "reference/service.rs"]
mod reference;
// Reference execution ends.
#[cfg(feature = "reference")]
use openb2c_schema::sql;
use openb2c_schema::*;
use rusqlite::{
    Connection, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, path::Path, time::Duration};

pub type Result<T> = std::result::Result<T, ServiceError>;
thread_local! {static AUDIT_SOURCE:std::cell::Cell<&'static str>=const{std::cell::Cell::new("service")};}
pub(crate) struct SourceScope(&'static str);
impl SourceScope {
    pub(crate) fn enter(source: &'static str) -> Self {
        Self(AUDIT_SOURCE.replace(source))
    }
}
impl Drop for SourceScope {
    fn drop(&mut self) {
        AUDIT_SOURCE.set(self.0);
    }
}
fn source() -> &'static str {
    AUDIT_SOURCE.get()
}
static EMAIL_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").expect("static email regex")
});
static PHONE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(\+44|0)[0-9]{10,11}$").expect("static phone regex")
});
static POSTCODE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)^[A-Z]{1,2}[0-9][0-9A-Z]?\s?[0-9][A-Z]{2}$")
        .expect("static postcode regex")
});

/// Nested operations use savepoints; the outer writer retains one immediate transaction.
pub(crate) struct Atomic<'a> {
    db: &'a Connection,
    savepoint: Option<String>,
    finished: bool,
}
impl Atomic<'_> {
    pub(crate) fn finish(mut self) -> std::result::Result<(), rusqlite::Error> {
        self.db.execute_batch(
            &self
                .savepoint
                .as_ref()
                .map(|n| format!("RELEASE SAVEPOINT {n}"))
                .unwrap_or_else(|| "COMMIT".into()),
        )?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for Atomic<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let sql = self
                .savepoint
                .as_ref()
                .map(|n| format!("ROLLBACK TO SAVEPOINT {n}; RELEASE SAVEPOINT {n}"))
                .unwrap_or_else(|| "ROLLBACK".into());
            let _ = self.db.execute_batch(&sql);
        }
    }
}
pub(crate) fn transaction(db: &Connection) -> std::result::Result<Atomic<'_>, rusqlite::Error> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let savepoint = if db.is_autocommit() {
        db.execute_batch("BEGIN IMMEDIATE")?;
        None
    } else {
        let name = format!(
            "openb2c_{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        db.execute_batch(&format!("SAVEPOINT {name}"))?;
        Some(name)
    };
    Ok(Atomic {
        db,
        savepoint,
        finished: false,
    })
}

thread_local! {static REQUEST_DEADLINE: std::cell::Cell<Option<std::time::Instant>> = const {std::cell::Cell::new(None)};}
pub(crate) struct Deadline(Option<std::time::Instant>);
impl Deadline {
    pub fn enter(duration: Duration) -> Self {
        Self(REQUEST_DEADLINE.replace(Some(std::time::Instant::now() + duration)))
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        REQUEST_DEADLINE.set(self.0);
    }
}
pub(crate) fn check_deadline() -> Result<()> {
    if REQUEST_DEADLINE
        .get()
        .is_some_and(|t| std::time::Instant::now() >= t)
    {
        Err(ServiceError::new("timeout", "Request deadline exceeded"))
    } else {
        Ok(())
    }
}
pub(crate) fn commit(tx: Atomic<'_>) -> Result<()> {
    check_deadline()?;
    tx.finish()?;
    Ok(())
}

#[derive(Debug)]
pub struct ServiceError {
    pub code: &'static str,
    pub message: String,
}
impl ServiceError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn status(&self) -> u16 {
        match self.code {
            "unauthorized" => 401,
            "forbidden" => 403,
            "not_found" => 404,
            "conflict" | "bad_state" => 409,
            "invalid" => 422,
            "malformed" => 400,
            "rate_limited" => 429,
            "unsupported_media_type" => 415,
            "payload_too_large" => 413,
            "timeout" => 504,
            "unavailable" => 503,
            "unsupported_version" => 400,
            "unsupported" => 501,
            _ => 500,
        }
    }
}
impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ServiceError {}
impl From<rusqlite::Error> for ServiceError {
    fn from(error: rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::SqliteFailure(ref e, _)
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Self::new("conflict", "Record violates a schema constraint")
            }
            _ => {
                eprintln!(
                    "{}",
                    json!({"level":"error", "message":"Database operation failed", "error":error.to_string()})
                );
                Self::new("internal_error", "Database operation failed")
            }
        }
    }
}
impl From<anyhow::Error> for ServiceError {
    fn from(error: anyhow::Error) -> Self {
        Self::new("invalid", error.to_string())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthContext {
    pub user_id: Option<i64>,
    pub scopes: Vec<String>,
}
impl AuthContext {
    pub fn system() -> Self {
        Self {
            user_id: None,
            scopes: vec!["*".into()],
        }
    }
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == "*" || s == scope)
    }
}

#[derive(Clone, Debug)]
pub struct ListOptions {
    pub limit: u32,
    pub offset: u32,
    pub sort: Option<String>,
    pub descending: bool,
    pub filter: BTreeMap<String, Value>,
}
impl Default for ListOptions {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            sort: None,
            descending: false,
            filter: BTreeMap::new(),
        }
    }
}

pub struct Service {
    pub schema: Schema,
    pub db: Connection,
}

impl Service {
    pub fn open(schema: Schema, path: impl AsRef<Path>, fixtures: bool) -> anyhow::Result<Self> {
        Self::open_at(
            schema,
            path,
            fixtures,
            &std::env::var_os("MIGRATIONS_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("migrations")),
        )
    }

    pub fn open_at(
        schema: Schema,
        path: impl AsRef<Path>,
        fixtures: bool,
        migrations: &Path,
    ) -> anyhow::Result<Self> {
        #[cfg(not(feature = "reference"))]
        crate::composition::check_model(&schema)?;
        #[cfg(feature = "reference")]
        let errors = validate::validate(&schema);
        #[cfg(feature = "reference")]
        anyhow::ensure!(errors.is_empty(), "Invalid schema:\n{}", errors.join("\n"));
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let service = Self { schema, db };
        let tx = crate::service::transaction(&service.db)?;
        let old = crate::transitions::stored_model(&service.db)?;
        crate::migrations::run(&service.db, &service.schema, Some(migrations))?;
        service.bootstrap(fixtures)?;
        service.complete_transition(old.as_ref())?;
        crate::service::commit(tx)?;
        Ok(service)
    }

    fn bootstrap(&self, fixtures: bool) -> anyhow::Result<()> {
        let transaction = crate::service::transaction(&self.db)?;
        self.effect_tables()?;
        self.db.execute_batch(
            "CREATE TABLE IF NOT EXISTS openb2c_rust_audit (
            id INTEGER PRIMARY KEY, entity TEXT NOT NULL, record_id TEXT, operation TEXT NOT NULL,
            actor_user_id INTEGER, before_json TEXT, after_json TEXT,
            metadata_json TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
            CREATE TABLE IF NOT EXISTS openb2c_rust_outbox (
            id INTEGER PRIMARY KEY, entity TEXT NOT NULL, record_id TEXT, operation TEXT NOT NULL,
            effect_json TEXT NOT NULL, data_json TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
        )?;
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_audit_log (id INTEGER PRIMARY KEY AUTOINCREMENT,entity TEXT NOT NULL,action TEXT NOT NULL,record_id INTEGER,category TEXT NOT NULL,reason TEXT,actor_user_id INTEGER,source TEXT NOT NULL,result_json TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
        #[cfg(feature = "reference")]
        self.db
            .execute_batch(&sql::seed_sql(&self.schema, false)?)?;
        #[cfg(not(feature = "reference"))]
        self.db.execute_batch(crate::composition::SEED_SQL)?;
        if fixtures {
            #[cfg(feature = "reference")]
            self.db.execute_batch(&sql::seed_sql(&self.schema, true)?)?;
            #[cfg(not(feature = "reference"))]
            self.db.execute_batch(crate::composition::FIXTURES_SQL)?;
        }
        crate::service::commit(transaction)?;
        Ok(())
    }

    /// Serialize inspection/operational output; application storage uses concrete Record mappings.
    pub fn query(&self, sql: &str, values: &[SqlValue]) -> Result<Vec<Value>> {
        let mut stmt = self.db.prepare(sql)?;
        let fields: Vec<_> = stmt
            .column_names()
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let rows = stmt.query_map(params_from_iter(values), |r| {
            let mut row = Map::new();
            for (index, field) in fields.iter().enumerate() {
                let value = match r.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => json!(v),
                    ValueRef::Real(v) => json!(v),
                    ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                    ValueRef::Blob(v) => json!(v),
                };
                row.insert(field.clone(), value);
            }
            Ok(Value::Object(row))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub(crate) fn record(
        &self,
        table: &str,
        id: &Value,
        action: &str,
        auth: &AuthContext,
        change: (Option<&Value>, Option<&Value>),
        operation: &Operation,
    ) -> Result<()> {
        let (before, after) = change;
        let redact = |row: Option<&Value>| {
            row.map(|r| {
                let mut row = r.clone();
                self.redact(table, &mut row);
                row.to_string()
            })
        };
        self.db.execute("INSERT INTO openb2c_rust_audit (entity, record_id, operation, actor_user_id, before_json, after_json, metadata_json) VALUES (?, ?, ?, ?, ?, ?, ?)", rusqlite::params![table, id.to_string(), action, auth.user_id, redact(before), redact(after), operation.audit.to_string()])?;
        let (required, category, reason) = self.audit_defaults(table, action);
        if operation.audit["required"] == true || required {
            self.db.execute("INSERT INTO openb2c_audit_log(entity,action,record_id,category,reason,actor_user_id,source,result_json) VALUES (?,?,?,?,?,?,?,?)",rusqlite::params![table,action,id.as_i64(),operation.audit["category"].as_str().or(category).unwrap_or("data"),operation.audit["reason"].as_str().or(reason),auth.user_id,source(),redact(after).unwrap_or_else(||"{\"deleted\":true}".into())])?;
        }
        for effect in &operation.effects {
            self.db.execute("INSERT INTO openb2c_rust_outbox (entity, record_id, operation, effect_json, data_json) VALUES (?, ?, ?, ?, ?)", rusqlite::params![table, id.to_string(), action, effect.to_string(), redact(after).unwrap_or_else(|| "null".into())])?;
        }
        Ok(())
    }

    /// Read existing bcrypt bearer credentials; native credential issuance is separate.
    pub fn authenticate(&self, token: &str) -> Result<AuthContext> {
        if token.starts_with("pwd_") {
            return self.authenticate_password_session(token);
        }
        if token.starts_with("sess_") && self.model_has_table("identity_session") {
            let prefix: String = token.chars().take(16).collect();
            let mut statement=self.db.prepare("SELECT id, user_id, token_hash FROM identity_session WHERE revoked = 0 AND token_prefix = ? AND julianday(expires_at) > julianday('now')")?;
            let rows = statement
                .query_map([prefix], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, user_id, token_hash) in rows {
                if bcrypt::verify(token, &token_hash).unwrap_or(false) {
                    self.db.execute(
                        "UPDATE identity_session SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?",
                        [id],
                    )?;
                    let scopes = self.self_scopes();
                    return Ok(AuthContext { user_id, scopes });
                }
            }
        } else if self.model_has_table("api_key") {
            let prefix: String = token.chars().take(11).collect();
            let mut statement=self.db.prepare("SELECT id, user_id, scopes, key_hash FROM api_key WHERE active = 1 AND key_prefix = ? AND (expires_at IS NULL OR julianday(expires_at) > julianday('now'))")?;
            let rows = statement
                .query_map([prefix], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, user_id, scopes, key_hash) in rows {
                if bcrypt::verify(token, &key_hash).unwrap_or(false) {
                    self.db.execute(
                        "UPDATE api_key SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?",
                        [id],
                    )?;
                    return Ok(AuthContext {
                        user_id,
                        scopes: scopes
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect(),
                    });
                }
            }
        }
        Err(ServiceError::new(
            "unauthorized",
            "Invalid or expired bearer credential",
        ))
    }
}

pub fn etag(table: &str, row: &Value) -> String {
    format!("\"{table}:{:x}\"", Sha256::digest(row.to_string()))
}

pub(crate) fn sql_value(value: &Value) -> Result<SqlValue> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(v) => SqlValue::Integer(i64::from(*v)),
        Value::Number(v) => {
            if let Some(v) = v.as_i64() {
                SqlValue::Integer(v)
            } else {
                SqlValue::Real(
                    v.as_f64()
                        .ok_or_else(|| ServiceError::new("invalid", "Number out of range"))?,
                )
            }
        }
        Value::String(v) => SqlValue::Text(v.clone()),
        Value::Array(v) if v.iter().all(|v| v.as_u64().is_some_and(|n| n < 256)) => {
            SqlValue::Blob(v.iter().map(|v| v.as_u64().unwrap_or(0) as u8).collect())
        }
        _ => {
            return Err(ServiceError::new(
                "invalid",
                "Expected scalar or byte array",
            ));
        }
    })
}

fn parse_instant(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            let text = text.replace(' ', "T");
            ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S%.f"]
                .into_iter()
                .find_map(|format| {
                    chrono::NaiveDateTime::parse_from_str(&text, format)
                        .ok()
                        .map(|t| t.and_utc())
                })
        })
}

#[cfg(not(feature = "reference"))]
pub(crate) fn validate_format(field: &str, format: &str, text: &str) -> Result<()> {
    let valid = match format {
        "email" => EMAIL_RE.is_match(text),
        "phone" => PHONE_RE.is_match(&text.split_whitespace().collect::<String>()),
        "postcode" => POSTCODE_RE.is_match(text),
        "date" => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok(),
        "time" => chrono::NaiveTime::parse_from_str(text, "%H:%M")
            .or_else(|_| chrono::NaiveTime::parse_from_str(text, "%H:%M:%S"))
            .is_ok(),
        "date-time" => parse_instant(text).is_some(),
        "url" | "uri" => reqwest::Url::parse(text).is_ok(),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(ServiceError::new(
            "invalid",
            format!("{field} has an invalid {format} format"),
        ))
    }
}

/// Date validation mechanism; the compiler supplies the selected field relationships.
#[cfg(not(feature = "reference"))]
pub(crate) fn validate_temporal_text(
    field: &str,
    date_only: bool,
    time_only: bool,
    paired_date: Option<&str>,
    paired_time: Option<&str>,
    has_paired_time: bool,
    text: &str,
) -> Result<()> {
    let instant = if time_only {
        let Some(date) = paired_date else {
            return Ok(());
        };
        parse_instant(&format!("{date}T{text}"))
    } else if date_only {
        let time = paired_time.unwrap_or("00:00");
        parse_instant(&format!("{text}T{time}"))
    } else {
        parse_instant(text)
    }
    .ok_or_else(|| ServiceError::new("invalid", format!("{field} must be a valid date/time")))?;
    let threshold = if date_only && !has_paired_time {
        chrono::Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight")
            .and_utc()
    } else {
        chrono::Utc::now()
    };
    if instant < threshold {
        Err(ServiceError::new(
            "invalid",
            format!("{field} must be in the future"),
        ))
    } else {
        Ok(())
    }
}
