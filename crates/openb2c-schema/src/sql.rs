use crate::*;
use anyhow::{Result, bail};
use std::collections::BTreeSet;

pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn literal(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "NULL".into(),
        Value::Bool(v) => if *v { "1" } else { "0" }.into(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => format!("'{}'", v.replace('\'', "''")),
        _ => bail!("SQL literals must be scalar"),
    })
}

pub fn expression(expr: &Expr) -> Result<String> {
    Ok(match expr {
        Expr::Field { name } => quote(name),
        Expr::Lit { value } => literal(value)?,
        Expr::Bin { op, left, right } => {
            let operator = match op.as_str() {
                "==" => "=",
                "&&" => "AND",
                "||" => "OR",
                "!=" | "<" | "<=" | ">" | ">=" | "+" | "-" | "*" | "/" => op,
                _ => bail!("Unsupported SQL operator: {op}"),
            };
            format!("({} {operator} {})", expression(left)?, expression(right)?)
        }
        Expr::Un { op, arg } => match op.as_str() {
            "isNull" => format!("({} IS NULL)", expression(arg)?),
            "notNull" => format!("({} IS NOT NULL)", expression(arg)?),
            "!" => format!("(NOT {})", expression(arg)?),
            "-" => format!("(-{})", expression(arg)?),
            _ => bail!("Unsupported SQL unary operator: {op}"),
        },
        _ => bail!("Database constraints cannot traverse relations"),
    })
}

pub fn sorted_tables(tables: &Tables) -> Vec<String> {
    fn visit(
        table: &str,
        tables: &Tables,
        visited: &mut BTreeSet<String>,
        output: &mut Vec<String>,
    ) {
        if !visited.insert(table.into()) {
            return;
        }
        for col in tables[table].values() {
            if let Some((target, _)) = col.references.as_deref().and_then(reference)
                && tables.contains_key(target)
            {
                visit(target, tables, visited, output);
            }
        }
        output.push(table.into());
    }
    let mut output = Vec::new();
    let mut visited = BTreeSet::new();
    for table in tables.keys() {
        visit(table, tables, &mut visited, &mut output);
    }
    output
}

pub fn column_definition(name: &str, col: &Column) -> String {
    let mut parts = vec![quote(name), col.kind.sql().into()];
    if col.pk {
        parts.push("PRIMARY KEY".into());
    }
    if col.auto {
        parts.push("AUTOINCREMENT".into());
    }
    if col.required && !col.pk {
        parts.push("NOT NULL".into());
    }
    if col.unique {
        parts.push("UNIQUE".into());
    }
    if let Some(value) = &col.default {
        parts.push(format!("DEFAULT {value}"));
    }
    if let Some((table, field)) = col.references.as_deref().and_then(reference) {
        parts.push(format!("REFERENCES {}({})", quote(table), quote(field)));
    }
    parts.join(" ")
}

pub fn generate(schema: &Schema) -> Result<String> {
    let mut statements = vec!["PRAGMA foreign_keys = ON;".into()];
    for table in sorted_tables(&schema.tables) {
        let columns = &schema.tables[&table];
        let mut definitions = Vec::new();
        let mut checks = Vec::new();
        for (field, col) in columns {
            definitions.push(column_definition(field, col));
            let constraints = &col.constraints;
            if !constraints.values.is_empty() {
                let values = constraints
                    .values
                    .iter()
                    .map(|v| literal(&Value::String(v.clone())))
                    .collect::<Result<Vec<_>>>()?;
                checks.push(format!(
                    "CONSTRAINT {} CHECK ({} IN ({}))",
                    quote(&format!("{table}_{field}_values")),
                    quote(field),
                    values.join(", ")
                ));
            }
            for (kind, bounds) in [
                ("length", &constraints.length),
                ("range", &constraints.range),
            ] {
                if let Some(bounds) = bounds {
                    let value = if kind == "length" {
                        format!("length({})", quote(field))
                    } else {
                        quote(field)
                    };
                    let mut clauses = Vec::new();
                    if let Some(min) = bounds.min {
                        clauses.push(format!("{value} >= {min}"));
                    }
                    if let Some(max) = bounds.max {
                        clauses.push(format!("{value} <= {max}"));
                    }
                    if !clauses.is_empty() {
                        checks.push(format!(
                            "CONSTRAINT {} CHECK ({})",
                            quote(&format!("{table}_{field}_{kind}")),
                            clauses.join(" AND ")
                        ));
                    }
                }
            }
        }
        definitions.extend(checks);
        if let Some(group) = schema.constraints.get(&table) {
            for (name, constraint) in &group.check {
                if constraint.enforcement == Enforcement::Application {
                    continue;
                }
                let Some(expr) = &constraint.expression else {
                    bail!("{table}.{name}: check requires expression");
                };
                let mut check = expression(expr)?;
                if let Some(condition) = &constraint.condition {
                    check = format!("((NOT {}) OR {check})", expression(condition)?);
                }
                definitions.push(format!(
                    "CONSTRAINT {} CHECK ({check})",
                    quote(&format!("{table}_{name}"))
                ));
            }
        }
        statements.push(format!(
            "CREATE TABLE IF NOT EXISTS {} (\n    {}\n);",
            quote(&table),
            definitions.join(",\n    ")
        ));
    }
    for (table, columns) in &schema.tables {
        let mut signatures = BTreeSet::new();
        let mut names = BTreeSet::new();
        if let Some(group) = schema.constraints.get(table) {
            for (name, constraint) in &group.unique {
                if constraint.enforcement == Enforcement::Application {
                    continue;
                }
                let fields: Vec<_> = constraint.fields.iter().map(|r| r.field.clone()).collect();
                if constraint.fields.iter().any(|r| &r.table != table) {
                    bail!("{table}.{name}: database unique constraint must be local");
                }
                let condition = constraint.condition.as_ref().map(expression).transpose()?;
                statements.push(index_sql(table, name, &fields, true, condition.as_deref()));
                signatures.insert(fields);
                names.insert(name.clone());
            }
        }
        let temporal = ["starts_at", "start_at", "scheduled_at", "date"]
            .into_iter()
            .find(|n| columns.contains_key(*n));
        let facet = ["kind", "category"]
            .into_iter()
            .find(|n| columns.contains_key(*n));
        let status = columns.contains_key("status");
        let foreign_keys: Vec<_> = columns
            .iter()
            .filter(|(_, c)| c.references.is_some() && !c.pk && !c.unique)
            .collect();
        let mut candidates = Vec::new();
        if status && foreign_keys.is_empty() {
            candidates.push(("by_status".into(), vec!["status".into()]));
        }
        for (field, _) in foreign_keys {
            let base = field.strip_suffix("_id").unwrap_or(field);
            let (name, fields) = if let Some(time) = temporal {
                (
                    format!(
                        "by_{base}_{}",
                        if time == "date" { "date" } else { "start" }
                    ),
                    vec![field.clone(), time.into()],
                )
            } else if let Some(facet) = facet {
                let mut fields = vec![field.clone(), facet.into()];
                if status {
                    fields.push("status".into());
                }
                (
                    format!("by_{base}_{facet}{}", if status { "_status" } else { "" }),
                    fields,
                )
            } else if status {
                (
                    format!("by_{base}_status"),
                    vec![field.clone(), "status".into()],
                )
            } else {
                (format!("by_{base}"), vec![field.clone()])
            };
            candidates.push((name, fields));
        }
        for (name, fields) in candidates {
            if !names.contains(&name) && signatures.insert(fields.clone()) {
                statements.push(index_sql(table, &name, &fields, false, None));
                names.insert(name);
            }
        }
    }
    Ok(format!("{}\n", statements.join("\n\n")))
}

fn index_sql(
    table: &str,
    name: &str,
    fields: &[String],
    unique: bool,
    condition: Option<&str>,
) -> String {
    format!(
        "CREATE {}INDEX IF NOT EXISTS {} ON {} ({}){};",
        if unique { "UNIQUE " } else { "" },
        quote(&format!("{table}_{name}")),
        quote(table),
        fields
            .iter()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(", "),
        condition.map(|c| format!(" WHERE {c}")).unwrap_or_default()
    )
}

pub fn seed_sql(schema: &Schema, fixtures: bool) -> Result<String> {
    let rows = if fixtures {
        &schema.seed.fixtures
    } else {
        &schema.seed.reference
    };
    let mut statements = Vec::new();
    for table in sorted_tables(&schema.tables) {
        let Some(rows) = rows.get(&table) else {
            continue;
        };
        for row in rows {
            let fields: Vec<_> = row.keys().cloned().collect();
            if fields.is_empty() {
                bail!("{table}: seed row cannot be empty");
            }
            let values = row.values().map(literal).collect::<Result<Vec<_>>>()?;
            let mut conflict: Vec<_> = schema.tables[&table]
                .iter()
                .filter(|(f, c)| c.pk && row.contains_key(*f))
                .map(|(f, _)| f.clone())
                .collect();
            if conflict.is_empty() {
                if let Some((field, _)) = schema.tables[&table]
                    .iter()
                    .find(|(f, c)| c.unique && row.contains_key(*f))
                {
                    conflict.push(field.clone());
                } else if let Some(group) = schema.constraints.get(&table) {
                    for constraint in group.unique.values() {
                        if constraint.enforcement != Enforcement::Application
                            && constraint.condition.is_none()
                            && constraint
                                .fields
                                .iter()
                                .all(|r| r.table == table && row.contains_key(&r.field))
                        {
                            conflict = constraint.fields.iter().map(|r| r.field.clone()).collect();
                            break;
                        }
                    }
                }
            }
            let columns = fields
                .iter()
                .map(|f| quote(f))
                .collect::<Vec<_>>()
                .join(", ");
            let values = values.join(", ");
            if fixtures || conflict.is_empty() {
                statements.push(format!(
                    "INSERT OR IGNORE INTO {} ({columns}) VALUES ({values});",
                    quote(&table)
                ));
            } else {
                let updates: Vec<_> = fields
                    .iter()
                    .filter(|f| !conflict.contains(f))
                    .map(|f| format!("{} = excluded.{}", quote(f), quote(f)))
                    .collect();
                let action = if updates.is_empty() {
                    "DO NOTHING".into()
                } else {
                    format!("DO UPDATE SET {}", updates.join(", "))
                };
                statements.push(format!(
                    "INSERT INTO {} ({columns}) VALUES ({values}) ON CONFLICT ({}) {action};",
                    quote(&table),
                    conflict
                        .iter()
                        .map(|f| quote(f))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    if fixtures && !schema.seed.catalog.is_empty() {
        let catalog = schema
            .capabilities
            .get("catalog")
            .ok_or_else(|| anyhow::anyhow!("Demo products require the catalog capability"))?;
        let products = quote(
            catalog
                .entities
                .get("product")
                .ok_or_else(|| anyhow::anyhow!("Catalog has no product role"))?,
        );
        let prices = quote(
            catalog
                .entities
                .get("price")
                .ok_or_else(|| anyhow::anyhow!("Catalog has no price role"))?,
        );
        for (code, product) in &schema.seed.catalog {
            let code = literal(&Value::String(code.clone()))?;
            let name = literal(&Value::String(product.name.clone()))?;
            let description = literal(&serde_json::to_value(&product.description)?)?;
            let currency = literal(&Value::String(product.currency.clone()))?;
            statements.push(format!(
                "INSERT INTO {products} (code, name, description) SELECT {code}, {name}, {description} WHERE NOT EXISTS (SELECT 1 FROM {products} WHERE code={code});"
            ));
            for price in &product.prices {
                let amount = price.amount;
                let interval = literal(&Value::String(price.interval.clone()))?;
                statements.push(format!(
                    "INSERT INTO {prices} (product_id, amount_minor, currency, billing_interval) SELECT p.id, {amount}, {currency}, {interval} FROM {products} AS p WHERE p.code={code} AND NOT EXISTS (SELECT 1 FROM {prices} WHERE product_id=p.id AND amount_minor={amount} AND currency={currency} AND billing_interval={interval});"
                ));
            }
        }
    }
    Ok(if statements.is_empty() {
        String::new()
    } else {
        format!("{}\n", statements.join("\n"))
    })
}
