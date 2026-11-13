use anyhow::{Context, Result, ensure};
use openb2c_schema::{Schema, sql};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

/// Matches the original JavaScript FNV-1a implementation, including UTF-16 input.
pub fn checksum(source: &str) -> String {
    let mut hash = 2166136261u32;
    for unit in source.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(16777619);
    }
    format!("{hash:08x}")
}
fn history(db: &Connection, id: &str) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT checksum FROM openb2c_migration WHERE id=?",
            [id],
            |r| r.get(0),
        )
        .optional()?)
}
fn record(db: &Connection, id: &str, hash: &str, description: &str) -> Result<()> {
    db.execute(
        "INSERT INTO openb2c_migration(id,checksum,description) VALUES (?,?,?)",
        params![id, hash, description],
    )?;
    Ok(())
}
fn apply(db: &Connection, id: &str, source: &str, description: &str) -> Result<Value> {
    let hash = checksum(source);
    if let Some(old) = history(db, id)? {
        ensure!(
            old == hash,
            "Migration {id} checksum changed after it was applied"
        );
        return Ok(json!({"id":id,"checksum":hash,"description":description,"status":"verified"}));
    }
    let tx = db.unchecked_transaction()?;
    db.execute_batch(source)
        .with_context(|| format!("Applying migration {id}"))?;
    record(db, id, &hash, description)?;
    tx.commit()?;
    Ok(json!({"id":id,"checksum":hash,"description":description,"status":"applied"}))
}
pub fn run(db: &Connection, schema: &Schema, dir: Option<&Path>) -> Result<Vec<Value>> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_migration(id TEXT PRIMARY KEY,checksum TEXT NOT NULL,description TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
        CREATE TABLE IF NOT EXISTS openb2c_schema_model(id INTEGER PRIMARY KEY CHECK(id=1), model_json TEXT NOT NULL);")?;
    let user_schema:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('openb2c_migration','openb2c_schema_model'))",[],|r|r.get(0))?;
    let mut files = Vec::new();
    if let Some(dir) = dir.filter(|p| p.exists()) {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "sql") {
                let id = path
                    .file_stem()
                    .context("Migration file requires a name")?
                    .to_string_lossy()
                    .into_owned();
                files.push((id, fs::read_to_string(&path)?));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut diagnostics = Vec::new();
    let mut newly_applied = false;
    if user_schema {
        for (id, source) in &files {
            newly_applied |= history(db, id)?.is_none();
            diagnostics.push(apply(db, id, source, &format!("migration file: {id}.sql"))?);
        }
    }
    let snapshot: Option<String> = db
        .query_row(
            "SELECT model_json FROM openb2c_schema_model WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(snapshot) = snapshot {
        let old: Value = serde_json::from_str(&snapshot)?;
        let new = serde_json::to_value(schema)?;
        if files
            .iter()
            .all(|(id, _)| history(db, id).ok().flatten().is_some())
        {
            // Removed columns and changed constraints require explicit forward SQL;
            // CREATE TABLE IF NOT EXISTS must never pretend to apply them.
            for (table, cols) in old["tables"].as_object().into_iter().flatten() {
                for (field, col) in cols.as_object().into_iter().flatten() {
                    if new["tables"][table][field].is_null() {
                        continue;
                    }
                    for property in [
                        "type",
                        "pk",
                        "auto",
                        "required",
                        "unique",
                        "default",
                        "references",
                        "constraints",
                    ] {
                        if col[property] != new["tables"][table][field][property] && !newly_applied
                        {
                            anyhow::bail!(
                                "{table}.{field}: changed {property} requires a numbered migration"
                            );
                        }
                    }
                }
            }
        }
    }
    let tx = db.unchecked_transaction()?;
    for (table, cols) in &schema.tables {
        let mut stmt = db.prepare(&format!("PRAGMA table_info({})", sql::quote(table)))?;
        let existing = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    (
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, i64>(5)?,
                    ),
                ))
            })?
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
        for (field, col) in cols {
            if let Some((kind, required, default, pk)) = existing.get(field) {
                ensure!(
                    kind.eq_ignore_ascii_case(col.kind.sql())
                        && (*pk > 0) == col.pk
                        && (*required > 0) == (col.required && !col.pk)
                        && default == &col.default,
                    "{table}.{field}: database shape differs; apply a data-preserving migration"
                );
            } else if !existing.is_empty() {
                ensure!(
                    !col.pk && !col.unique && (!col.required || col.default.is_some()),
                    "{table}.{field}: manual data-preserving migration required"
                );
                db.execute_batch(&format!(
                    "ALTER TABLE {} ADD COLUMN {};",
                    sql::quote(table),
                    sql::column_definition(field, col)
                ))?;
            }
        }
    }
    let source = sql::generate(schema)?;
    let hash = checksum(&source);
    let id = format!("schema:{hash}");
    if history(db, &id)?.is_none() {
        db.execute_batch(&source)?;
        record(
            db,
            &id,
            &hash,
            if user_schema {
                "generated schema sync"
            } else {
                "generated schema baseline"
            },
        )?;
    }
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(&source)?;
    for table in schema.tables.keys() {
        let actual: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
            [table],
            |r| r.get(0),
        )?;
        let wanted: String = expected.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
            [table],
            |r| r.get(0),
        )?;
        ensure!(
            checks(&actual) == checks(&wanted),
            "{table}: database CHECK constraints differ; apply a numbered table-rebuild migration"
        );
        type ForeignKeyShape = (String, String, String, String, String);
        let foreign_keys = |db: &Connection| -> Result<Vec<ForeignKeyShape>> {
            let mut stmt=db.prepare("SELECT \"from\",\"table\",\"to\",on_update,on_delete FROM pragma_foreign_key_list(?) ORDER BY \"from\",\"to\"")?;
            Ok(stmt
                .query_map([table], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        };
        ensure!(
            foreign_keys(db)? == foreign_keys(&expected)?,
            "{table}: foreign keys differ; apply a numbered migration"
        );
        let unique = |db: &Connection| -> Result<Vec<Vec<String>>> {
            let mut stmt = db.prepare(
                "SELECT name FROM pragma_index_list(?) WHERE \"unique\"=1 AND partial=0",
            )?;
            let names = stmt
                .query_map([table], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut result = Vec::new();
            for name in names {
                let mut stmt =
                    db.prepare("SELECT name FROM pragma_index_info(?) ORDER BY seqno")?;
                result.push(
                    stmt.query_map([name], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                );
            }
            Ok(result)
        };
        let existing = unique(db)?;
        ensure!(
            unique(&expected)?
                .iter()
                .all(|index| existing.contains(index)),
            "{table}: unique constraints differ; apply a numbered migration"
        );
    }
    let mut statement = expected
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL")?;
    let indexes = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (name, wanted) in indexes {
        let actual: Option<String> = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name=?",
                [&name],
                |r| r.get(0),
            )
            .optional()?;
        ensure!(
            actual.is_some_and(|sql| normalize(&sql) == normalize(&wanted)),
            "{name}: database index differs; apply a numbered migration"
        );
    }
    if !user_schema {
        for (id, source) in &files {
            let hash = checksum(source);
            if history(db, id)?.is_none() {
                record(db, id, &hash, "included in fresh schema baseline")?;
            }
            diagnostics.push(json!({"id":id,"checksum":hash,"status":"baseline_included"}));
        }
    }
    db.execute("INSERT INTO openb2c_schema_model(id,model_json) VALUES (1,?) ON CONFLICT(id) DO UPDATE SET model_json=excluded.model_json",[serde_json::to_string(schema)?])?;
    tx.commit()?;
    diagnostics.push(json!({"id":id,"checksum":hash,"status":"verified"}));
    Ok(diagnostics)
}

fn normalize(sql: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            out.push(c);
            if quoted && chars.peek() == Some(&'\'') {
                out.push(chars.next().expect("peeked quote"));
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            out.push(c);
        } else if !c.is_whitespace() && !['"', '[', ']'].contains(&c) {
            out.push(c.to_ascii_lowercase());
        }
    }
    out.replacen("createindexifnotexists", "createindex", 1)
        .replacen("createuniqueindexifnotexists", "createuniqueindex", 1)
}
fn checks(sql: &str) -> Vec<String> {
    let sql = normalize(sql);
    let bytes = sql.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        if bytes[index] == b'\'' {
            if quoted && bytes.get(index + 1) == Some(&b'\'') {
                index += 2;
                continue;
            }
            quoted = !quoted;
            index += 1;
            continue;
        }
        if !quoted && bytes.get(index..index + 6) == Some(b"check(") {
            let start = index + 6;
            let mut cursor = start;
            let mut depth = 1;
            let mut literal = false;
            while cursor < bytes.len() {
                let c = bytes[cursor];
                if c == b'\'' {
                    if literal && bytes.get(cursor + 1) == Some(&b'\'') {
                        cursor += 2;
                        continue;
                    }
                    literal = !literal;
                } else if !literal {
                    if c == b'(' {
                        depth += 1;
                    }
                    if c == b')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                cursor += 1;
            }
            result.push(outer(&sql[start..cursor]).into());
            index = cursor;
        }
        index += 1;
    }
    result.sort();
    result
}

fn outer(mut text: &str) -> &str {
    while text.starts_with('(') && text.ends_with(')') {
        let mut depth = 0;
        let mut quoted = false;
        let mut closes_early = false;
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'\'' {
                if quoted && bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                quoted = !quoted;
            } else if !quoted {
                if c == b'(' {
                    depth += 1;
                }
                if c == b')' {
                    depth -= 1;
                    if depth == 0 && i + 1 < bytes.len() {
                        closes_early = true;
                        break;
                    }
                }
            }
            i += 1;
        }
        if closes_early {
            break;
        }
        text = &text[1..text.len() - 1];
    }
    text
}
