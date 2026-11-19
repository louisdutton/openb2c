//! Lower declarative record operations to Rust predicates and fixed relationship queries.
use anyhow::{Context, Result};
use openb2c_schema::{Schema, SqlType, sql::quote};
use serde_json::{Value, json};

fn primary<'a>(schema: &'a Schema, table: &str) -> Result<&'a str> {
    schema
        .tables
        .get(table)
        .context("Unknown expression table")?
        .iter()
        .find(|(_, c)| c.pk)
        .map(|(n, _)| n.as_str())
        .context("Table needs a primary key")
}
pub fn compile(schema: &Schema) -> Result<String> {
    let mut out = String::from(
        "// Generated record operations; no expression AST is executed.\n#![allow(unused_variables,unused_mut,unused_imports,clippy::nonminimal_bool,clippy::let_and_return,clippy::needless_borrow)]\nuse crate::{Service,AuthContext,service::{Result,ServiceError,sql_value},records::*};\nuse openb2c_schema::{Column,Operation};\nuse serde_json::{Value,Map,json};\nuse rusqlite::{types::Value as SqlValue,params_from_iter};\nuse std::collections::BTreeMap;\nimpl Service {\n",
    );
    out.push_str("pub fn primary_key(&self,table:&str)->Result<&str>{match table{\n");
    for table in schema.tables.keys() {
        out.push_str(&format!("{table:?}=>Ok({:?}),\n", primary(schema, table)?));
    }
    out.push_str("_=>Err(ServiceError::new(\"not_found\",\"Unknown entity\"))}}\n");
    out.push_str(
        "pub fn columns(&self,table:&str)->Result<&BTreeMap<String,Column>>{match table{\n",
    );
    for (table, cols) in &schema.tables {
        let json = serde_json::to_string(cols)?;
        out.push_str(&format!("{table:?}=>{{static C:std::sync::LazyLock<BTreeMap<String,Column>>=std::sync::LazyLock::new(||serde_json::from_str({json:?}).expect(\"Compiled columns\"));Ok(&C)}},\n"));
    }
    out.push_str("_=>Err(ServiceError::new(\"not_found\",\"Unknown entity\"))}}\n");
    out.push_str("pub(crate) fn policy(&self,table:&str,action:&str)->Result<Operation>{self.primary_key(table)?;match(table,action){\n");
    for (table, ops) in &schema.operations {
        for (action, op) in ops {
            let json = json!({"audit":op.audit,"effects":op.effects}).to_string();
            out.push_str(&format!("({table:?},{action:?})=>{{static P:std::sync::LazyLock<Operation>=std::sync::LazyLock::new(||serde_json::from_str({json:?}).expect(\"Compiled audit metadata\"));Ok(P.clone())}},\n"));
        }
    }
    out.push_str("_=>Ok(Operation::default())}}\n");
    out.push_str("pub fn authorize(&self,table:&str,action:&str,auth:&AuthContext,row:Option<&Value>)->Result<()>{match(table,action){\n");
    for (table, ops) in &schema.operations {
        for (action, op) in ops {
            let code = if !op.enabled {
                "return Err(ServiceError::new(\"not_found\",\"Operation is disabled\"));".into()
            } else if table.starts_with("identity_") {
                "return Err(ServiceError::new(\"forbidden\",\"Identity storage is managed through identity endpoints\"));".into()
            } else {
                let allowed = if op.public {
                    "true".into()
                } else {
                    let scope = op
                        .scope
                        .clone()
                        .unwrap_or_else(|| format!("{table}.{action}"));
                    let relationships = if op.relationships.is_empty() {
                        "true".into()
                    } else {
                        format!(
                            "auth.user_id.is_some_and(|id| row.is_none_or(|row| {}))",
                            op.relationships
                                .iter()
                                .map(|r| format!("row[{:?}].as_i64()==Some(id)", r.field.field))
                                .collect::<Vec<_>>()
                                .join("||")
                        )
                    };
                    format!(
                        "auth.has_scope(\"*\") || (auth.has_scope({scope:?}) && ({relationships}))"
                    )
                };
                format!(
                    "if {allowed}{{Ok(())}}else{{Err(ServiceError::new(if auth.user_id.is_some(){{\"forbidden\"}}else{{\"unauthorized\"}},{:?}))}}",
                    format!("Not authorized to {action} {table}")
                )
            };
            out.push_str(&format!("({table:?},{action:?})=>{{{code}}},\n"));
        }
    }
    out.push_str("_=>Err(ServiceError::new(\"not_found\",\"Unknown operation\"))}}\n");
    out.push_str("pub(crate) fn auth_filter(&self,table:&str,action:&str,auth:&AuthContext,clauses:&mut Vec<String>,values:&mut Vec<SqlValue>)->Result<()>{self.authorize(table,action,auth,None)?;if auth.has_scope(\"*\"){return Ok(());}match(table,action){\n");
    for (table, ops) in &schema.operations {
        for (action, op) in ops {
            if op.public || op.relationships.is_empty() {
                continue;
            }
            let sql = op
                .relationships
                .iter()
                .map(|r| format!("{}=?", quote(&r.field.field)))
                .collect::<Vec<_>>()
                .join(" OR ");
            out.push_str(&format!("({table:?},{action:?})=>{{clauses.push({:?}.into());let id=auth.user_id.ok_or_else(||ServiceError::new(\"unauthorized\",\"Authentication required\"))?;",format!("({sql})")));
            for _ in &op.relationships {
                out.push_str("values.push(SqlValue::Integer(id));");
            }
            out.push_str("},\n");
        }
    }
    out.push_str("_=>{}}Ok(())}\n");
    out.push_str("pub fn redact(&self,table:&str,row:&mut Value){let Some(row)=row.as_object_mut()else{return;};match table{\n");
    for (table, cols) in &schema.tables {
        let mut fields = cols
            .iter()
            .filter(|(n, c)| !super::super::generate::secret_field(n, &c.metadata))
            .map(|(n, _)| n)
            .collect::<Vec<_>>();
        fields.extend(
            schema
                .derived
                .get(table)
                .into_iter()
                .flatten()
                .filter(|(n, c)| !super::super::generate::secret_field(n, &c.metadata))
                .map(|(n, _)| n),
        );
        let condition = if fields.is_empty() {
            "false".into()
        } else {
            format!(
                "matches!(field.as_str(),{})",
                fields
                    .iter()
                    .map(|n| format!("{n:?}"))
                    .collect::<Vec<_>>()
                    .join("|")
            )
        };
        out.push_str(&format!("{table:?}=>row.retain(|field,_|{condition}),\n"));
    }
    out.push_str("_=>{}}}\n");

    out.push_str(&future_and_scopes(schema)?);
    out.push_str(&listings(schema)?);
    out.push_str(&audit_defaults(schema));
    out.push_str(&protocol_settings(schema));
    out.push_str("}\n");
    Ok(out)
}

fn future_and_scopes(schema: &Schema) -> Result<String> {
    let mut out = String::new();
    let mut scopes = schema
        .operations
        .iter()
        .flat_map(|(t, ops)| {
            ops.iter()
                .filter(|(_, op)| op.enabled && !op.public && !op.relationships.is_empty())
                .map(move |(name, op)| op.scope.clone().unwrap_or_else(|| format!("{t}.{name}")))
        })
        .collect::<Vec<_>>();
    scopes.extend(openb2c_schema::capabilities::customer_scopes(schema));
    out.push_str(&format!(
        "pub fn self_scopes(&self)->Vec<String>{{vec![{}]}}\n",
        scopes
            .iter()
            .map(|s| format!("{s:?}.to_string()"))
            .collect::<Vec<_>>()
            .join(",")
    ));
    Ok(out)
}

fn listings(schema: &Schema) -> Result<String> {
    let mut out = String::from(
        "pub(crate) fn column_kind(&self,table:&str,field:&str)->Result<openb2c_schema::SqlType>{match(table,field){\n",
    );
    for (table, cols) in &schema.tables {
        for (name, c) in cols {
            let ty = match c.kind {
                SqlType::Integer => "Integer",
                SqlType::Real => "Real",
                SqlType::Text => "Text",
                SqlType::Blob => "Blob",
            };
            out.push_str(&format!(
                "({table:?},{name:?})=>Ok(openb2c_schema::SqlType::{ty}),\n"
            ));
        }
    }
    out.push_str("_=>Err(ServiceError::new(\"invalid\",\"Unknown field\"))}}\n");
    out.push_str("pub fn raw(&self,table:&str,id:&Value)->Result<Value>{match table{\n");
    for table in schema.tables.keys() {
        out.push_str(&format!("{table:?}=>serde_json::to_value({}::get(self,serde_json::from_value(id.clone()).map_err(|e|ServiceError::new(\"invalid\",e.to_string()))?)?).map_err(|e|ServiceError::new(\"invalid\",e.to_string())),\n",super::record_name(table)));
    }
    out.push_str("_=>Err(ServiceError::new(\"not_found\",\"Unknown entity\")),}}\n");
    out.push_str("pub(crate) fn export_rows(&self,table:&str)->Result<Vec<Value>>{match table{");
    for table in schema.tables.keys() {
        out.push_str(&format!(
            "{table:?}=>{}::select(self,{:?},&[])?.iter().map(|row|row.wire(self)).collect(),",
            super::record_name(table),
            format!("ORDER BY {}", quote(primary(schema, table)?))
        ));
    }
    out.push_str("_=>Err(ServiceError::new(\"not_found\",\"Unknown entity\")),}}\n");
    out.push_str("pub fn list(&self,table:&str,options:&crate::service::ListOptions,auth:&AuthContext)->Result<Value>{let mut clauses=Vec::new();let mut values=Vec::new();self.auth_filter(table,\"read\",auth,&mut clauses,&mut values)?;let(select,count,primary)=match table{\n");
    for (table, cols) in &schema.tables {
        let select = format!(
            "SELECT {} FROM {}",
            cols.keys().map(|f| quote(f)).collect::<Vec<_>>().join(","),
            quote(table)
        );
        let count = format!("SELECT count(*) FROM {}", quote(table));
        let pk = primary(schema, table)?;
        out.push_str(&format!("{table:?}=>({select:?},{count:?},{pk:?}),\n"));
    }
    out.push_str("_=>return Err(ServiceError::new(\"not_found\",\"Unknown entity\")),};for(field,value)in &options.filter{let clause=match(table,field.as_str()){\n");
    for (table, cols) in &schema.tables {
        for (name, c) in cols {
            if super::super::generate::secret_field(name, &c.metadata) {
                continue;
            }
            out.push_str(&format!(
                "({table:?},{name:?})=>{:?},\n",
                format!("{} IS ?", quote(name))
            ));
        }
    }
    out.push_str("_=>return Err(ServiceError::new(\"invalid\",\"Unknown or secret filter field\")),};clauses.push(clause.into());values.push(sql_value(value)?);}let condition=if clauses.is_empty(){String::new()}else{format!(\" WHERE {}\",clauses.join(\" AND \"))};let total:i64=self.db.query_row(&format!(\"{count}{condition}\"),params_from_iter(&values),|r|r.get(0))?;let sort=options.sort.as_deref().unwrap_or(primary);let sort=match(table,sort){\n");
    for (table, cols) in &schema.tables {
        for (name, c) in cols {
            if super::super::generate::secret_field(name, &c.metadata) {
                continue;
            }
            out.push_str(&format!("({table:?},{name:?})=>{:?},\n", quote(name)));
        }
    }
    out.push_str("_=>return Err(ServiceError::new(\"invalid\",\"Unknown or secret sort field\")),};let limit=options.limit.clamp(1,1000);values.push(SqlValue::Integer(i64::from(limit)));values.push(SqlValue::Integer(i64::from(options.offset)));let sql=format!(\"{select}{condition} ORDER BY {sort} {} LIMIT ? OFFSET ?\",if options.descending{\"DESC\"}else{\"ASC\"});let params=values.iter().map(|v|v as &dyn rusqlite::ToSql).collect::<Vec<_>>();let items=match table{");
    for table in schema.tables.keys() {
        out.push_str(&format!("{table:?}=>{}::query(self,&sql,&params)?.iter().map(|row|row.wire(self)).collect::<Result<Vec<_>>>()?,",super::record_name(table)));
    }
    out.push_str("_=>return Err(ServiceError::new(\"not_found\",\"Unknown entity\")),};Ok(json!({\"items\":items,\"total\":total,\"limit\":limit,\"offset\":options.offset}))}\n");
    Ok(out)
}

fn audit_defaults(schema: &Schema) -> String {
    let mut out = String::from(
        "pub(crate) fn audit_defaults(&self,table:&str,action:&str)->(bool,Option<&str>,Option<&str>){match(table,action){\n",
    );
    for (table, audit) in schema.metadata.get("audit").unwrap_or(&Value::Null)["entities"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let category = audit["category"]
            .as_str()
            .map(|s| format!("Some({s:?})"))
            .unwrap_or("None".into());
        let reason = audit["reason"]
            .as_str()
            .map(|s| format!("Some({s:?})"))
            .unwrap_or("None".into());
        for action in audit["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            out.push_str(&format!(
                "({table:?},{action:?})=>(true,{category},{reason}),\n"
            ));
        }
        out.push_str(&format!("({table:?},_)=>(false,{category},{reason}),\n"));
    }
    out.push_str("_=>(false,None,None)}}\n");
    out
}

fn protocol_settings(schema: &Schema) -> String {
    let signing =
        &schema.metadata.get("integrations").unwrap_or(&Value::Null)["webhookEffects"]["signing"];
    let value = if signing["enabled"] == true {
        format!(
            "Some(({:?},{:?}))",
            signing["timestampHeader"]
                .as_str()
                .unwrap_or("X-OpenB2C-Timestamp"),
            signing["signatureHeader"]
                .as_str()
                .unwrap_or("X-OpenB2C-Signature")
        )
    } else {
        "None".into()
    };
    format!("pub(crate) fn webhook_signing(&self)->Option<(&str,&str)>{{{value}}}\n")
}
