//! Concrete persistence records and writes for the evaluated application.
use super::{field, record_name};
use anyhow::{Result, ensure};
use openb2c_schema::{Schema, SqlType, sql::quote};
use std::collections::BTreeSet;

fn variant(value: &str) -> String {
    let identifier = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    let name = super::super::generate::pascal(&identifier);
    if name == "Self" {
        "SelfValue".into()
    } else {
        name
    }
}

pub fn compile(schema: &Schema) -> Result<String> {
    let mut out = String::from(
        r#"// Generated database records. Do not edit.
#![allow(dead_code,unused_imports,unused_variables)]
use crate::{Service,AuthContext,b2c::invalid,service::{Result,ServiceError}};
use serde::{Serialize,Deserialize};
use rusqlite::{ToSql,OptionalExtension,params_from_iter};
#[derive(Clone,Debug)]
pub(crate) enum Change<T> { Keep, Set(T) }
impl<T> Default for Change<T> {fn default()->Self{Self::Keep}}
impl<T> From<T> for Change<T> { fn from(value:T)->Self { Self::Set(value) } }
impl<'de,T:Deserialize<'de>> Deserialize<'de> for Change<T>{fn deserialize<D:serde::Deserializer<'de>>(deserializer:D)->std::result::Result<Self,D::Error>{T::deserialize(deserializer).map(Self::Set)}}
pub(crate) trait Record: Sized + Serialize {
    type Key:ToSql+Clone+Serialize;
    const TABLE: &'static str;
    fn read(row:&rusqlite::Row<'_>)->rusqlite::Result<Self>;
    fn id(&self)->Self::Key;
    fn query(service:&Service,sql:&str,values:&[&dyn ToSql])->Result<Vec<Self>> {
        let mut statement=service.db.prepare(sql)?;
        Ok(statement.query_map(values,Self::read)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    fn get(service:&Service,id:Self::Key)->Result<Self>;
    fn select(service:&Service,suffix:&str,values:&[&dyn ToSql])->Result<Vec<Self>> {
        Self::query(service,&format!("SELECT * FROM \"{}\" {suffix}",Self::TABLE),values)
    }
    fn first(service:&Service,suffix:&str,values:&[&dyn ToSql])->Result<Option<Self>> {
        let mut statement=service.db.prepare(&format!("SELECT * FROM \"{}\" {suffix}",Self::TABLE))?;
        Ok(statement.query_row(values,Self::read).optional()?)
    }
    fn wire(&self,service:&Service)->Result<serde_json::Value>;
    fn audit(&self,service:&Service,action:&str,auth:&AuthContext,before:Option<&Self>)->Result<()> {
        let before=before.map(serde_json::to_value).transpose().map_err(|e|invalid(e.to_string()))?;
        let after=serde_json::to_value(self).map_err(|e|invalid(e.to_string()))?;
        service.cap_audit(Self::TABLE,action,auth,before.as_ref(),&after)
    }
    fn audit_operation(&self,service:&Service,action:&str,auth:&AuthContext,before:Option<&Self>,deleted:bool)->Result<()> {
        let before=before.map(serde_json::to_value).transpose().map_err(|e|invalid(e.to_string()))?;
        let after=if deleted{None}else{Some(serde_json::to_value(self).map_err(|e|invalid(e.to_string()))?)};
        let id=serde_json::to_value(self.id()).map_err(|e|invalid(e.to_string()))?;
        service.record(Self::TABLE,&id,action,auth,(before.as_ref(),after.as_ref()),&service.policy(Self::TABLE,action)?)
    }
}
pub(crate) trait OwnedRecord:Record<Key=i64> {fn owner(&self)->i64;}
impl Service {
    pub(crate) fn owned<R:OwnedRecord>(&self,id:i64,cap:&str,scope:&str,auth:&AuthContext)->Result<R>{
        self.cap_command(cap,scope,auth)?;let row=R::get(self,id)?;
        if !auth.has_scope("*")&&Some(row.owner())!=auth.user_id{return Err(ServiceError::new("forbidden","Record belongs to another customer"));}Ok(row)
    }
}
"#,
    );
    let mut names = BTreeSet::new();
    for (table, cols) in &schema.tables {
        let name = record_name(table);
        ensure!(names.insert(name.clone()), "Record type collision {name}");
        let mut fields = BTreeSet::new();
        for column in cols.keys().chain(
            schema
                .derived
                .get(table)
                .into_iter()
                .flat_map(|fields| fields.keys()),
        ) {
            ensure!(
                fields.insert(field(column)),
                "Rust field name collision in {table}: {column}"
            );
        }
        let Some((pk, primary)) = cols.iter().find(|(_, c)| c.pk) else {
            continue;
        };
        let mut types = Vec::new();
        for (column, c) in cols {
            let native = if c.kind == SqlType::Text && !c.constraints.values.is_empty() {
                let en = format!(
                    "{}{}",
                    super::super::generate::pascal(table),
                    super::super::generate::pascal(column)
                );
                ensure!(names.insert(en.clone()), "Record enum collision {en}");
                let mut variants = BTreeSet::new();
                out.push_str(&format!("#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)] pub(crate) enum {en} {{"));
                for value in &c.constraints.values {
                    let v = variant(value);
                    ensure!(
                        variants.insert(v.clone()),
                        "Enum variant collision {en}.{v}"
                    );
                    out.push_str(&format!("#[serde(rename={value:?})]{v},"));
                }
                out.push_str(&format!(
                    "}} impl {en} {{ pub(crate) fn as_str(&self)->&'static str {{match self{{"
                ));
                for value in &c.constraints.values {
                    out.push_str(&format!("Self::{}=>{value:?},", variant(value)));
                }
                out.push_str("}}}\n");
                out.push_str(&format!("impl TryFrom<&str> for {en} {{type Error=ServiceError;fn try_from(value:&str)->Result<Self>{{match value{{"));
                for value in &c.constraints.values {
                    out.push_str(&format!("{value:?}=>Ok(Self::{}),", variant(value)));
                }
                out.push_str("_=>Err(invalid(\"Invalid enum value\"))}}}\n");
                out.push_str(&format!("impl rusqlite::types::FromSql for {en} {{fn column_result(value:rusqlite::types::ValueRef<'_>)->rusqlite::types::FromSqlResult<Self>{{match value.as_str()?{{"));
                for value in &c.constraints.values {
                    out.push_str(&format!("{value:?}=>Ok(Self::{}),", variant(value)));
                }
                out.push_str("_=>Err(rusqlite::types::FromSqlError::InvalidType)}}}\n");
                out.push_str(&format!("impl ToSql for {en} {{fn to_sql(&self)->rusqlite::Result<rusqlite::types::ToSqlOutput<'_>>{{Ok(self.as_str().into())}}}}\n"));
                en
            } else {
                c.kind.rust().into()
            };
            types.push((
                column,
                c,
                if c.required || c.pk {
                    native
                } else {
                    format!("Option<{native}>")
                },
            ));
        }
        out.push_str(&format!(
            "#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)] pub(crate) struct {name} {{"
        ));
        for (column, _, ty) in &types {
            out.push_str(&format!(
                "#[serde(rename={column:?})] pub(crate) {}:{ty},",
                field(column)
            ));
        }
        let key_type = &types
            .iter()
            .find(|(n, _, _)| *n == pk)
            .expect("Primary key")
            .2;
        out.push_str(&format!("}} impl Record for {name} {{type Key={key_type};const TABLE:&'static str={table:?};fn id(&self)->Self::Key{{self.{}.clone()}} fn read(row:&rusqlite::Row<'_>)->rusqlite::Result<Self>{{Ok(Self{{",field(pk)));
        for (column, _, _) in &types {
            out.push_str(&format!("{}:row.get({column:?})?,", field(column)));
        }
        let get = format!("SELECT * FROM {} WHERE {}=?", quote(table), quote(pk));
        out.push_str(&format!("}})}} fn get(service:&Service,id:Self::Key)->Result<Self>{{let mut statement=service.db.prepare({get:?})?; statement.query_row([id],Self::read).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){{ServiceError::new(\"not_found\",\"Record not found\")}}else{{e.into()}})}}"));
        out.push_str("fn wire(&self,service:&Service)->Result<serde_json::Value>{let mut response=serde_json::to_value(self).map_err(|e|invalid(e.to_string()))?;");
        for (column, d) in schema.derived.get(table).into_iter().flatten() {
            if super::super::generate::secret_field(column, &d.metadata) {
                continue;
            }
            let value = if let Some(e) = &d.expression {
                super::typed::expr(schema, table, "self", e)?.0
            } else if let Some(template) = &d.template {
                let mut code = format!("{{let mut text={template:?}.to_owned();");
                for dep in &d.dependencies {
                    let e = if dep.table == *table {
                        openb2c_schema::Expr::Field {
                            name: dep.field.clone(),
                        }
                    } else {
                        openb2c_schema::Expr::Rel {
                            entity: dep.table.clone(),
                            field: dep.field.clone(),
                            via: None,
                        }
                    };
                    let (v, t) = super::typed::expr(schema, table, "self", &e)?;
                    let display = match t {
                        super::Type::Optional(_) => {
                            format!("({v}).map(|v|v.to_string()).unwrap_or_default()")
                        }
                        super::Type::Null => "String::new()".into(),
                        _ => format!("({v}).to_string()"),
                    };
                    code.push_str(&format!(
                        "let value={display};text=text.replace({:?},&value).replace({:?},&value);",
                        format!("{{{}}}", dep.field),
                        format!("{{{}.{}}}", dep.table, dep.field)
                    ));
                }
                code.push_str("text}");
                code
            } else {
                anyhow::bail!("Derived field {table}.{column} lacks an expression")
            };
            out.push_str(&format!("response[{column:?}]=serde_json::to_value({value}).map_err(|e|invalid(e.to_string()))?;"));
        }
        out.push_str(&format!(
            "service.redact({table:?},&mut response);Ok(response)}}}}\n"
        ));
        if primary.kind == SqlType::Integer
            && cols
                .get("user_id")
                .is_some_and(|c| c.kind == SqlType::Integer && c.required)
        {
            out.push_str(&format!(
                "impl OwnedRecord for {name}{{fn owner(&self)->i64{{self.user_id}}}}\n"
            ));
        }
        out.push_str(&format!(
            "#[derive(Default,Clone,Debug,Deserialize)] #[serde(deny_unknown_fields)] pub(crate) struct {name}Write {{"
        ));
        for (column, c, ty) in &types {
            if !c.pk || !c.auto {
                out.push_str(&format!(
                    "#[serde(default,rename={column:?})] pub(crate) {}:Change<{ty}>,",
                    field(column)
                ));
            }
        }
        out.push_str(&format!("}} impl {name}Write {{fn values(&self)->Result<Vec<(&'static str,&dyn ToSql)>>{{let mut values:Vec<(&str,&dyn ToSql)>=Vec::new();"));
        for (column, c, _) in &types {
            if c.pk && c.auto {
                continue;
            }
            out.push_str(&format!(
                "if let Change::Set(value)=&self.{}{{",
                field(column)
            ));
            let nullable = !c.required && !c.pk;
            if nullable {
                out.push_str("if let Some(value)=value{");
            }
            if c.kind == SqlType::Text {
                {
                    let fmt = c.metadata["format"].as_str().unwrap_or(column);
                    if [
                        "email",
                        "phone",
                        "postcode",
                        "date",
                        "time",
                        "date-time",
                        "url",
                        "uri",
                    ]
                    .contains(&fmt)
                    {
                        out.push_str(&format!(
                            "crate::service::validate_format({column:?},{fmt:?},value.as_str())?;"
                        ));
                    }
                    if let Some(pattern) = &c.constraints.pattern {
                        out.push_str(&format!("{{static P:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new({pattern:?}).expect(\"Validated pattern\"));if !P.is_match(value.as_str()){{return Err(invalid({:?}));}}}}",format!("{column} has an invalid format")));
                    }
                }
                if let Some(b) = &c.constraints.length {
                    let text = "value.as_str()";
                    out.push_str(&format!("let length={text}.chars().count() as f64;"));
                    for (bound, op) in [(b.min, "<"), (b.max, ">")] {
                        if let Some(v) = bound {
                            out.push_str(&format!(
                                "if length{op}{v:?}{{return Err(invalid({:?}));}}",
                                format!("{column} is outside its allowed bounds")
                            ));
                        }
                    }
                }
            }
            if matches!(c.kind, SqlType::Integer | SqlType::Real) {
                if !c.constraints.values.is_empty() {
                    let values = c
                        .constraints
                        .values
                        .iter()
                        .map(|s| s.parse::<f64>())
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    out.push_str(&format!(
                        "if !{:?}.contains(&(*value as f64)){{return Err(invalid({:?}));}}",
                        values,
                        format!("{column} has an invalid enum value")
                    ));
                }
                if c.kind == SqlType::Real {
                    out.push_str("if !value.is_finite(){return Err(invalid(\"A finite number is required\"));}");
                }
                if let Some(b) = &c.constraints.range {
                    for (bound, op) in [(b.min, "<"), (b.max, ">")] {
                        if let Some(v) = bound {
                            out.push_str(&format!(
                                "if (*value as f64){op}{v:?}{{return Err(invalid({:?}));}}",
                                format!("{column} is outside its allowed bounds")
                            ));
                        }
                    }
                }
            }
            if nullable {
                out.push('}');
            }
            out.push_str(&format!("values.push(({:?},value));}}", quote(column)));
        }
        out.push_str("Ok(values)}\n");
        out.push_str(&format!(
            "pub(crate) fn insert(&self,service:&Service)->Result<{name}>{{"
        ));
        for (column, c, _) in &types {
            if c.required && !c.auto && c.default.is_none() {
                out.push_str(&format!(
                    "if matches!(self.{},Change::Keep){{return Err(invalid({:?}));}}",
                    field(column),
                    format!("{column} is required")
                ));
            }
        }
        let insert = format!("INSERT INTO {}", quote(table));
        let key = if primary.auto {
            "service.db.last_insert_rowid()".into()
        } else {
            format!("self.{}_value()?", field(pk))
        };
        out.push_str(&format!("let values=self.values()?; let sql=if values.is_empty(){{format!(\"{{}} DEFAULT VALUES\",{insert:?})}}else{{format!(\"{{}} ({{}}) VALUES ({{}})\",{insert:?},values.iter().map(|(n,_)|*n).collect::<Vec<_>>().join(\",\"),vec![\"?\";values.len()].join(\",\"))}}; service.db.execute(&sql,params_from_iter(values.iter().map(|(_,v)|*v)))?; let row={name}::get(service,{key})?;row.check(service)?;Ok(row)}}\n"));
        let update = format!("UPDATE {} SET {{}} WHERE {}=?", quote(table), quote(pk));
        out.push_str(&format!(
            "pub(crate) fn update(&self,service:&Service,id:{key_type})->Result<{name}>{{"
        ));
        if !primary.auto {
            out.push_str(&format!("if !matches!(self.{},Change::Keep){{return Err(invalid(\"Primary keys are immutable\"));}}",field(pk)));
        }
        out.push_str(&format!("let values=self.values()?;if !values.is_empty(){{let sql=format!({update:?},values.iter().map(|(n,_)|format!(\"{{n}}=?\")).collect::<Vec<_>>().join(\",\"));let mut params=values.iter().map(|(_,v)|*v).collect::<Vec<_>>();params.push(&id);service.db.execute(&sql,params_from_iter(params))?;}}let row={name}::get(service,id)?;row.check(service)?;Ok(row)}}\n"));
        out.push_str(&format!(
            "pub(crate) fn apply(&self,old:&{name})->{name}{{let mut row=old.clone();"
        ));
        for (column, c, _) in &types {
            if !c.pk || !c.auto {
                let f = field(column);
                out.push_str(&format!(
                    "if let Change::Set(value)=&self.{f}{{row.{f}=value.clone();}}"
                ));
            }
        }
        out.push_str("row}\npub(crate) fn is_empty(&self)->bool{true");
        for (column, c, _) in &types {
            if !c.pk || !c.auto {
                out.push_str(&format!("&&matches!(self.{},Change::Keep)", field(column)));
            }
        }
        out.push_str("}\n");
        for (column, c, ty) in &types {
            let f = field(column);
            out.push_str(&format!("pub(crate) fn {f}_value(&self)->Result<{ty}>{{"));
            if c.pk && c.auto {
                out.push_str(
                    "Err(invalid(\"Generated identity is not available before insertion\"))",
                );
            } else {
                let missing = if !c.required && !c.pk {
                    "Ok(None)".into()
                } else {
                    format!("Err(invalid({:?}))", format!("{column} input is required"))
                };
                out.push_str(&format!("match &self.{f}{{Change::Set(value)=>Ok(value.clone()),Change::Keep=>{missing}}}"));
            }
            out.push_str("}\n");
            out.push_str(&format!("pub(crate) fn {f}_input(&self)->Result<{ty}>{{"));
            if c.pk && c.auto {
                out.push_str(&format!("self.{f}_value()"));
            } else {
                out.push_str(&format!("match &self.{f}{{Change::Set(value)=>Ok(value.clone()),Change::Keep=>Err(invalid({:?}))}}",format!("{column} input is required")));
            }
            out.push_str("}\n");
        }
        out.push_str("}\n");
        out.push_str(&format!(
            "impl {name} {{pub(crate) fn check(&self,service:&Service)->Result<()>{{"
        ));
        if schema.constraints.contains_key(table) {
            // Replaced below by typed constraint lowering; no JSON records enter storage.
            out.push_str(&super::typed::constraints(schema, table)?);
        }
        out.push_str("Ok(())}}\n");
    }
    // Stable role aliases bind reusable algorithms to application-owned table names.
    for (cap, c) in &schema.capabilities {
        for (role, table) in &c.entities {
            let alias = format!(
                "{}{}",
                super::super::generate::pascal(cap),
                super::super::generate::pascal(role)
            );
            let name = record_name(table);
            if !schema.tables[table].values().any(|c| c.pk) {
                continue;
            }
            out.push_str(&format!(
                "pub(crate) type {alias}={name};pub(crate) type {alias}Write={name}Write;\n"
            ));
            for (column, col) in &schema.tables[table] {
                if col.kind == SqlType::Text && !col.constraints.values.is_empty() {
                    if format!("{alias}{}", super::super::generate::pascal(column))
                        == format!(
                            "{}{}",
                            super::super::generate::pascal(table),
                            super::super::generate::pascal(column)
                        )
                    {
                        continue;
                    }
                    out.push_str(&format!(
                        "pub(crate) type {alias}{}={}{};\n",
                        super::super::generate::pascal(column),
                        super::super::generate::pascal(table),
                        super::super::generate::pascal(column)
                    ));
                }
            }
        }
    }
    out.push_str("pub(crate) enum Event {");
    for event in [
        "payment",
        "order",
        "subscription",
        "bank_transaction",
        "payment_allocation",
        "subscription_due",
    ] {
        let (cap, role, variant) = super::event_role(event)?;
        if let Some(table) = openb2c_schema::capabilities::entity(schema, cap, role) {
            let ty = record_name(table);
            if event == "payment_allocation" {
                out.push_str(&format!("{variant}{{after:{ty}}},"));
            } else {
                out.push_str(&format!("{variant}{{before:{ty},after:{ty}}},"));
            }
        }
    }
    out.push_str("}\n");
    out.push_str("impl Service {pub(crate) fn insert_signup_account(&self,email:&str,name:&str)->Result<i64>{");
    if let Some(table) = openb2c_schema::capabilities::entity(schema, "accounts", "account") {
        out.push_str(&format!("Ok({}Write{{email:email.to_owned().into(),name:name.to_owned().into(),..Default::default()}}.insert(self)?.id)",record_name(table)));
    } else {
        out.push_str("Err(ServiceError::new(\"not_found\",\"Accounts are not enabled\"))");
    }
    out.push_str("} pub(crate) fn provisioned_staff(&self,user:i64,auth:&AuthContext)->Result<serde_json::Value>{");
    if let Some(table) = openb2c_schema::capabilities::entity(schema, "accounts", "staff") {
        out.push_str(&format!("let row={}::first(self,\"WHERE user_id=?\",&[&user])?.ok_or_else(||ServiceError::new(\"not_found\",\"Staff membership not found\"))?;row.audit(self,\"accounts.provision_staff\",auth,None)?;row.wire(self)",record_name(table)));
    } else {
        out.push_str("Err(ServiceError::new(\"not_found\",\"Staff membership is not enabled\"))");
    }
    out.push_str("}}\n");
    Ok(out)
}
