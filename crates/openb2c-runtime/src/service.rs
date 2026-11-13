use openb2c_schema::{
    sql::{self, quote},
    *,
};
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

pub(crate) fn transaction(
    db: &Connection,
) -> std::result::Result<rusqlite::Transaction<'_>, rusqlite::Error> {
    rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)
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
pub(crate) fn commit(tx: rusqlite::Transaction<'_>) -> Result<()> {
    check_deadline()?;
    tx.commit()?;
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
        let errors = validate::validate(&schema);
        anyhow::ensure!(errors.is_empty(), "Invalid schema:\n{}", errors.join("\n"));
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        crate::migrations::run(&db, &schema, Some(migrations))?;
        let service = Self { schema, db };
        service.bootstrap(fixtures)?;
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
        self.db
            .execute_batch(&sql::seed_sql(&self.schema, false)?)?;
        if fixtures {
            self.db.execute_batch(&sql::seed_sql(&self.schema, true)?)?;
        }
        crate::service::commit(transaction)?;
        Ok(())
    }

    pub fn columns(&self, table: &str) -> Result<&BTreeMap<String, Column>> {
        self.schema
            .tables
            .get(table)
            .ok_or_else(|| ServiceError::new("not_found", "Unknown entity"))
    }

    pub fn primary_key(&self, table: &str) -> Result<&str> {
        self.columns(table)?
            .iter()
            .find(|(_, c)| c.pk)
            .map(|(f, _)| f.as_str())
            .ok_or_else(|| ServiceError::new("invalid", "Entity has no primary key"))
    }

    fn policy(&self, table: &str, action: &str) -> Result<Operation> {
        self.columns(table)?;
        // Nix normally supplies implicit CRUD. Missing policies remain private.
        Ok(self
            .schema
            .operations
            .get(table)
            .and_then(|ops| ops.get(action))
            .cloned()
            .unwrap_or_default())
    }

    pub fn authorize(
        &self,
        table: &str,
        action: &str,
        auth: &AuthContext,
        row: Option<&Value>,
    ) -> Result<()> {
        let policy = self.policy(table, action)?;
        if table.starts_with("identity_") {
            return Err(ServiceError::new(
                "forbidden",
                "Identity storage is managed through identity endpoints",
            ));
        }
        if policy.public || auth.has_scope("*") {
            return Ok(());
        }
        let scope = policy.scope.unwrap_or_else(|| format!("{table}.{action}"));
        let allowed = auth.has_scope(&scope)
            && (policy.relationships.is_empty()
                || auth.user_id.is_some_and(|id| {
                    row.is_none_or(|row| {
                        policy
                            .relationships
                            .iter()
                            .any(|r| row[&r.field.field].as_i64() == Some(id))
                    })
                }));
        if allowed {
            Ok(())
        } else {
            Err(ServiceError::new(
                if auth.user_id.is_some() {
                    "forbidden"
                } else {
                    "unauthorized"
                },
                format!("Not authorized to {action} {table}"),
            ))
        }
    }

    fn auth_filter(
        &self,
        table: &str,
        action: &str,
        auth: &AuthContext,
        clauses: &mut Vec<String>,
        values: &mut Vec<SqlValue>,
    ) -> Result<()> {
        self.authorize(table, action, auth, None)?;
        let policy = self.policy(table, action)?;
        if policy.public || auth.has_scope("*") || policy.relationships.is_empty() {
            return Ok(());
        }
        let mut fields = Vec::new();
        for rel in policy.relationships {
            fields.push(format!("{} = ?", quote(&rel.field.field)));
            values.push(SqlValue::Integer(auth.user_id.ok_or_else(|| {
                ServiceError::new("unauthorized", "Authentication required")
            })?));
        }
        clauses.push(format!("({})", fields.join(" OR ")));
        Ok(())
    }

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

    pub fn raw(&self, table: &str, id: &Value) -> Result<Value> {
        self.query(
            &format!(
                "SELECT * FROM {} WHERE {} = ?",
                quote(table),
                quote(self.primary_key(table)?)
            ),
            &[sql_value(id)?],
        )?
        .into_iter()
        .next()
        .ok_or_else(|| ServiceError::new("not_found", "Record not found"))
    }

    pub fn redact(&self, table: &str, row: &mut Value) {
        let Some(cols) = self.schema.tables.get(table) else {
            return;
        };
        let Some(row) = row.as_object_mut() else {
            return;
        };
        row.retain(|field, _| {
            cols.contains_key(field)
                || self
                    .schema
                    .derived
                    .get(table)
                    .is_some_and(|d| d.contains_key(field))
        });
        for (field, col) in cols {
            let inherent_secret =
                field.ends_with("_hash") || field == "signature" || field == "code_hash";
            if inherent_secret
                || col.metadata["redact"] == true
                || col.metadata["privacy"] == "secret"
            {
                row.remove(field);
            }
        }
        for (field, derived) in self.schema.derived.get(table).into_iter().flatten() {
            if derived.metadata["privacy"] == "secret" || derived.metadata["redact"] == true {
                row.remove(field);
            }
        }
    }

    pub(crate) fn derived(&self, table: &str, row: &mut Value) -> Result<()> {
        for (name, field) in self.schema.derived.get(table).into_iter().flatten() {
            let value = if let Some(expr) = &field.expression {
                self.evaluate(table, row, expr)?
            } else if let Some(template) = &field.template {
                let mut text = template.clone();
                for dependency in &field.dependencies {
                    let value = if dependency.table == table {
                        row[&dependency.field].clone()
                    } else {
                        self.related(table, row, &dependency.table, &dependency.field)?
                    };
                    text =
                        text.replace(&format!("{{{}}}", dependency.field), &display_value(&value));
                    text = text.replace(
                        &format!("{{{}.{}}}", dependency.table, dependency.field),
                        &display_value(&value),
                    );
                }
                Value::String(text)
            } else {
                return Err(ServiceError::new(
                    "invalid",
                    "Derived field lacks expression",
                ));
            };
            row[name] = value;
        }
        Ok(())
    }

    pub fn find(&self, table: &str, id: &Value, auth: &AuthContext) -> Result<Value> {
        self.authorize(table, "read", auth, None)?;
        let mut row = self.raw(table, id)?;
        self.authorize(table, "read", auth, Some(&row))?;
        self.derived(table, &mut row)?;
        self.redact(table, &mut row);
        Ok(row)
    }

    pub fn list(&self, table: &str, options: &ListOptions, auth: &AuthContext) -> Result<Value> {
        let columns = self.columns(table)?;
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        self.auth_filter(table, "read", auth, &mut clauses, &mut values)?;
        for (field, value) in &options.filter {
            let col = columns.get(field).ok_or_else(|| {
                ServiceError::new("invalid", format!("Unknown filter field {field}"))
            })?;
            if col.metadata["privacy"] == "secret"
                || col.metadata["redact"] == true
                || field.ends_with("_hash")
            {
                return Err(ServiceError::new("invalid", "Cannot filter secret field"));
            }
            clauses.push(format!("{} IS ?", quote(field)));
            values.push(sql_value(value)?);
        }
        let condition = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let total: i64 = self.db.query_row(
            &format!("SELECT count(*) FROM {}{condition}", quote(table)),
            params_from_iter(&values),
            |r| r.get(0),
        )?;
        let sort = options.sort.as_deref().unwrap_or(self.primary_key(table)?);
        if !columns.contains_key(sort) {
            return Err(ServiceError::new("invalid", "Unknown sort field"));
        }
        let col = &columns[sort];
        if col.metadata["privacy"] == "secret"
            || col.metadata["redact"] == true
            || sort.ends_with("_hash")
        {
            return Err(ServiceError::new("invalid", "Cannot sort secret field"));
        }
        let limit = options.limit.clamp(1, 1000);
        values.push(SqlValue::Integer(i64::from(limit)));
        values.push(SqlValue::Integer(i64::from(options.offset)));
        let mut items = self.query(
            &format!(
                "SELECT * FROM {}{condition} ORDER BY {} {} LIMIT ? OFFSET ?",
                quote(table),
                quote(sort),
                if options.descending { "DESC" } else { "ASC" }
            ),
            &values,
        )?;
        for row in &mut items {
            self.derived(table, row)?;
            self.redact(table, row);
        }
        Ok(json!({"items":items, "total":total, "limit":limit, "offset":options.offset}))
    }

    pub fn create(&self, table: &str, input: &Value, auth: &AuthContext) -> Result<Value> {
        if table == "api_key" {
            return self.create_api_key(input, auth);
        }
        let transaction = crate::service::transaction(&self.db)?;
        let policy = self.policy(table, "create")?;
        let mut data = input
            .as_object()
            .cloned()
            .ok_or_else(|| ServiceError::new("invalid", "Expected an object"))?;
        data.remove("created_at");
        data.remove("updated_at");
        if !policy.public && !auth.has_scope("*") {
            for rel in &policy.relationships {
                if let Some(id) = auth.user_id {
                    data.entry(rel.field.field.clone()).or_insert(json!(id));
                }
            }
        }
        self.apply_assignments(table, &mut data, &policy.set, input)?;
        self.authorize(table, "create", auth, Some(&Value::Object(data.clone())))?;
        self.validate_input(table, &data, true)?;
        self.validate_future(table, &data, None)?;
        let fields: Vec<_> = data.keys().map(|f| quote(f)).collect();
        let values = data.values().map(sql_value).collect::<Result<Vec<_>>>()?;
        if fields.is_empty() {
            self.db
                .execute(&format!("INSERT INTO {} DEFAULT VALUES", quote(table)), [])?;
        } else {
            self.db.execute(
                &format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    quote(table),
                    fields.join(", "),
                    vec!["?"; fields.len()].join(", ")
                ),
                params_from_iter(&values),
            )?;
        }
        let id = data
            .get(self.primary_key(table)?)
            .cloned()
            .unwrap_or_else(|| json!(self.db.last_insert_rowid()));
        let mut row = self.raw(table, &id)?;
        if let Some(guard) = &policy.guard
            && !truthy(&self.evaluate(table, &row, guard)?)
        {
            return Err(ServiceError::new(
                "bad_state",
                "Operation guard rejected record",
            ));
        }
        self.application_constraints(table, &row)?;
        self.record(table, &id, "create", auth, (None, Some(&row)), &policy)?;
        self.derived(table, &mut row)?;
        crate::service::commit(transaction)?;
        self.redact(table, &mut row);
        Ok(row)
    }

    pub fn mutate(
        &self,
        table: &str,
        id: &Value,
        action: &str,
        input: &Value,
        auth: &AuthContext,
        if_match: Option<&str>,
    ) -> Result<Value> {
        if ["read", "create"].contains(&action)
            || (!["update", "delete"].contains(&action)
                && self
                    .schema
                    .operations
                    .get(table)
                    .is_none_or(|ops| !ops.contains_key(action)))
        {
            return Err(ServiceError::new("not_found", "Unknown mutation operation"));
        }
        let transaction = crate::service::transaction(&self.db)?;
        self.authorize(table, action, auth, None)?;
        let old = self.raw(table, id)?;
        self.authorize(table, action, auth, Some(&old))?;
        if let Some(expected) = if_match {
            let mut visible = old.clone();
            self.derived(table, &mut visible)?;
            self.redact(table, &mut visible);
            let current = etag(table, &visible);
            if !expected
                .split(',')
                .map(str::trim)
                .any(|s| s == "*" || s == current)
            {
                return Err(ServiceError::new(
                    "conflict",
                    "Record has changed; refresh and retry",
                ));
            }
        }
        let policy = self.policy(table, action)?;
        if table == "api_key" && action == "update" {
            if ["key_hash", "key_prefix", "user_id"]
                .iter()
                .any(|f| input.get(f).is_some())
            {
                return Err(ServiceError::new(
                    "forbidden",
                    "Key credentials and owner are immutable",
                ));
            }
            if input["scopes"]
                .as_str()
                .is_some_and(|s| s.split(',').map(str::trim).any(|s| !auth.has_scope(s)))
            {
                return Err(ServiceError::new(
                    "forbidden",
                    "Cannot grant scopes you do not hold",
                ));
            }
        }
        if let Some(guard) = &policy.guard
            && !truthy(&self.evaluate(table, &old, guard)?)
        {
            return Err(ServiceError::new(
                "bad_state",
                "Operation guard rejected record",
            ));
        }
        for transition in &policy.transitions {
            if !transition
                .from
                .iter()
                .any(|v| old[&transition.field.field] == *v)
            {
                return Err(ServiceError::new(
                    "bad_state",
                    "Record is not in an allowed transition state",
                ));
            }
        }
        if action == "delete" {
            self.db.execute(
                &format!(
                    "DELETE FROM {} WHERE {} = ?",
                    quote(table),
                    quote(self.primary_key(table)?)
                ),
                [sql_value(id)?],
            )?;
            self.record(table, id, action, auth, (Some(&old), None), &policy)?;
            crate::service::commit(transaction)?;
            return Ok(json!({"id":id, "deleted":true}));
        }
        let mut changes = if action == "update" {
            input
                .as_object()
                .cloned()
                .ok_or_else(|| ServiceError::new("invalid", "Expected an object"))?
        } else {
            Map::new()
        };
        changes.remove("updated_at");
        changes.remove("created_at");
        self.validate_input(table, &changes, false)?;
        self.validate_future(table, &changes, Some(&old))?;
        let mut merged = old
            .as_object()
            .cloned()
            .ok_or_else(|| ServiceError::new("internal_error", "Invalid record"))?;
        merged.extend(changes.clone());
        self.apply_assignments(table, &mut merged, &policy.set, input)?;
        for field in policy.set.keys() {
            changes.insert(field.clone(), merged[field].clone());
        }
        self.authorize(table, action, auth, Some(&Value::Object(merged.clone())))?;
        self.validate_input(table, &changes, false)?;
        if !changes.is_empty() && self.columns(table)?.contains_key("updated_at") {
            changes.insert("updated_at".into(), json!(crate::identity::now()));
        }
        if !changes.is_empty() {
            let assignments = changes
                .keys()
                .map(|field| format!("{} = ?", quote(field)))
                .collect::<Vec<_>>();
            let mut values = changes
                .values()
                .map(sql_value)
                .collect::<Result<Vec<_>>>()?;
            values.push(sql_value(id)?);
            self.db.execute(
                &format!(
                    "UPDATE {} SET {} WHERE {} = ?",
                    quote(table),
                    assignments.join(", "),
                    quote(self.primary_key(table)?)
                ),
                params_from_iter(&values),
            )?;
        }
        let mut row = self.raw(table, id)?;
        self.application_constraints(table, &row)?;
        for cascade in &policy.cascade {
            self.cascade(table, id, cascade, input)?;
        }
        self.record(table, id, action, auth, (Some(&old), Some(&row)), &policy)?;
        self.derived(table, &mut row)?;
        crate::service::commit(transaction)?;
        self.redact(table, &mut row);
        Ok(row)
    }

    pub(crate) fn validate_input(
        &self,
        table: &str,
        data: &Map<String, Value>,
        create: bool,
    ) -> Result<()> {
        let cols = self.columns(table)?;
        for (field, value) in data {
            let col = cols
                .get(field)
                .ok_or_else(|| ServiceError::new("invalid", format!("Unknown field {field}")))?;
            if col.pk && (!create || col.auto) {
                return Err(ServiceError::new(
                    "invalid",
                    format!("{field} is generated or immutable"),
                ));
            }
            if value.is_null() {
                if col.required || col.pk {
                    return Err(ServiceError::new("invalid", format!("{field} is required")));
                }
                continue;
            }
            let valid = match col.kind {
                SqlType::Integer => value.is_i64(),
                SqlType::Real => value.is_number(),
                SqlType::Text => value.is_string(),
                SqlType::Blob => value
                    .as_array()
                    .is_some_and(|v| v.iter().all(|v| v.as_u64().is_some_and(|n| n < 256))),
            };
            if !valid {
                return Err(ServiceError::new(
                    "invalid",
                    format!("{field} has the wrong type"),
                ));
            }
            if let Some(text) = value.as_str() {
                let format = col.metadata["format"].as_str().unwrap_or(field);
                let matches = match format {
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
                if !matches {
                    return Err(ServiceError::new(
                        "invalid",
                        format!("{field} has an invalid {format} format"),
                    ));
                }
            }
            let rules = &col.constraints;
            if !rules.values.is_empty() && !rules.values.contains(&display_value(value)) {
                return Err(ServiceError::new(
                    "invalid",
                    format!("{field} must be one of {}", rules.values.join(", ")),
                ));
            }
            if let (Some(pattern), Some(text)) = (&rules.pattern, value.as_str()) {
                let pattern = regex::Regex::new(pattern).map_err(|_| {
                    ServiceError::new("invalid", format!("{field} has an unsupported pattern"))
                })?;
                if !pattern.is_match(text) {
                    return Err(ServiceError::new(
                        "invalid",
                        format!("{field} has an invalid format"),
                    ));
                }
            }
            for (bounds, number) in [
                (&rules.range, value.as_f64()),
                (
                    &rules.length,
                    value.as_str().map(|s| s.chars().count() as f64),
                ),
            ] {
                if let (Some(bounds), Some(number)) = (bounds, number)
                    && (bounds.min.is_some_and(|min| number < min)
                        || bounds.max.is_some_and(|max| number > max))
                {
                    return Err(ServiceError::new(
                        "invalid",
                        format!("{field} is outside its allowed bounds"),
                    ));
                }
            }
        }
        if create {
            for (field, col) in cols {
                if col.required && !col.auto && col.default.is_none() && !data.contains_key(field) {
                    return Err(ServiceError::new("invalid", format!("{field} is required")));
                }
            }
        }
        Ok(())
    }

    fn validate_future(
        &self,
        table: &str,
        data: &Map<String, Value>,
        current: Option<&Value>,
    ) -> Result<()> {
        let mut record = current
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        record.extend(data.clone());
        for (field, value) in data {
            if value.is_null() || ["created_at", "updated_at"].contains(&field.as_str()) {
                continue;
            }
            let col = &self.columns(table)?[field];
            let format = col.metadata["format"].as_str().unwrap_or("");
            if !["date", "time", "date-time"].contains(&format)
                && !field.ends_with("_at")
                && !field.ends_with("_date")
                && !field.ends_with("_time")
                && field != "date"
                && field != "time"
            {
                continue;
            }
            let text = value.as_str().unwrap_or("");
            let date_only = format == "date" || field == "date" || field.ends_with("_date");
            let time_only = format == "time" || field == "time" || field.ends_with("_time");
            let paired_date = if field == "time" {
                "date".into()
            } else {
                format!("{}_date", field.strip_suffix("_time").unwrap_or(field))
            };
            let paired_time = if field == "date" {
                "time".into()
            } else {
                format!("{}_time", field.strip_suffix("_date").unwrap_or(field))
            };
            let instant = if time_only {
                let Some(date) = record.get(&paired_date).and_then(Value::as_str) else {
                    continue;
                };
                parse_instant(&format!("{date}T{text}"))
            } else if date_only {
                let time = record
                    .get(&paired_time)
                    .and_then(Value::as_str)
                    .unwrap_or("00:00");
                parse_instant(&format!("{text}T{time}"))
            } else {
                parse_instant(text)
            }
            .ok_or_else(|| {
                ServiceError::new("invalid", format!("{field} must be a valid date/time"))
            })?;
            let threshold = if date_only && !record.contains_key(&paired_time) {
                chrono::Utc::now()
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight")
                    .and_utc()
            } else {
                chrono::Utc::now()
            };
            if instant < threshold {
                return Err(ServiceError::new(
                    "invalid",
                    format!("{field} must be in the future"),
                ));
            }
        }
        Ok(())
    }

    fn application_constraints(&self, table: &str, row: &Value) -> Result<()> {
        let Some(group) = self.schema.constraints.get(table) else {
            return Ok(());
        };
        for (name, constraint) in &group.check {
            if let Some(condition) = &constraint.condition
                && !truthy(&self.evaluate(table, row, condition)?)
            {
                continue;
            }
            if let Some(expr) = &constraint.expression
                && !truthy(&self.evaluate(table, row, expr)?)
            {
                return Err(ServiceError::new(
                    "invalid",
                    format!("{name} constraint failed"),
                ));
            }
        }
        for (name, constraint) in &group.unique {
            if constraint.enforcement != Enforcement::Application {
                continue;
            }
            if let Some(condition) = &constraint.condition
                && !truthy(&self.evaluate(table, row, condition)?)
            {
                continue;
            }
            let mut clauses = vec![format!("{} != ?", quote(self.primary_key(table)?))];
            let mut values = vec![sql_value(&row[self.primary_key(table)?])?];
            for field in &constraint.fields {
                clauses.push(format!("{} IS ?", quote(&field.field)));
                values.push(sql_value(&row[&field.field])?);
            }
            let count: i64 = self.db.query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE {}",
                    quote(table),
                    clauses.join(" AND ")
                ),
                params_from_iter(&values),
                |r| r.get(0),
            )?;
            if count > 0 {
                return Err(ServiceError::new(
                    "conflict",
                    format!("{name} must be unique"),
                ));
            }
        }
        Ok(())
    }

    fn apply_assignments(
        &self,
        table: &str,
        data: &mut Map<String, Value>,
        set: &Assignments,
        input: &Value,
    ) -> Result<()> {
        let original = Value::Object(data.clone());
        for (field, source) in set {
            let value = if let Some(value) = &source.value {
                value.clone()
            } else if source.clear == Some(true) {
                Value::Null
            } else if source.input == Some(true) {
                input.get(field).cloned().ok_or_else(|| {
                    ServiceError::new("invalid", format!("{field} input is required"))
                })?
            } else if source.now == Some(true) {
                json!(self.db.query_row(
                    "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                    [],
                    |r| r.get::<_, String>(0)
                )?)
            } else if let Some(expr) = &source.expression {
                self.evaluate(table, &original, expr)?
            } else {
                return Err(ServiceError::new("invalid", "Assignment has no source"));
            };
            data.insert(field.clone(), value);
        }
        Ok(())
    }

    fn related_rows(
        &self,
        table: &str,
        row: &Value,
        target: &str,
        via: Option<&str>,
    ) -> Result<Vec<Value>> {
        use openb2c_schema::inference::{fk, foreign_keys};
        self.columns(target)?;
        if let Some(bridge) = via {
            let local = fk(&self.schema, bridge, table)?;
            let remote = fk(&self.schema, bridge, target)?;
            return self.query(
                &format!(
                    "SELECT t.* FROM {} t JOIN {} b ON t.{}=b.{} WHERE b.{}=?",
                    quote(target),
                    quote(bridge),
                    quote(self.primary_key(target)?),
                    quote(&remote),
                    quote(&local)
                ),
                &[sql_value(&row[self.primary_key(table)?])?],
            );
        }
        let direct = foreign_keys(&self.schema, table)
            .into_iter()
            .filter(|(_, t, _)| *t == target)
            .collect::<Vec<_>>();
        if direct.len() == 1 {
            return self.query(
                &format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(target),
                    quote(direct[0].2)
                ),
                &[sql_value(&row[direct[0].0])?],
            );
        }
        let inverse = foreign_keys(&self.schema, target)
            .into_iter()
            .filter(|(_, t, _)| *t == table)
            .collect::<Vec<_>>();
        if inverse.len() == 1 {
            return self.query(
                &format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(target),
                    quote(inverse[0].0)
                ),
                &[sql_value(&row[inverse[0].2])?],
            );
        }
        Err(ServiceError::new(
            "invalid",
            format!("Ambiguous or missing FK from {table} to {target}; specify a junction"),
        ))
    }
    fn related(&self, table: &str, row: &Value, entity: &str, field: &str) -> Result<Value> {
        let fks: Vec<_> = self
            .columns(table)?
            .iter()
            .filter_map(|(name, col)| {
                col.references
                    .as_deref()
                    .and_then(reference)
                    .filter(|(target, _)| *target == entity)
                    .map(|(_, key)| (name, key))
            })
            .collect();
        if fks.len() != 1 {
            return Err(ServiceError::new(
                "invalid",
                format!("Ambiguous or missing FK from {table} to {entity}"),
            ));
        }
        let (fk, key) = fks[0];
        Ok(self
            .query(
                &format!(
                    "SELECT {} FROM {} WHERE {} = ?",
                    quote(field),
                    quote(entity),
                    quote(key)
                ),
                &[sql_value(&row[fk])?],
            )?
            .first()
            .map(|r| r[field].clone())
            .unwrap_or(Value::Null))
    }

    pub fn evaluate(&self, table: &str, row: &Value, expr: &Expr) -> Result<Value> {
        Ok(match expr {
            Expr::Field { name } => row[name].clone(),
            Expr::Rel { entity, field, via } => {
                let rows = self.related_rows(table, row, entity, via.as_deref())?;
                if rows.len() == 1 {
                    rows[0][field].clone()
                } else if rows.is_empty() {
                    Value::Null
                } else {
                    json!(rows.iter().map(|r| r[field].clone()).collect::<Vec<_>>())
                }
            }
            Expr::Lit { value } => value.clone(),
            Expr::Bin { op, left, right } => {
                let l = self.evaluate(table, row, left)?;
                if op == "&&" && !truthy(&l) {
                    return Ok(json!(false));
                }
                if op == "||" && truthy(&l) {
                    return Ok(json!(true));
                }
                let r = self.evaluate(table, row, right)?;
                match op.as_str() {
                    "==" => json!(l == r),
                    "!=" => json!(l != r),
                    "&&" => json!(truthy(&l) && truthy(&r)),
                    "||" => json!(truthy(&l) || truthy(&r)),
                    "<" | "<=" | ">" | ">=" => {
                        let ordering = if let (Some(l), Some(r)) = (l.as_i64(), r.as_i64()) {
                            Some(l.cmp(&r))
                        } else if let (Some(l), Some(r)) = (l.as_f64(), r.as_f64()) {
                            l.partial_cmp(&r)
                        } else if let (Some(l), Some(r)) = (l.as_str(), r.as_str()) {
                            Some(l.cmp(r))
                        } else {
                            None
                        };
                        let valid = ordering.is_some_and(|c| match op.as_str() {
                            "<" => c.is_lt(),
                            "<=" => c.is_le(),
                            ">" => c.is_gt(),
                            _ => c.is_ge(),
                        });
                        json!(valid)
                    }
                    "+" | "-" | "*" | "/" => {
                        if op != "/"
                            && let (Some(l), Some(r)) = (l.as_i64(), r.as_i64())
                        {
                            let value = match op.as_str() {
                                "+" => l.checked_add(r),
                                "-" => l.checked_sub(r),
                                _ => l.checked_mul(r),
                            }
                            .ok_or_else(|| {
                                ServiceError::new("invalid", "Integer arithmetic overflow")
                            })?;
                            return Ok(json!(value));
                        }
                        let l = l.as_f64().ok_or_else(|| {
                            ServiceError::new("invalid", "Arithmetic requires numeric values")
                        })?;
                        let r = r.as_f64().ok_or_else(|| {
                            ServiceError::new("invalid", "Arithmetic requires numeric values")
                        })?;
                        let value = match op.as_str() {
                            "+" => l + r,
                            "-" => l - r,
                            "*" => l * r,
                            _ => l / r,
                        };
                        if !value.is_finite() {
                            return Err(ServiceError::new(
                                "invalid",
                                "Arithmetic result is not finite",
                            ));
                        }
                        if value.fract() == 0.0
                            && value >= i64::MIN as f64
                            && value < i64::MAX as f64
                        {
                            json!(value as i64)
                        } else {
                            json!(value)
                        }
                    }
                    _ => {
                        return Err(ServiceError::new(
                            "unsupported",
                            "Unknown expression operator",
                        ));
                    }
                }
            }
            Expr::Un { op, arg } => {
                let value = self.evaluate(table, row, arg)?;
                match op.as_str() {
                    "!" => json!(!truthy(&value)),
                    "isNull" => json!(value.is_null()),
                    "notNull" => json!(!value.is_null()),
                    "-" => json!(-value.as_f64().ok_or_else(|| ServiceError::new(
                        "invalid",
                        "Negation requires a number"
                    ))?),
                    _ => return Err(ServiceError::new("unsupported", "Unknown unary operator")),
                }
            }
            Expr::Agg {
                op,
                rel,
                field,
                pred,
            } => {
                let entity = rel["table"]
                    .as_str()
                    .or_else(|| rel["entity"].as_str())
                    .ok_or_else(|| {
                        ServiceError::new("invalid", "Aggregate requires a related entity")
                    })?;
                let rows = self.related_rows(table, row, entity, rel["via"].as_str())?;
                match op.as_str() {
                    "count" => json!(rows.len()),
                    "sum" => {
                        let field = field
                            .as_deref()
                            .ok_or_else(|| ServiceError::new("invalid", "Sum requires a field"))?;
                        let mut total = json!(0);
                        for row in rows {
                            if row[field].is_null() {
                                continue;
                            }
                            total = self.evaluate(
                                table,
                                &json!({}),
                                &Expr::Bin {
                                    op: "+".into(),
                                    left: Box::new(Expr::Lit { value: total }),
                                    right: Box::new(Expr::Lit {
                                        value: row[field].clone(),
                                    }),
                                },
                            )?;
                        }
                        total
                    }
                    "any" | "all" => {
                        let pred = pred.as_ref().ok_or_else(|| {
                            ServiceError::new("invalid", "Aggregate requires a predicate")
                        })?;
                        let values = rows
                            .iter()
                            .map(|r| self.evaluate(entity, r, pred).map(|v| truthy(&v)))
                            .collect::<Result<Vec<_>>>()?;
                        json!(if op == "all" {
                            values.iter().all(|v| *v)
                        } else {
                            values.iter().any(|v| *v)
                        })
                    }
                    _ => return Err(ServiceError::new("invalid", "Unknown aggregate operator")),
                }
            }
        })
    }

    fn cascade(&self, table: &str, id: &Value, cascade: &Cascade, input: &Value) -> Result<()> {
        let target = &cascade.entity.table;
        let parent = self.raw(table, id)?;
        let rows = if let Some(via) = &cascade.via {
            if let Some(field) = via["field"].as_str() {
                self.query(
                    &format!("SELECT * FROM {} WHERE {}=?", quote(target), quote(field)),
                    &[sql_value(id)?],
                )?
            } else {
                self.related_rows(table, &parent, target, via["table"].as_str())?
            }
        } else {
            self.related_rows(table, &parent, target, None)?
        };
        for row in rows {
            let mut merged = row
                .as_object()
                .cloned()
                .ok_or_else(|| ServiceError::new("internal_error", "Invalid cascade record"))?;
            self.apply_assignments(target, &mut merged, &cascade.set, input)?;
            let changes: Map<_, _> = cascade
                .set
                .keys()
                .map(|f| (f.clone(), merged[f].clone()))
                .collect();
            self.validate_input(target, &changes, false)?;
            if changes.is_empty() {
                continue;
            }
            let mut values = changes
                .values()
                .map(sql_value)
                .collect::<Result<Vec<_>>>()?;
            values.push(sql_value(&row[self.primary_key(target)?])?);
            let assignments = changes
                .keys()
                .map(|f| format!("{} = ?", quote(f)))
                .collect::<Vec<_>>();
            self.db.execute(
                &format!(
                    "UPDATE {} SET {} WHERE {} = ?",
                    quote(target),
                    assignments.join(", "),
                    quote(self.primary_key(target)?)
                ),
                params_from_iter(&values),
            )?;
            self.application_constraints(target, &Value::Object(merged))?;
        }
        Ok(())
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
        let entity_audit =
            &self.schema.metadata.get("audit").unwrap_or(&Value::Null)["entities"][table];
        if operation.audit["required"] == true
            || entity_audit["operations"]
                .as_array()
                .is_some_and(|ops| ops.iter().any(|op| op == action))
        {
            self.db.execute("INSERT INTO openb2c_audit_log(entity,action,record_id,category,reason,actor_user_id,source,result_json) VALUES (?,?,?,?,?,?,?,?)",rusqlite::params![table,action,id.as_i64(),operation.audit["category"].as_str().or(entity_audit["category"].as_str()).unwrap_or("data"),operation.audit["reason"].as_str().or(entity_audit["reason"].as_str()),auth.user_id,source(),redact(after).unwrap_or_else(||"{\"deleted\":true}".into())])?;
        }
        for effect in &operation.effects {
            self.db.execute("INSERT INTO openb2c_rust_outbox (entity, record_id, operation, effect_json, data_json) VALUES (?, ?, ?, ?, ?)", rusqlite::params![table, id.to_string(), action, effect.to_string(), redact(after).unwrap_or_else(|| "null".into())])?;
        }
        Ok(())
    }

    /// Read existing bcrypt bearer credentials; native credential issuance is separate.
    pub fn authenticate(&self, token: &str) -> Result<AuthContext> {
        if token.starts_with("sess_") && self.schema.tables.contains_key("identity_session") {
            let prefix: String = token.chars().take(16).collect();
            let rows = self.query("SELECT id, user_id, token_hash FROM identity_session WHERE revoked = 0 AND token_prefix = ? AND julianday(expires_at) > julianday('now')", &[SqlValue::Text(prefix)])?;
            for row in rows {
                if bcrypt::verify(token, row["token_hash"].as_str().unwrap_or_default())
                    .unwrap_or(false)
                {
                    self.db.execute(
                        "UPDATE identity_session SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?",
                        [sql_value(&row["id"])?],
                    )?;
                    let scopes = self.self_scopes();
                    return Ok(AuthContext {
                        user_id: row["user_id"].as_i64(),
                        scopes,
                    });
                }
            }
        } else if self.schema.tables.contains_key("api_key") {
            let prefix: String = token.chars().take(11).collect();
            let rows = self.query("SELECT id, user_id, scopes, key_hash FROM api_key WHERE active = 1 AND key_prefix = ? AND (expires_at IS NULL OR julianday(expires_at) > julianday('now'))", &[SqlValue::Text(prefix)])?;
            for row in rows {
                if bcrypt::verify(token, row["key_hash"].as_str().unwrap_or_default())
                    .unwrap_or(false)
                {
                    self.db.execute(
                        "UPDATE api_key SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?",
                        [sql_value(&row["id"])?],
                    )?;
                    return Ok(AuthContext {
                        user_id: row["user_id"].as_i64(),
                        scopes: row["scopes"]
                            .as_str()
                            .unwrap_or_default()
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

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
fn display_value(value: &Value) -> String {
    value.as_str().map(str::to_string).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
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
