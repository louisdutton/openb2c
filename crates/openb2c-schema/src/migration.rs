use crate::{Schema, sql};
use anyhow::Result;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub kind: String,
    pub description: String,
    pub sql: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
    pub rollback_guidance: String,
    pub forward_fix_guidance: String,
}
pub fn plan(old: &Schema, new: &Schema) -> Result<Plan> {
    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    for (table, cols) in &new.tables {
        if !old.tables.contains_key(table) {
            let model = Schema {
                tables: std::collections::BTreeMap::from([(table.clone(), cols.clone())]),
                ..Default::default()
            };
            steps.push(Step {
                kind: "create_table".into(),
                description: format!("Create table {table}"),
                sql: Some(sql::generate(&model)?),
            });
            continue;
        }
        if serde_json::to_value(old.constraints.get(table))?
            != serde_json::to_value(new.constraints.get(table))?
        {
            let message = format!(
                "Changed constraints on {table} require a manual data-preserving migration"
            );
            warnings.push(message.clone());
            steps.push(Step {
                kind: "manual".into(),
                description: message,
                sql: None,
            });
        }
        for (field, col) in cols {
            let previous = old.tables[table].get(field);
            if previous
                .is_some_and(|c| serde_json::to_value(c).ok() == serde_json::to_value(col).ok())
            {
                continue;
            }
            let safe = previous.is_none()
                && !col.pk
                && !col.unique
                && (!col.required || col.default.is_some());
            if safe {
                steps.push(Step {
                    kind: "add_column".into(),
                    description: format!("Add column {table}.{field}"),
                    sql: Some(format!(
                        "ALTER TABLE {} ADD COLUMN {};",
                        sql::quote(table),
                        sql::column_definition(field, col)
                    )),
                });
            } else {
                let message =
                    format!("{table}.{field} requires a manual data-preserving migration");
                warnings.push(message.clone());
                steps.push(Step {
                    kind: "manual".into(),
                    description: message,
                    sql: None,
                });
            }
        }
        for field in old.tables[table].keys().filter(|f| !cols.contains_key(*f)) {
            let message =
                format!("Removed field {table}.{field} requires a manual archival migration");
            warnings.push(message.clone());
            steps.push(Step {
                kind: "manual".into(),
                description: message,
                sql: None,
            });
        }
    }
    for table in old.tables.keys().filter(|t| !new.tables.contains_key(*t)) {
        let message = format!("Removed table {table} requires a manual archival migration");
        warnings.push(message.clone());
        steps.push(Step {
            kind: "manual".into(),
            description: message,
            sql: None,
        });
    }
    Ok(Plan {
        steps,
        warnings,
        rollback_guidance:
            "Back up SQLite before applying migrations; restore the backup to roll back.".into(),
        forward_fix_guidance:
            "Keep applied migration files unchanged; add a numbered forward-fix migration.".into(),
    })
}
pub fn stub(plan: &Plan) -> String {
    let mut text = format!(
        "-- OpenB2C migration\n-- Rollback: {}\n-- Forward fix: {}\n",
        plan.rollback_guidance, plan.forward_fix_guidance
    );
    for step in &plan.steps {
        text.push_str(&format!(
            "\n-- {}\n{}\n",
            step.description,
            step.sql
                .as_deref()
                .unwrap_or("-- TODO: Write a data-preserving migration.")
        ));
    }
    text
}
