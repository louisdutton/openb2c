//! Native expression lowering used by concrete records.
use super::{Type, field, record_name};
use anyhow::{Context, Result, bail, ensure};
use openb2c_schema::{
    Enforcement, Expr, Schema, SqlType,
    inference::{fk, foreign_keys},
    sql::quote,
};
use serde_json::Value;

fn primary<'a>(schema: &'a Schema, table: &str) -> Result<&'a str> {
    schema.tables[table]
        .iter()
        .find(|(_, c)| c.pk)
        .map(|(n, _)| n.as_str())
        .context("Typed record needs a primary key")
}
fn member(schema: &Schema, table: &str, row: &str, name: &str) -> Result<(String, Type)> {
    let c = schema.tables[table]
        .get(name)
        .context("Unknown typed field")?;
    let mut value = if row == "draft" {
        format!("({row}.{}_value()?)", field(name))
    } else {
        format!("{row}.{}", field(name))
    };
    let native = match c.kind {
        SqlType::Integer => Type::Int,
        SqlType::Real => Type::Real,
        SqlType::Text => Type::Text,
        SqlType::Blob => bail!("Blob expressions are unsupported"),
    };
    let optional = !c.required && !c.pk;
    if c.kind == SqlType::Text {
        value = if optional {
            if c.constraints.values.is_empty() {
                format!("{value}.clone()")
            } else {
                format!("{value}.as_ref().map(|v|v.as_str().to_owned())")
            }
        } else if c.constraints.values.is_empty() {
            format!("{value}.clone()")
        } else {
            format!("{value}.as_str().to_owned()")
        };
    }
    Ok((
        value,
        if optional {
            Type::Optional(Box::new(native))
        } else {
            native
        },
    ))
}
pub(super) fn related(
    schema: &Schema,
    table: &str,
    target: &str,
    via: Option<&str>,
    row: &str,
) -> Result<String> {
    let (sql, key) = if let Some(bridge) = via {
        let local = fk(schema, bridge, table)?;
        let remote = fk(schema, bridge, target)?;
        (
            format!(
                "SELECT t.* FROM {} t JOIN {} b ON t.{}=b.{} WHERE b.{}=?",
                quote(target),
                quote(bridge),
                quote(primary(schema, target)?),
                quote(&remote),
                quote(&local)
            ),
            primary(schema, table)?.to_string(),
        )
    } else {
        let direct = foreign_keys(schema, table)
            .into_iter()
            .filter(|(_, t, _)| *t == target)
            .collect::<Vec<_>>();
        let inverse = foreign_keys(schema, target)
            .into_iter()
            .filter(|(_, t, _)| *t == table)
            .collect::<Vec<_>>();
        if direct.len() == 1 {
            (
                format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(target),
                    quote(direct[0].2)
                ),
                direct[0].0.to_owned(),
            )
        } else if inverse.len() == 1 {
            (
                format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(target),
                    quote(inverse[0].0)
                ),
                inverse[0].2.to_owned(),
            )
        } else {
            bail!("Ambiguous typed relationship from {table} to {target}")
        }
    };
    let key = if row == "draft" {
        format!("{row}.{}_value()?", field(&key))
    } else {
        format!("{row}.{}", field(&key))
    };
    Ok(format!(
        "{}::query(service,{sql:?},&[&{key}])?",
        record_name(target)
    ))
}
pub(super) fn expr(schema: &Schema, table: &str, row: &str, e: &Expr) -> Result<(String, Type)> {
    Ok(match e {
        Expr::Field { name } => member(schema, table, row, name)?,
        Expr::Lit { value } => match value {
            Value::Null => ("None::<String>".into(), Type::Null),
            Value::Bool(v) => (v.to_string(), Type::Bool),
            Value::String(v) => (format!("{v:?}.to_owned()"), Type::Text),
            Value::Number(v) if v.is_i64() => (format!("{}i64", v.as_i64().unwrap()), Type::Int),
            Value::Number(v) => (
                format!("{}f64", v.as_f64().context("Invalid numeric literal")?),
                Type::Real,
            ),
            _ => bail!("Typed expressions require scalar literals"),
        },
        Expr::Bin { op, left, right } => {
            let (mut l, lt) = expr(schema, table, row, left)?;
            let (mut r, rt) = expr(schema, table, row, right)?;
            if ["==", "!="].contains(&op.as_str()) && (lt == Type::Null || rt == Type::Null) {
                let (value, ty) = if lt == Type::Null { (r, rt) } else { (l, lt) };
                let null = if matches!(ty, Type::Optional(_) | Type::Null) {
                    format!("({value}).is_none()")
                } else {
                    "false".into()
                };
                (
                    if op == "!=" {
                        format!("!({null})")
                    } else {
                        null
                    },
                    Type::Bool,
                )
            } else if ["+", "-", "*", "/"].contains(&op.as_str()) {
                let base = |v: String, t: &Type| -> Result<(String, Type)> {
                    Ok(match t {
                        Type::Optional(inner) => (
                            format!(
                                "({v}).ok_or_else(||invalid(\"Arithmetic requires a value\"))?"
                            ),
                            *inner.clone(),
                        ),
                        _ => (v, t.clone()),
                    })
                };
                let (l, lt) = base(l, &lt)?;
                let (r, rt) = base(r, &rt)?;
                ensure!(
                    matches!(lt, Type::Int | Type::Real) && matches!(rt, Type::Int | Type::Real),
                    "Arithmetic requires numbers"
                );
                if lt == Type::Int && rt == Type::Int && op != "/" {
                    let method = match op.as_str() {
                        "+" => "checked_add",
                        "-" => "checked_sub",
                        _ => "checked_mul",
                    };
                    (
                        format!(
                            "({l}).{method}({r}).ok_or_else(||invalid(\"Integer arithmetic overflow\"))?"
                        ),
                        Type::Int,
                    )
                } else {
                    (
                        format!(
                            "{{let value=({l} as f64){op}({r} as f64);if !value.is_finite(){{return Err(invalid(\"Non-finite arithmetic result\"));}}value}}"
                        ),
                        Type::Real,
                    )
                }
            } else {
                if lt != rt {
                    if lt == Type::Optional(Box::new(rt.clone())) {
                        r = format!("Some({r})");
                    } else if rt == Type::Optional(Box::new(lt.clone())) {
                        l = format!("Some({l})");
                    } else if lt == Type::Int && rt == Type::Real {
                        l = format!("({l}) as f64");
                    } else if lt == Type::Real && rt == Type::Int {
                        r = format!("({r}) as f64");
                    } else {
                        bail!("Incompatible typed comparison {lt:?} {op} {rt:?}");
                    }
                }
                ensure!(
                    ["==", "!=", "<", "<=", ">", ">=", "&&", "||"].contains(&op.as_str()),
                    "Unsupported typed operator {op}"
                );
                if ["&&", "||"].contains(&op.as_str()) {
                    ensure!(
                        lt == Type::Bool && rt == Type::Bool,
                        "Boolean values required"
                    );
                }
                if ["<", "<=", ">", ">="].contains(&op.as_str())
                    && (matches!(lt, Type::Optional(_)) || matches!(rt, Type::Optional(_)))
                {
                    (
                        format!(
                            "match ({l},{r}){{(Some(left),Some(right))=>left{op}right,_=>false}}"
                        ),
                        Type::Bool,
                    )
                } else {
                    (format!("(({l}){op}({r}))"), Type::Bool)
                }
            }
        }
        Expr::Un { op, arg } => {
            let (v, t) = expr(schema, table, row, arg)?;
            match op.as_str() {
                "!" => {
                    ensure!(t == Type::Bool, "Boolean value required");
                    (format!("!({v})"), Type::Bool)
                }
                "isNull" | "notNull" => {
                    let null = if matches!(t, Type::Optional(_) | Type::Null) {
                        format!("({v}).is_none()")
                    } else {
                        "false".into()
                    };
                    (
                        if op == "notNull" {
                            format!("!({null})")
                        } else {
                            null
                        },
                        Type::Bool,
                    )
                }
                "-" if t == Type::Int => (
                    format!("({v}).checked_neg().ok_or_else(||invalid(\"Integer overflow\"))?"),
                    Type::Int,
                ),
                "-" if t == Type::Real => (format!("-({v})"), Type::Real),
                _ => bail!("Unsupported typed unary operator {op}"),
            }
        }
        Expr::Rel { entity, field, via } => {
            let rows = related(schema, table, entity, via.as_deref(), row)?;
            let (value, ty) = member(schema, entity, "record", field)?;
            let (v, ty) = if matches!(ty, Type::Optional(_)) {
                (value, ty)
            } else {
                (format!("Some({value})"), Type::Optional(Box::new(ty)))
            };
            (
                format!(
                    "{{let rows={rows};match rows.as_slice(){{[]=>None,[record]=>{v},_=>return Err(invalid(\"Expected one related record\"))}}}}"
                ),
                ty,
            )
        }
        Expr::Agg {
            op,
            rel,
            field: column,
            pred,
        } => {
            let target = rel["table"]
                .as_str()
                .or_else(|| rel["entity"].as_str())
                .context("Aggregate needs entity")?;
            let rows = related(schema, table, target, rel["via"].as_str(), row)?;
            match op.as_str() {
                "count" => (
                    format!(
                        "i64::try_from(({rows}).len()).map_err(|_|invalid(\"Count overflow\"))?"
                    ),
                    Type::Int,
                ),
                "any" | "all" => {
                    let (p, t) = expr(
                        schema,
                        target,
                        "item",
                        pred.as_deref().context("Predicate required")?,
                    )?;
                    ensure!(t == Type::Bool, "Boolean predicate required");
                    let identity = op == "all";
                    let check = if identity { format!("!({p})") } else { p };
                    (
                        format!(
                            "{{let mut result={identity};for item in {rows}{{if {check}{{result={};break;}}}}result}}",
                            !identity
                        ),
                        Type::Bool,
                    )
                }
                "sum" => {
                    let (v, t) = member(
                        schema,
                        target,
                        "item",
                        column.as_deref().context("Sum field required")?,
                    )?;
                    let (nullable, base) = match t {
                        Type::Optional(inner) => (true, *inner),
                        _ => (false, t),
                    };
                    ensure!(
                        matches!(base, Type::Int | Type::Real),
                        "Sum requires a numeric field"
                    );
                    let zero = if base == Type::Int { "0i64" } else { "0f64" };
                    let add = if base == Type::Int {
                        "total=total.checked_add(value).ok_or_else(||invalid(\"Sum overflow\"))?;"
                    } else {
                        "total+=value;if !total.is_finite(){return Err(invalid(\"Sum overflow\"));}"
                    };
                    let step = if nullable {
                        format!("if let Some(value)={v}{{{add}}}")
                    } else {
                        format!("let value={v};{add}")
                    };
                    (
                        format!("{{let mut total={zero};for item in {rows}{{{step}}}total}}"),
                        base,
                    )
                }
                _ => bail!("Unsupported typed aggregate {op}"),
            }
        }
    })
}
pub(super) fn constraints(schema: &Schema, table: &str) -> Result<String> {
    let mut out = String::new();
    let group = &schema.constraints[table];
    for (name, c) in &group.check {
        let condition = c
            .condition
            .as_ref()
            .map(|e| expr(schema, table, "self", e))
            .transpose()?
            .unwrap_or(("true".into(), Type::Bool));
        ensure!(
            condition.1 == Type::Bool,
            "Constraint condition must be boolean"
        );
        if let Some(e) = &c.expression {
            let (e, t) = expr(schema, table, "self", e)?;
            ensure!(t == Type::Bool, "Constraint must be boolean");
            out.push_str(&format!(
                "if ({})&&!({e}){{return Err(invalid({:?}));}}",
                condition.0,
                format!("{name} constraint failed")
            ));
        }
    }
    for (name, c) in &group.unique {
        if c.enforcement != Enforcement::Application {
            continue;
        }
        let condition = c
            .condition
            .as_ref()
            .map(|e| expr(schema, table, "self", e))
            .transpose()?
            .unwrap_or(("true".into(), Type::Bool));
        ensure!(
            condition.1 == Type::Bool,
            "Constraint condition must be boolean"
        );
        let pk = primary(schema, table)?;
        let sql = format!(
            "SELECT count(*) FROM {} WHERE {}!=?{}",
            quote(table),
            quote(pk),
            c.fields
                .iter()
                .map(|f| format!(" AND {} IS ?", quote(&f.field)))
                .collect::<String>()
        );
        let mut fields = vec![format!("self.{}", field(pk))];
        fields.extend(c.fields.iter().map(|f| format!("self.{}", field(&f.field))));
        out.push_str(&format!("if {}{{let count:i64=service.db.query_row({sql:?},rusqlite::params![{}],|r|r.get(0))?;if count>0{{return Err(ServiceError::new(\"conflict\",{:?}));}}}}",condition.0,fields.join(","),format!("{name} must be unique")));
    }
    Ok(out)
}
