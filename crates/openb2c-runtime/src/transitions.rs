//! Reviewed capability changes over populated databases. Historical tables are retained.
use anyhow::{Context, Result, ensure};
use openb2c_schema::{Schema, sql::quote};
use rusqlite::{Connection, OptionalExtension, types::ValueRef};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn exists(db: &Connection, table: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        [table],
        |r| r.get(0),
    )?)
}
pub fn stored_model(db: &Connection) -> Result<Option<Schema>> {
    if !exists(db, "openb2c_schema_model")? {
        return Ok(None);
    }
    let source: Option<String> = db
        .query_row(
            "SELECT model_json FROM openb2c_schema_model WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    source
        .map(|s| serde_json::from_str(&s).context("Invalid stored application model"))
        .transpose()
}
pub fn model_hash(schema: &Schema) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(schema).expect("schema is serializable"),
    ))
}
fn rows(db: &Connection, table: &str) -> Result<i64> {
    if !exists(db, table)? {
        return Ok(0);
    }
    Ok(
        db.query_row(&format!("SELECT count(*) FROM {}", quote(table)), [], |r| {
            r.get(0)
        })?,
    )
}
fn state_hash(db: &Connection, old: &Schema) -> Result<String> {
    let mut hash = Sha256::new();
    for table in old.tables.keys() {
        hash.update(table.as_bytes());
        if !exists(db, table)? {
            continue;
        }
        let columns = old.tables[table]
            .keys()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = db.prepare(&format!(
            "SELECT {columns} FROM {} ORDER BY {}",
            quote(table),
            quote(
                old.tables[table]
                    .iter()
                    .find(|(_, c)| c.pk)
                    .map(|(f, _)| f.as_str())
                    .unwrap_or("id")
            )
        ))?;
        let width = statement.column_count();
        let mut result = statement.query([])?;
        while let Some(row) = result.next()? {
            for index in 0..width {
                match row.get_ref(index)? {
                    ValueRef::Null => hash.update([0]),
                    ValueRef::Integer(v) => {
                        hash.update([1]);
                        hash.update(v.to_be_bytes());
                    }
                    ValueRef::Real(v) => {
                        hash.update([2]);
                        hash.update(v.to_bits().to_be_bytes());
                    }
                    ValueRef::Text(v) => {
                        hash.update([3]);
                        hash.update((v.len() as u64).to_be_bytes());
                        hash.update(v);
                    }
                    ValueRef::Blob(v) => {
                        hash.update([4]);
                        hash.update((v.len() as u64).to_be_bytes());
                        hash.update(v);
                    }
                }
            }
        }
    }
    Ok(hex::encode(hash.finalize()))
}
pub fn plan(db: &Connection, new: &Schema) -> Result<Value> {
    let errors = openb2c_schema::validate::validate(new);
    ensure!(
        errors.is_empty(),
        "Invalid target model: {}",
        errors.join("; ")
    );
    let Some(old) = stored_model(db)? else {
        return Ok(
            json!({"requires_review":false,"changes":[],"blockers":[],"fresh_database":true}),
        );
    };
    let mut changes = vec![];
    let mut blockers = vec![];
    let mut archives = vec![];
    let names = old
        .capabilities
        .keys()
        .chain(new.capabilities.keys())
        .collect::<BTreeSet<_>>();
    for name in names {
        let before = old.capabilities.get(name);
        let after = new.capabilities.get(name);
        if serde_json::to_value(before)? == serde_json::to_value(after)? {
            continue;
        }
        let kind = if before.is_none() {
            "enable"
        } else if after.is_none() {
            "disable"
        } else {
            "reconfigure"
        };
        changes.push(json!({"capability":name,"action":kind,"before":before,"after":after}));
        if let Some(cap) = before
            && after.is_none()
        {
            for table in cap.entities.values() {
                archives.push(json!({"table":table,"rows":rows(db,table)?}));
            }
            let role = |r: &str| cap.entities.get(r).map(String::as_str);
            let count_where = |role_name: &str, predicate: &str| -> Result<i64> {
                let Some(t) = role(role_name) else {
                    return Ok(0);
                };
                if !exists(db, t)? {
                    return Ok(0);
                }
                Ok(db.query_row(
                    &format!("SELECT count(*) FROM {} WHERE {predicate}", quote(t)),
                    [],
                    |r| r.get(0),
                )?)
            };
            let unresolved = match name.as_str() {
                "orders" => count_where("order", "status='pending'")?,
                "payments" => {
                    let mut count = count_where("payment", "status='pending'")?;
                    if let (Some(t), Some(a)) = (role("transaction"), role("allocation"))
                        && exists(db, t)?
                        && exists(db, a)?
                    {
                        let partial_review = if old.capabilities.contains_key("billing_payments") {
                            ""
                        } else {
                            "a.amount_minor<>p.amount_minor OR "
                        };
                        count+=db.query_row(&format!("SELECT count(*) FROM {} t WHERE t.disposition='open' AND (t.review_reason IS NOT NULL OR (t.status='settled' AND t.amount_minor>(SELECT coalesce(sum(a.amount_minor),0) FROM {} a WHERE a.transaction_id=t.id)) OR EXISTS (SELECT 1 FROM {} a JOIN {} p ON p.id=a.payment_id WHERE a.transaction_id=t.id AND a.kind='receipt' AND ({partial_review}p.status IN ('pending','cancelled','failed'))))",quote(t),quote(a),quote(a),quote(role("payment").ok_or_else(||anyhow::anyhow!("Missing payment role"))?)),[],|r|r.get::<_,i64>(0))?;
                    }
                    count
                }
                "billing_payments" => {
                    if let (Some(p), Some(a)) = (
                        openb2c_schema::capabilities::entity(&old, "payments", "payment"),
                        openb2c_schema::capabilities::entity(&old, "payments", "allocation"),
                    ) {
                        db.query_row(&format!("SELECT count(*) FROM {} p WHERE p.status IN ('pending','cancelled','failed') AND EXISTS(SELECT 1 FROM {} a WHERE a.payment_id=p.id)",quote(p),quote(a)),[],|r|r.get(0))?
                    } else {
                        0
                    }
                }
                "subscriptions" => {
                    count_where("subscription", "status IN ('pending','active','past_due')")?
                }
                "entitlements" => count_where(
                    "entitlement",
                    "status='active' AND (ends_at IS NULL OR julianday(ends_at)>julianday('now'))",
                )?,
                "purchasing" => count_where(
                    "expense",
                    "payment_journal_id IS NULL AND void_journal_id IS NULL",
                )?,
                "accounting" => count_where("journal", "1=1")?,
                "billing" => {
                    let mut count = count_where("event", "status='pending'")?;
                    if let (Some(i), Some(r), Some(c)) =
                        (role("invoice"), role("receipt"), role("credit"))
                    {
                        count+=db.query_row(&format!("SELECT count(*) FROM {} i WHERE NOT EXISTS(SELECT 1 FROM {} c WHERE c.invoice_id=i.id) AND (NOT EXISTS(SELECT 1 FROM {} r WHERE r.invoice_id=i.id) OR (i.period_end IS NOT NULL AND i.recognized_minor<i.net_minor))",quote(i),quote(c),quote(r)),[],|r|r.get::<_,i64>(0))?;
                        if let Some(f) = role("refund") {
                            count+=db.query_row(&format!("SELECT count(*) FROM {} i JOIN {} c ON c.invoice_id=i.id JOIN {} r ON r.invoice_id=i.id WHERE NOT EXISTS(SELECT 1 FROM {} f WHERE f.invoice_id=i.id)",quote(i),quote(c),quote(r),quote(f)),[],|r|r.get::<_,i64>(0))?;
                        }
                    }
                    count
                }
                "billing_subscriptions" | "subscription_entitlements" => {
                    if let Some(t) =
                        openb2c_schema::capabilities::entity(&old, "subscriptions", "subscription")
                    {
                        db.query_row(&format!("SELECT count(*) FROM {} WHERE status IN ('pending','active','past_due')",quote(t)),[],|r|r.get(0))?
                    } else {
                        0
                    }
                }
                "order_payments" => {
                    if let Some(t) = openb2c_schema::capabilities::entity(&old, "orders", "order") {
                        db.query_row(
                            &format!("SELECT count(*) FROM {} WHERE status='pending'", quote(t)),
                            [],
                            |r| r.get(0),
                        )?
                    } else {
                        0
                    }
                }
                _ => 0,
            };
            let unresolved = if [
                "billing_orders",
                "billing_subscriptions",
                "billing_payments",
            ]
            .contains(&name.as_str())
            {
                if let Some(table) = openb2c_schema::capabilities::entity(&old, "billing", "event")
                {
                    if exists(db, table)? {
                        db.query_row(
                            &format!(
                                "SELECT count(*) FROM {} WHERE status='pending'",
                                quote(table)
                            ),
                            [],
                            |r| r.get::<_, i64>(0),
                        )?
                    } else {
                        0
                    }
                } else {
                    0
                }
            } else {
                unresolved
            };
            if unresolved > 0 {
                blockers.push(json!({"capability":name,"unresolved":unresolved,"reason":"Resolve obligations before retiring this capability"}));
            }
        }
    }
    if !old.capabilities.is_empty()
        && serde_json::to_value(&old.operations)? != serde_json::to_value(&new.operations)?
    {
        changes.push(json!({"action":"business_operations_changed"}));
    }
    if serde_json::to_value(&old.workflows)? != serde_json::to_value(&new.workflows)? {
        changes.push(json!({"action":"business_workflows_changed"}));
    }
    if !old.capabilities.is_empty() && old.metadata.get("auth") != new.metadata.get("auth") {
        changes.push(json!({"action":"authorization_policy_changed"}));
    }
    let populated = old
        .tables
        .keys()
        .try_fold(false, |any, t| -> Result<bool> {
            Ok(any || rows(db, t)? > 0)
        })?;
    let state = state_hash(db, &old)?;
    let old_hash = model_hash(&old);
    let new_hash = model_hash(new);
    let hash = hex::encode(Sha256::digest(format!("{old_hash}:{new_hash}:{state}")));
    Ok(
        json!({"plan_hash":hash,"from_model":old_hash,"to_model":new_hash,"state_hash":state,"requires_review":populated&&!changes.is_empty(),"changes":changes,"blockers":blockers,"retained_archives":archives,"historical_billing":"Enabling sale integrations backfills source payment history with unassessed tax; closed periods queue postings for explicit review. Historical tax classification is never invented."}),
    )
}
pub(crate) fn check(db: &Connection, new: &Schema, dir: Option<&std::path::Path>) -> Result<()> {
    let p = plan_at(db, new, dir)?;
    ensure!(
        p["blockers"].as_array().is_some_and(|b| b.is_empty()),
        "Upgrade blocked: {}",
        p["blockers"]
    );
    if p["requires_review"] == true {
        let approved = exists(db, "openb2c_capability_approval")?
            && db.query_row(
                "SELECT EXISTS(SELECT 1 FROM openb2c_capability_approval WHERE plan_hash=?)",
                [p["plan_hash"].as_str().unwrap_or("")],
                |r| r.get::<_, bool>(0),
            )?;
        ensure!(
            approved,
            "Capability/behaviour transition requires review; run --plan-upgrade then --approve-upgrade PLAN_HASH REASON. Plan: {}",
            p
        );
    }
    Ok(())
}
pub(crate) fn verify_roles(
    db: &Connection,
    new: &Schema,
    snapshots: &[RoleSnapshot],
) -> Result<()> {
    let Some(old) = stored_model(db)? else {
        return Ok(());
    };
    // v1 customer IDs were account IDs. Preserve the complete legacy membership explicitly.
    if openb2c_schema::capabilities::entity(&old, "customers", "customer") == Some("user")
        && let Some(target) = openb2c_schema::capabilities::entity(new, "customers", "customer")
        && target != "user"
    {
        db.execute(&format!("INSERT INTO {} (id,user_id) SELECT id,id FROM user WHERE id NOT IN (SELECT user_id FROM {})",quote(target),quote(target)),[])?;
    }
    verify_snapshots(db, new, snapshots)
}
impl crate::Service {
    pub(crate) fn complete_transition(&self, old: Option<&Schema>) -> Result<()> {
        if let Some(old) = old {
            #[cfg(any(feature = "reference", feature = "cap_billing"))]
            {
                self.preserve_legacy_refunds(old)?;
                self.backfill_billing(old)?;
            }
        }
        Ok(())
    }
    #[cfg(feature = "reference")]
    pub fn assert_current_model(&self) -> crate::service::Result<()> {
        let old = stored_model(&self.db).map_err(|_| {
            crate::service::ServiceError::new("internal_error", "Application model unavailable")
        })?;
        if old
            .as_ref()
            .is_some_and(|old| model_hash(old) != model_hash(&self.schema))
        {
            return Err(crate::service::ServiceError::new(
                "bad_state",
                "Application model changed; restart this process before serving requests",
            ));
        }
        Ok(())
    }
}

/// Bind review to the actual operator-owned SQL, not just the target model.
pub fn plan_at(db: &Connection, new: &Schema, dir: Option<&std::path::Path>) -> Result<Value> {
    let mut p = plan(db, new)?;
    let mut files = std::collections::BTreeMap::new();
    if let Some(dir) = dir.filter(|d| d.exists()) {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "sql") {
                let name = path
                    .file_name()
                    .context("Missing migration name")?
                    .to_string_lossy()
                    .into_owned();
                files.insert(name, hex::encode(Sha256::digest(std::fs::read(&path)?)));
            }
        }
    }
    if let Some(base) = p["plan_hash"].as_str() {
        p["plan_hash"] = json!(hex::encode(Sha256::digest(format!(
            "{base}:{}",
            serde_json::to_string(&files)?
        ))));
    }
    if !files.is_empty() && stored_model(db)?.is_some_and(|m| !m.capabilities.is_empty()) {
        for name in files.keys() {
            let applied: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM openb2c_migration WHERE id=?)",
                [name.trim_end_matches(".sql")],
                |r| r.get(0),
            )?;
            if !applied {
                p["requires_review"] = json!(true);
            }
        }
    }
    p["migration_files"] = json!(files);
    Ok(p)
}
pub fn approve_at(
    db: &Connection,
    new: &Schema,
    dir: Option<&std::path::Path>,
    expected: &str,
    reason: &str,
) -> Result<Value> {
    ensure!(
        !reason.trim().is_empty(),
        "An upgrade review reason is required"
    );
    let tx = crate::service::transaction(db)?;
    let p = plan_at(db, new, dir)?;
    ensure!(
        p["plan_hash"] == expected,
        "Upgrade plan changed; inspect a fresh plan"
    );
    ensure!(
        p["blockers"].as_array().is_some_and(|b| b.is_empty()),
        "Upgrade has unresolved obligations: {}",
        p["blockers"]
    );
    db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_capability_approval(plan_hash TEXT PRIMARY KEY,reason TEXT NOT NULL,approved_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")?;
    db.execute("INSERT INTO openb2c_capability_approval(plan_hash,reason) VALUES (?,?) ON CONFLICT(plan_hash) DO UPDATE SET reason=excluded.reason",[expected,reason])?;
    tx.finish()?;
    Ok(p)
}

pub(crate) struct RoleSnapshot {
    capability: String,
    role: String,
    table: String,
    fields: Vec<String>,
    rows: Vec<Vec<rusqlite::types::Value>>,
}
pub(crate) fn snapshot_roles(db: &Connection) -> Result<Vec<RoleSnapshot>> {
    let Some(old) = stored_model(db)? else {
        return Ok(vec![]);
    };
    let mut snapshots = vec![];
    for (name, cap) in &old.capabilities {
        for (role, table) in &cap.entities {
            ensure!(
                exists(db, table)?,
                "Stored capability {name}.{role} has a missing table {table}; restore or repair before upgrading"
            );
            let fields = old.tables[table].keys().cloned().collect::<Vec<_>>();
            let columns = fields
                .iter()
                .map(|f| quote(f))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = db.prepare(&format!("SELECT {columns} FROM {}", quote(table)))?;
            let rows = statement
                .query_map([], |r| {
                    fields
                        .iter()
                        .enumerate()
                        .map(|(i, _)| r.get(i))
                        .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            snapshots.push(RoleSnapshot {
                capability: name.clone(),
                role: role.clone(),
                table: table.clone(),
                fields,
                rows,
            });
        }
    }
    Ok(snapshots)
}
fn verify_snapshots(db: &Connection, new: &Schema, snapshots: &[RoleSnapshot]) -> Result<()> {
    for saved in snapshots {
        ensure!(
            exists(db, &saved.table)?,
            "Retain historical table {} when changing capability ownership",
            saved.table
        );
        let target =
            if saved.capability == "customers" && saved.role == "customer" && saved.table == "user"
            {
                "user"
            } else {
                new.capabilities
                    .get(&saved.capability)
                    .and_then(|c| c.entities.get(&saved.role))
                    .map(String::as_str)
                    .unwrap_or(&saved.table)
            };
        ensure!(exists(db, target)?, "Migration target {target} is missing");
        let actual_fields = {
            let mut statement = db.prepare(&format!("PRAGMA table_info({})", quote(target)))?;
            statement
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<BTreeSet<_>>>()?
        };
        // Every original value must remain in the live role or its retained archive.
        let fields = saved
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| actual_fields.contains(*f))
            .collect::<Vec<_>>();
        ensure!(
            fields.iter().any(|(_, f)| f.as_str() == "id"),
            "Role migration {} -> {target} needs an ID-preserving mapping",
            saved.table
        );
        for row in &saved.rows {
            let archival = saved
                .fields
                .iter()
                .map(|f| format!("{} IS ?", quote(f)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let retained: bool = db
                .query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM {} WHERE {archival})",
                        quote(&saved.table)
                    ),
                    rusqlite::params_from_iter(row.iter()),
                    |r| r.get(0),
                )
                .context("Retain original role columns and records in their archive")?;
            ensure!(
                retained,
                "Migration changes historical {} data; preserve an archive and use explicit workflow operations for corrections",
                saved.table
            );
            let conditions = fields
                .iter()
                .map(|(_, f)| format!("{} IS ?", quote(f)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let values = fields.iter().map(|(i, _)| &row[*i]);
            let found: bool = db.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE {conditions})",
                    quote(target)
                ),
                rusqlite::params_from_iter(values),
                |r| r.get(0),
            )?;
            ensure!(
                found,
                "Role migration {} -> {target} loses or changes an original record; provide a data-preserving numbered migration",
                saved.table
            );
        }
    }
    Ok(())
}
