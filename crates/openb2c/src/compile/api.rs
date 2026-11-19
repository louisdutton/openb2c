//! Typed application operations behind the REST JSON boundary.
use super::{Type, field, record_name, typed};
use anyhow::{Context, Result, bail, ensure};
use openb2c_schema::{Assignments, Expr, Operation, Schema, SqlType, sql::quote};

fn primary<'a>(schema: &'a Schema, table: &str) -> Result<&'a str> {
    schema.tables[table]
        .iter()
        .find(|(_, c)| c.pk)
        .map(|(n, _)| n.as_str())
        .context("Operation needs a primary key")
}
fn convert(schema: &Schema, table: &str, column: &str, value: String, ty: Type) -> Result<String> {
    let c = &schema.tables[table][column];
    let target = match c.kind {
        SqlType::Integer => Type::Int,
        SqlType::Real => Type::Real,
        SqlType::Text => Type::Text,
        SqlType::Blob => bail!("Blob assignments need a typed input"),
    };
    let nullable = !c.required && !c.pk;
    if ty == Type::Null {
        ensure!(nullable, "Cannot clear required field {table}.{column}");
        return Ok("None".into());
    }
    let (value, base, optional) = match ty {
        Type::Optional(inner) => (value, *inner, true),
        _ => (value, ty, false),
    };
    ensure!(
        base == target,
        "Assignment type mismatch {table}.{column}: {base:?} requires {target:?}"
    );
    let enum_name = format!(
        "{}{}",
        super::super::generate::pascal(table),
        super::super::generate::pascal(column)
    );
    let wrap = |v: &str| {
        if c.kind == SqlType::Text && !c.constraints.values.is_empty() {
            format!("{enum_name}::try_from(AsRef::<str>::as_ref(&({v})))?")
        } else {
            v.to_owned()
        }
    };
    Ok(match (optional, nullable) {
        (false, false) => wrap(&value),
        (false, true) => format!("Some({})", wrap(&value)),
        (true, false) => wrap(&format!(
            "({value}).ok_or_else(||invalid(\"A required assignment is missing\"))?"
        )),
        (true, true) if c.kind == SqlType::Text && !c.constraints.values.is_empty() => {
            format!("({value}).map(|value|{enum_name}::try_from(value.as_str())).transpose()?")
        }
        (true, true) => value,
    })
}
fn assignments(schema: &Schema, table: &str, set: &Assignments, source: &str) -> Result<String> {
    let mut out = String::new();
    // Every expression reads the same original record, never an earlier assignment.
    for (name, a) in set {
        ensure!(a.source_count() == 1, "Invalid assignment {table}.{name}");
        let f = field(name);
        let value = if a.input == Some(true) {
            format!("input.{f}_input()?")
        } else {
            let (value, ty) = if let Some(value) = &a.value {
                typed::expr(
                    schema,
                    table,
                    source,
                    &Expr::Lit {
                        value: value.clone(),
                    },
                )?
            } else if a.clear == Some(true) {
                ("None::<String>".into(), Type::Null)
            } else if a.now == Some(true) {
                ("service.db.query_row(\"SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')\",[],|r|r.get::<_,String>(0))?".into(),Type::Text)
            } else {
                typed::expr(
                    schema,
                    table,
                    source,
                    a.expression
                        .as_ref()
                        .context("Missing assignment expression")?,
                )?
            };
            convert(schema, table, name, value, ty)?
        };
        out.push_str(&format!("let assignment_{f}={value};"));
    }
    for name in set.keys() {
        let f = field(name);
        out.push_str(&format!("changes.{f}=Change::Set(assignment_{f});"));
    }
    Ok(out)
}
fn guard(schema: &Schema, table: &str, op: &Operation, row: &str, create: bool) -> Result<String> {
    let mut code = String::new();
    if let Some(e) = &op.guard {
        let (e, t) = typed::expr(schema, table, row, e)?;
        ensure!(t == Type::Bool, "Operation guard must be boolean");
        code.push_str(&format!("if !({e}){{return Err(ServiceError::new(\"bad_state\",\"Operation guard rejected record\"));}}"));
    }
    if !create {
        for t in &op.transitions {
            let f = field(&t.field.field);
            let c = &schema.tables[table][&t.field.field];
            let value = if c.kind == SqlType::Text {
                format!("{row}.{f}.as_str().to_owned()")
            } else {
                format!("{row}.{f}.to_string()")
            };
            code.push_str(&format!("if !{:?}.contains(&({value}).as_str()){{return Err(ServiceError::new(\"bad_state\",\"Record is not in an allowed transition state\"));}}",t.from));
        }
    }
    Ok(code)
}
pub(super) fn authorization(schema: &Schema, table: &str, op: &Operation, row: &str) -> String {
    if op.public || op.relationships.is_empty() {
        return String::new();
    }
    let predicates = op
        .relationships
        .iter()
        .map(|r| {
            let f = field(&r.field.field);
            if schema.tables[table][&r.field.field].required {
                format!("{row}.{f}==id")
            } else {
                format!("{row}.{f}==Some(id)")
            }
        })
        .collect::<Vec<_>>()
        .join("||");
    format!(
        "if !auth.has_scope(\"*\")&&!auth.user_id.is_some_and(|id|{predicates}){{return Err(ServiceError::new(if auth.user_id.is_some(){{\"forbidden\"}}else{{\"unauthorized\"}},\"Record belongs to another user\"));}}"
    )
}
fn future(schema: &Schema, table: &str, row: &str, create: bool) -> Result<String> {
    let cols = &schema.tables[table];
    let mut out = String::new();
    for (name, c) in cols {
        if ["created_at", "updated_at"].contains(&name.as_str()) {
            continue;
        }
        let fmt = c.metadata["format"].as_str().unwrap_or("");
        if !["date", "time", "date-time"].contains(&fmt)
            && !name.ends_with("_at")
            && !name.ends_with("_date")
            && !name.ends_with("_time")
            && name != "date"
            && name != "time"
        {
            continue;
        }
        ensure!(
            c.kind == SqlType::Text,
            "Temporal field {table}.{name} must be text"
        );
        let date = fmt == "date" || name == "date" || name.ends_with("_date");
        let time = fmt == "time" || name == "time" || name.ends_with("_time");
        let paired_date = if name == "time" {
            "date".into()
        } else {
            format!("{}_date", name.strip_suffix("_time").unwrap_or(name))
        };
        let paired_time = if name == "date" {
            "time".into()
        } else {
            format!("{}_time", name.strip_suffix("_date").unwrap_or(name))
        };
        let get = |name: &str| -> String {
            if let Some(c) = cols.get(name) {
                let f = field(name);
                if c.required {
                    format!("Some({row}.{f}.as_str())")
                } else {
                    format!("{row}.{f}.as_deref()")
                }
            } else {
                "None".into()
            }
        };
        let present = if cols.contains_key(&paired_time) {
            if create {
                format!("!matches!(changes.{},Change::Keep)", field(&paired_time))
            } else {
                "true".into()
            }
        } else {
            "false".into()
        };
        let value = if c.required {
            "value.as_str()"
        } else {
            "value.as_deref()"
        };
        out.push_str(&format!(
            "if let Change::Set(value)=&changes.{}{{",
            field(name)
        ));
        if !c.required {
            out.push_str(&format!("if let Some(value)={value}{{"));
        }
        out.push_str(&format!(
            "crate::service::validate_temporal_text({name:?},{date},{time},{},{},{present},{})?;",
            get(&paired_date),
            get(&paired_time),
            if c.required { value } else { "value" }
        ));
        if !c.required {
            out.push('}');
        }
        out.push('}');
    }
    Ok(out)
}
pub fn compile(schema: &Schema) -> Result<String> {
    let mut out = String::from(
        "// Generated typed operations. Do not edit.\n#![allow(dead_code,unused_imports,unused_variables,unused_mut)]\nuse crate::{Service,AuthContext,records::*,b2c::invalid,service::{Result,ServiceError}};\nuse serde_json::Value;\nimpl Service {\n",
    );
    for (table, ops) in &schema.operations {
        let ty = record_name(table);
        let pk = primary(schema, table)?;
        let key = field(pk);
        for (action, op) in ops {
            if !op.enabled || action == "read" {
                continue;
            }
            let method = format!("typed_{table}_{action}");
            let create = action == "create";
            out.push_str(&format!("fn {method}(&self,id:&Value,payload:&Value,auth:&AuthContext,if_match:Option<&str>)->Result<Value>{{let service=self;self.authorize({table:?},{action:?},auth,None)?;let tx=crate::service::transaction(&self.db)?;"));
            if create {
                out.push_str(&format!("let input:{ty}Write=serde_json::from_value(payload.clone()).map_err(|e|invalid(e.to_string()))?;let mut changes=input.clone();"));
                for rel in &op.relationships {
                    if op.public {
                        continue;
                    }
                    let f = field(&rel.field.field);
                    let value = if schema.tables[table][&rel.field.field].required {
                        "id"
                    } else {
                        "Some(id)"
                    };
                    out.push_str(&format!("if !auth.has_scope(\"*\")&&let Some(id)=auth.user_id&&matches!(changes.{f},Change::Keep){{changes.{f}=Change::Set({value});}}"));
                }
                out.push_str("let draft=changes.clone();");
                out.push_str(&assignments(schema, table, &op.set, "draft")?);
                out.push_str("let row=changes.insert(self)?;");
                out.push_str(&guard(schema, table, op, "row", true)?);
                out.push_str(&authorization(schema, table, op, "row"));
                out.push_str(&future(schema, table, "row", true)?);
                out.push_str("row.audit_operation(self,\"create\",auth,None,false)?;let response=row.wire(self)?;crate::service::commit(tx)?;Ok(response)}\n");
                continue;
            }
            out.push_str(&format!("let key=serde_json::from_value(id.clone()).map_err(|e|invalid(format!(\"Invalid record ID: {{e}}\")))?;let old={ty}::get(self,key)?;"));
            out.push_str(&authorization(schema, table, op, "old"));
            out.push_str(&format!("if let Some(expected)=if_match{{let current=crate::service::etag({table:?},&old.wire(self)?);if !expected.split(',').map(str::trim).any(|v|v==\"*\"||v==current){{return Err(ServiceError::new(\"conflict\",\"Record has changed; refresh and retry\"));}}}}"));
            out.push_str(&guard(schema, table, op, "old", false)?);
            if action == "delete" {
                let sql = format!("DELETE FROM {} WHERE {}=?", quote(table), quote(pk));
                out.push_str(&format!("self.db.execute({sql:?},[&old.{key}])?;old.audit_operation(self,\"delete\",auth,Some(&old),true)?;crate::service::commit(tx)?;Ok(serde_json::json!({{\"id\":id,\"deleted\":true}}))}}\n"));
                continue;
            }
            out.push_str(&format!("let input:{ty}Write=serde_json::from_value(payload.clone()).map_err(|e|invalid(e.to_string()))?;let mut changes:{ty}Write="));
            out.push_str(if action == "update" {
                "input.clone();"
            } else {
                "Default::default();"
            });
            out.push_str("let original=changes.apply(&old);");
            if table == "api_key" && action == "update" {
                out.push_str("if [\"key_hash\",\"key_prefix\",\"user_id\"].iter().any(|f|payload.get(f).is_some()){return Err(ServiceError::new(\"forbidden\",\"Key credentials and owner are immutable\"));}if let Change::Set(Some(scopes))=&input.scopes{if scopes.split(',').map(str::trim).any(|s|!auth.has_scope(s)){return Err(ServiceError::new(\"forbidden\",\"Cannot grant scopes you do not hold\"));}}");
            }
            out.push_str(&assignments(schema, table, &op.set, "original")?);
            out.push_str("let merged=changes.apply(&old);");
            out.push_str(&authorization(schema, table, op, "merged"));
            out.push_str(&future(schema, table, "merged", false)?);
            if schema.tables[table].contains_key("updated_at") {
                let c = &schema.tables[table]["updated_at"];
                let value = if c.required {
                    "crate::identity::now()"
                } else {
                    "Some(crate::identity::now())"
                };
                out.push_str(&format!(
                    "if !changes.is_empty(){{changes.updated_at=Change::Set({value});}}"
                ));
            }
            out.push_str(&format!("let row=changes.update(self,old.{key}.clone())?;"));
            for c in &op.cascade {
                let target = &c.entity.table;
                let target_ty = record_name(target);
                let rows = if let Some(f) = c.via.as_ref().and_then(|v| v["field"].as_str()) {
                    ensure!(
                        schema.tables[target].contains_key(f),
                        "Invalid cascade field"
                    );
                    format!(
                        "{target_ty}::query(self,{:?},&[&row.{key}])?",
                        format!("SELECT * FROM {} WHERE {}=?", quote(target), quote(f))
                    )
                } else {
                    typed::related(
                        schema,
                        table,
                        target,
                        c.via.as_ref().and_then(|v| v["table"].as_str()),
                        "row",
                    )?
                };
                out.push_str(&format!(
                    "for original in {rows}{{let mut changes={target_ty}Write::default();"
                ));
                // Cascades with input assignments use the target's typed input contract.
                if c.set.values().any(|a| a.input == Some(true)) {
                    out.push_str(&format!("let input:{target_ty}Write=serde_json::from_value(payload.clone()).map_err(|e|invalid(e.to_string()))?;"));
                }
                out.push_str(&assignments(schema, target, &c.set, "original")?);
                out.push_str(&format!(
                    "changes.update(self,original.{}.clone())?;}}",
                    field(primary(schema, target)?)
                ));
            }
            out.push_str(&format!("row.audit_operation(self,{action:?},auth,Some(&old),false)?;let response=row.wire(self)?;crate::service::commit(tx)?;Ok(response)}}\n"));
        }
    }
    out.push_str("pub fn create(&self,table:&str,input:&Value,auth:&AuthContext)->Result<Value>{if table==\"api_key\"{return self.create_api_key(input,auth);}let mut payload=input.clone();if let Some(fields)=payload.as_object_mut(){fields.remove(\"created_at\");fields.remove(\"updated_at\");}match table{");
    for (table, ops) in &schema.operations {
        if ops.get("create").is_some_and(|op| op.enabled) {
            out.push_str(&format!(
                "{table:?}=>self.typed_{table}_create(&Value::Null,&payload,auth,None),"
            ));
        }
    }
    out.push_str("_=>Err(ServiceError::new(\"not_found\",\"Operation is disabled\")),}}\n");
    out.push_str("pub fn mutate(&self,table:&str,id:&Value,action:&str,input:&Value,auth:&AuthContext,if_match:Option<&str>)->Result<Value>{let mut payload=input.clone();if let Some(fields)=payload.as_object_mut(){fields.remove(\"created_at\");fields.remove(\"updated_at\");}match(table,action){");
    for (table, ops) in &schema.operations {
        for (action, op) in ops {
            if op.enabled && !["read", "create"].contains(&action.as_str()) {
                out.push_str(&format!("({table:?},{action:?})=>self.typed_{table}_{action}(id,&payload,auth,if_match),"));
            }
        }
    }
    out.push_str(
        "_=>Err(ServiceError::new(\"not_found\",\"Unknown or disabled mutation operation\")),}}\n",
    );
    out.push_str("pub fn find(&self,table:&str,id:&Value,auth:&AuthContext)->Result<Value>{self.authorize(table,\"read\",auth,None)?;match table{");
    for (table, ops) in &schema.operations {
        if let Some(op) = ops.get("read").filter(|op| op.enabled) {
            out.push_str(&format!("{table:?}=>{{let row={}::get(self,serde_json::from_value(id.clone()).map_err(|e|invalid(format!(\"Invalid record ID: {{e}}\")))?)?;{}row.wire(self)}},",record_name(table),authorization(schema,table,op,"row")));
        }
    }
    out.push_str(
        "_=>Err(ServiceError::new(\"not_found\",\"Unknown or disabled read operation\")),}}\n}\n",
    );
    Ok(out)
}
