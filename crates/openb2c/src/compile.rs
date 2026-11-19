//! Compile evaluated business wiring into typed Rust and fixed SQL.
pub mod api;
pub mod operations;
pub mod records;
mod typed;
use anyhow::{Context, Result, bail, ensure};
use openb2c_schema::{
    Schema, SqlType,
    capabilities::entity,
    sql::quote,
    workflows::{Step, Term},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
enum Type {
    Int,
    Real,
    Text,
    Bool,
    Null,
    Optional(Box<Type>),
}
#[derive(Clone)]
struct Binding {
    table: Option<String>,
    fields: BTreeMap<String, Type>,
}
struct Compiler<'a> {
    schema: &'a Schema,
    bindings: BTreeMap<String, Binding>,
    serial: usize,
}
fn event_role(event: &str) -> Result<(&str, &str, &str)> {
    Ok(match event {
        "payment" => ("payments", "payment", "Payment"),
        "bank_transaction" => ("payments", "transaction", "BankTransaction"),
        "payment_allocation" => ("payments", "allocation", "PaymentAllocation"),
        "order" => ("orders", "order", "Order"),
        "subscription" => ("subscriptions", "subscription", "Subscription"),
        "subscription_due" => ("subscriptions", "subscription", "SubscriptionDue"),
        _ => bail!("Unknown workflow event {event}"),
    })
}
fn record_name(table: &str) -> String {
    format!("{}Record", super::generate::pascal(table))
}
fn binding(schema: &Schema, table: &str) -> Result<Binding> {
    let fields = schema
        .tables
        .get(table)
        .with_context(|| format!("Unknown workflow table {table}"))?
        .iter()
        .map(|(name, col)| {
            let ty = match col.kind {
                SqlType::Integer => Type::Int,
                SqlType::Real => Type::Real,
                SqlType::Text => Type::Text,
                SqlType::Blob => {
                    return Err(anyhow::anyhow!(
                        "Workflow records do not yet support blob field {table}.{name}"
                    ));
                }
            };
            Ok((
                name.clone(),
                if col.required || col.pk {
                    ty
                } else {
                    Type::Optional(Box::new(ty))
                },
            ))
        })
        .collect::<Result<_>>()?;
    Ok(Binding {
        table: Some(table.into()),
        fields,
    })
}
fn field(name: &str) -> String {
    super::generate::rust_field(name)
}
fn valid_binding(name: &str) -> bool {
    openb2c_schema::identifier(name)
        && ![
            "event", "service", "auth", "now", "old", "next", "row", "input",
        ]
        .contains(&name)
}
impl Compiler<'_> {
    fn term(&self, term: &Term) -> Result<(String, Type)> {
        Ok(match term {
            Term::Field { path } => {
                let (name, col) = path
                    .split_once('.')
                    .context("A workflow path must be binding.field")?;
                let binding = self
                    .bindings
                    .get(name)
                    .with_context(|| format!("Unknown workflow binding {name}"))?;
                let ty = binding
                    .fields
                    .get(col)
                    .with_context(|| format!("Unknown workflow field {path}"))?;
                let access = format!("{}.{}", field(name), field(col));
                let access = match ty {
                    Type::Text => format!("{access}.as_str()"),
                    Type::Optional(inner) if **inner == Type::Text => {
                        format!("{access}.as_deref()")
                    }
                    _ => access,
                };
                (access, ty.clone())
            }
            Term::Literal { value } => match value {
                Value::Null => ("None::<String>".into(), Type::Null),
                Value::Bool(v) => (v.to_string(), Type::Bool),
                Value::Number(v) if v.is_i64() => {
                    (format!("{}i64", v.as_i64().unwrap()), Type::Int)
                }
                Value::Number(v) => (
                    format!("{}f64", v.as_f64().context("Unsupported workflow number")?),
                    Type::Real,
                ),
                Value::String(v) => (format!("{v:?}"), Type::Text),
                _ => bail!("Workflow literals must be scalar values"),
            },
            Term::Concat { parts } => {
                let mut values = Vec::new();
                for p in parts {
                    let (expr, ty) = self.term(p)?;
                    ensure!(
                        !matches!(ty, Type::Optional(_) | Type::Null),
                        "Cannot format nullable workflow values"
                    );
                    values.push(expr);
                }
                (
                    if values.is_empty() {
                        "String::new()".into()
                    } else {
                        format!(
                            "format!({:?}, {})",
                            "{}".repeat(values.len()),
                            values.join(",")
                        )
                    },
                    Type::Text,
                )
            }
            Term::Next { next } => {
                let (expr, ty) = self.term(next)?;
                ensure!(ty == Type::Int, "Next requires an integer");
                (
                    format!(
                        "({expr}).checked_add(1).ok_or_else(|| invalid(\"Billing cycle overflow\"))?"
                    ),
                    Type::Int,
                )
            }
            Term::Date { date } => {
                let (expr, ty) = self.term(date)?;
                let expr = match ty {
                    Type::Text => expr,
                    Type::Optional(inner) if *inner == Type::Text => format!(
                        "({expr}).ok_or_else(|| invalid(\"A service period is required\"))?"
                    ),
                    _ => bail!("Date requires text"),
                };
                (
                    format!("crate::b2c::instant(&({expr}))?.date_naive().to_string()"),
                    Type::Text,
                )
            }
            Term::Binary { op, left, right } => {
                let (l, lt) = self.term(left)?;
                let (r, rt) = self.term(right)?;
                ensure!(
                    lt == rt,
                    "Workflow operator {op} compares incompatible types {lt:?} and {rt:?}"
                );
                let rust = match op.as_str() {
                    "eq" => "==",
                    "ne" => "!=",
                    "and" | "or" => {
                        ensure!(lt == Type::Bool, "Boolean operator requires booleans");
                        if op == "and" { "&&" } else { "||" }
                    }
                    _ => bail!("Unknown workflow operator {op}"),
                };
                (format!("(({l}) {rust} ({r}))"), Type::Bool)
            }
        })
    }
    fn typed(&self, term: &Term, expected: Type) -> Result<String> {
        let (expr, ty) = self.term(term)?;
        ensure!(
            ty == expected,
            "Workflow argument requires {expected:?}, got {ty:?}"
        );
        Ok(expr)
    }
    fn optional(&self, term: &Option<Term>) -> Result<String> {
        Ok(if let Some(term) = term {
            let (expr, ty) = self.term(term)?;
            match ty {
                Type::Text => format!("Some({expr})"),
                Type::Optional(inner) if *inner == Type::Text => expr,
                _ => bail!("Optional text argument requires text"),
            }
        } else {
            "None::<String>".into()
        })
    }
    fn record(&self, name: &str) -> Result<&str> {
        self.bindings
            .get(name)
            .and_then(|b| b.table.as_deref())
            .with_context(|| format!("{name} is not a table record"))
    }
    fn steps(&mut self, steps: &[Step]) -> Result<String> {
        let mut out = String::new();
        for step in steps {
            self.serial += 1;
            let serial = self.serial;
            match step {
                Step::ReverseTransfer { transaction } => {
                    ensure!(
                        self.schema.capabilities.contains_key("payments"),
                        "Reversal requires payments"
                    );
                    out.push_str(&format!(
                        "service.cap_reverse_transfer({},auth,now)?;\n",
                        self.typed(transaction, Type::Int)?
                    ));
                }
                Step::SyncTransfers => {
                    ensure!(
                        self.schema
                            .capabilities
                            .get("payments")
                            .is_some_and(|c| c.settings["feed"] == "starling"
                                || c.settings["feed"] == "open_banking"),
                        "Bank sync requires a read-only bank feed"
                    );
                    out.push_str("service.poll_bank_feed(now)?;\n");
                }
                Step::ReconcileTransfers => {
                    ensure!(
                        self.schema.capabilities.contains_key("payments"),
                        "Reconciliation requires payments"
                    );
                    out.push_str("service.reconcile_pending_transfers(auth)?;\n");
                }
                Step::AllocateTransfer {
                    transaction,
                    payment,
                    amount,
                } => {
                    ensure!(
                        self.schema.capabilities.contains_key("payments"),
                        "Allocation requires payments"
                    );
                    out.push_str(&format!(
                        "service.cap_allocate_transfer({}, {}, {}, auth, now)?;\n",
                        self.typed(transaction, Type::Int)?,
                        self.typed(payment, Type::Int)?,
                        self.typed(amount, Type::Int)?
                    ));
                }
                Step::RenewSubscriptions => {
                    ensure!(
                        self.schema.capabilities.contains_key("subscriptions"),
                        "Renewal requires subscriptions"
                    );
                    out.push_str("service.b2c_renew_due(auth)?;\n");
                }
                Step::Select {
                    table,
                    bind,
                    r#where,
                    steps,
                } => {
                    ensure!(
                        valid_binding(bind) && !self.bindings.contains_key(bind),
                        "Invalid or shadowed workflow binding {bind}"
                    );
                    let target = binding(self.schema, table)?;
                    ensure!(
                        !r#where.is_empty(),
                        "Workflow selects require an explicit predicate"
                    );
                    let mut clauses = vec![];
                    let mut values = vec![];
                    for (column, value) in r#where {
                        let ty = target
                            .fields
                            .get(column)
                            .with_context(|| format!("Unknown select field {table}.{column}"))?;
                        clauses.push(format!("{}=?", quote(column)));
                        values.push(self.typed(value, ty.clone())?);
                    }
                    let columns = target
                        .fields
                        .keys()
                        .map(|s| quote(s))
                        .collect::<Vec<_>>()
                        .join(",");
                    let sql = format!(
                        "SELECT {columns} FROM {} WHERE {} ORDER BY {}",
                        quote(table),
                        clauses.join(" AND "),
                        quote(
                            self.schema.tables[table]
                                .iter()
                                .find(|(_, c)| c.pk)
                                .context("Workflow table needs a primary key")?
                                .0
                        )
                    );
                    out.push_str(&format!("let rows_{serial} = {{ let mut statement = service.db.prepare({sql:?})?; statement.query_map(rusqlite::params![{}], {}::read)?.collect::<std::result::Result<Vec<_>,_>>()? }};\nfor {} in rows_{serial} {{\n",values.join(","),record_name(table),field(bind)));
                    self.bindings.insert(bind.clone(), target);
                    out.push_str(&self.steps(steps)?);
                    out.push_str("}\n");
                    self.bindings.remove(bind);
                }
                Step::If {
                    condition,
                    steps,
                    otherwise,
                } => {
                    out.push_str(&format!(
                        "if {} {{\n{}\n}} else {{\n{}\n}}\n",
                        self.typed(condition, Type::Bool)?,
                        self.steps(steps)?,
                        self.steps(otherwise)?
                    ));
                }
                Step::Require { condition, message } => {
                    out.push_str(&format!(
                        "if !({}) {{ return Err(state({message:?})); }}\n",
                        self.typed(condition, Type::Bool)?
                    ));
                }
                Step::Update {
                    record,
                    set,
                    action,
                    event,
                } => {
                    let table = self.record(record)?.to_string();
                    let (cap, role, variant) = event_role(event)?;
                    ensure!(
                        entity(self.schema, cap, role) == Some(table.as_str()),
                        "An {event} event must update its declared table"
                    );
                    ensure!(!set.is_empty(), "Workflow updates need assignments");
                    let mut values = vec![];
                    for (column, term) in set {
                        let col = self.schema.tables[&table]
                            .get(column)
                            .with_context(|| format!("Unknown update field {table}.{column}"))?;
                        ensure!(!col.pk, "Workflow cannot update a primary key");
                        if let Term::Literal {
                            value: Value::String(value),
                        } = term
                        {
                            ensure!(
                                col.constraints.values.is_empty()
                                    || col.constraints.values.contains(value),
                                "Invalid enum value {value} for {table}.{column}"
                            );
                        }
                        let expected = &self.bindings[record].fields[column];
                        let (expr, ty) = self.term(term)?;
                        let expr = if expected == &ty {
                            expr
                        } else if let Type::Optional(inner) = expected {
                            if ty == Type::Null {
                                let native = match **inner {
                                    Type::Int => "i64",
                                    Type::Real => "f64",
                                    Type::Text => "String",
                                    _ => bail!("Unsupported nullable assignment"),
                                };
                                format!("None::<{native}>")
                            } else if **inner == ty {
                                format!("Some({expr})")
                            } else {
                                bail!("Invalid update type for {table}.{column}")
                            }
                        } else {
                            bail!("Invalid update type for {table}.{column}")
                        };
                        let expr = if col.kind == SqlType::Text {
                            if col.constraints.values.is_empty() {
                                if col.required {
                                    format!("({expr}).to_string()")
                                } else {
                                    format!("({expr}).map(|v|v.to_string())")
                                }
                            } else {
                                let en = format!(
                                    "{}{}",
                                    super::generate::pascal(&table),
                                    super::generate::pascal(column)
                                );
                                if col.required {
                                    format!("{en}::try_from(AsRef::<str>::as_ref(&({expr})))?")
                                } else {
                                    format!(
                                        "({expr}).map(|v|{en}::try_from(AsRef::<str>::as_ref(&v))).transpose()?"
                                    )
                                }
                            }
                        } else {
                            expr
                        };
                        values.push(format!("{}:Change::Set({expr})", field(column)));
                    }
                    out.push_str(&format!("{{let old={}.clone();let next={}Write{{{},..Default::default()}}.update(service,old.id)?;next.audit(service,{action:?},auth,Some(&old))?;service.cap_publish(Event::{variant}{{before:old,after:next}},auth,now)?;}}\n",field(record),record_name(&table),values.join(",")));
                }
                Step::Grant {
                    user,
                    resource,
                    source,
                    end,
                } => {
                    ensure!(
                        self.schema.capabilities.contains_key("entitlements"),
                        "Grant requires entitlements"
                    );
                    out.push_str(&format!(
                        "service.cap_grant({}, &({}), &({}), ({}).as_deref(), auth, now)?;\n",
                        self.typed(user, Type::Int)?,
                        self.typed(resource, Type::Text)?,
                        self.typed(source, Type::Text)?,
                        self.optional(end)?
                    ));
                }
                Step::Revoke { source } => {
                    ensure!(
                        self.schema.capabilities.contains_key("entitlements"),
                        "Revoke requires entitlements"
                    );
                    out.push_str(&format!(
                        "service.cap_revoke_source(&({}),auth)?;\n",
                        self.typed(source, Type::Text)?
                    ));
                }
                Step::CancelPayment { id } => {
                    ensure!(
                        self.schema.capabilities.contains_key("payments"),
                        "Cancel payment requires payments"
                    );
                    out.push_str(&format!(
                        "service.cap_cancel_payment({},auth)?;\n",
                        self.typed(id, Type::Int)?
                    ));
                }
                Step::CancelSubscriptionPayments { id } => {
                    ensure!(
                        self.schema.capabilities.contains_key("subscriptions"),
                        "Cancel subscription payments requires subscriptions"
                    );
                    out.push_str(&format!(
                        "service.cap_cancel_subscription_payments({},auth)?;\n",
                        self.typed(id, Type::Int)?
                    ));
                }
                Step::Schedule {
                    record,
                    cycle,
                    bind,
                    steps,
                } => {
                    ensure!(
                        entity(self.schema, "subscriptions", "subscription")
                            == Some(self.record(record)?),
                        "Schedule requires a subscription record"
                    );
                    ensure!(
                        valid_binding(bind) && !self.bindings.contains_key(bind),
                        "Invalid schedule binding"
                    );
                    out.push_str(&format!("let {} = crate::b2c::billing_period({}.started_at.as_deref(), {}.current_period_end.as_deref(), {}.billing_interval.as_str(), {}, now)?;\n",field(bind),field(record),field(record),field(record),self.typed(cycle,Type::Int)?));
                    self.bindings.insert(
                        bind.clone(),
                        Binding {
                            table: None,
                            fields: ["anchor", "start", "end"]
                                .into_iter()
                                .map(|f| (f.into(), Type::Text))
                                .collect(),
                        },
                    );
                    out.push_str(&self.steps(steps)?);
                    self.bindings.remove(bind);
                }
                Step::Cash {
                    source,
                    amount,
                    currency,
                    debit,
                    credit,
                } => {
                    ensure!(
                        self.schema.capabilities.contains_key("billing"),
                        "Capture cash requires billing"
                    );
                    out.push_str(&format!(
                        "service.capture_cash(&({}),{},&({}),&({}),&({}),auth,now)?;\n",
                        self.typed(source, Type::Text)?,
                        self.typed(amount, Type::Int)?,
                        self.typed(currency, Type::Text)?,
                        self.typed(debit, Type::Text)?,
                        self.typed(credit, Type::Text)?
                    ));
                }
                Step::Sale {
                    record,
                    source,
                    description,
                    start,
                    end,
                    cash_account,
                } => {
                    ensure!(
                        self.schema.capabilities.contains_key("billing"),
                        "Capture sale requires billing"
                    );
                    self.record(record)?;
                    for (name, ty) in [
                        ("user_id", Type::Int),
                        ("amount_minor", Type::Int),
                        ("currency", Type::Text),
                    ] {
                        ensure!(
                            self.bindings[record].fields.get(name) == Some(&ty),
                            "Sale record requires {name}"
                        );
                    }
                    ensure!(
                        start.is_some() == end.is_some(),
                        "A sale period needs both boundaries"
                    );
                    let period = if start.is_some() {
                        format!(
                            "Some(({},{}))",
                            self.typed(start.as_ref().unwrap(), Type::Text)?,
                            self.typed(end.as_ref().unwrap(), Type::Text)?
                        )
                    } else {
                        "None".into()
                    };
                    let cash = cash_account
                        .as_ref()
                        .map(|t| {
                            self.typed(t, Type::Text)
                                .map(|v| format!("Some(({v}).to_string())"))
                        })
                        .transpose()?
                        .unwrap_or_else(|| "None".into());
                    out.push_str(&format!("service.capture_sale(&({}), crate::backoffice::SaleFacts {{user_id:{}.user_id,amount_minor:{}.amount_minor,currency:{}.currency.clone()}}, &({}), {},{},auth,now)?;\n",self.typed(source,Type::Text)?,field(record),field(record),field(record),self.typed(description,Type::Text)?,period,cash));
                }
                Step::Refund {
                    source,
                    cash_account,
                } => {
                    ensure!(
                        self.schema.capabilities.contains_key("billing"),
                        "Capture refund requires billing"
                    );
                    let cash = cash_account
                        .as_ref()
                        .map(|t| {
                            self.typed(t, Type::Text)
                                .map(|v| format!("Some(({v}).to_string())"))
                        })
                        .transpose()?
                        .unwrap_or_else(|| "None".into());
                    out.push_str(&format!(
                        "service.capture_refund(&({}),{},auth,now)?;\n",
                        self.typed(source, Type::Text)?,
                        cash
                    ));
                }
            }
        }
        Ok(out)
    }
}

pub fn composition(schema: &Schema) -> Result<String> {
    let mut source = String::from(
        "// Generated business composition. Do not edit.\n#![allow(unused_imports,unused_variables,clippy::clone_on_copy,clippy::nonminimal_bool,clippy::let_and_return,clippy::double_parens)]\nuse crate::{Service,AuthContext,b2c::{Event,invalid,state},service::Result,records::*};\nuse chrono::{DateTime,Utc};\n",
    );
    let model = serde_json::to_string(schema)?;
    let fingerprint = format!("{:x}", Sha256::digest(model.as_bytes()));
    for (name, sql) in [
        ("SCHEMA_SQL", openb2c_schema::sql::generate(schema)?),
        ("SEED_SQL", openb2c_schema::sql::seed_sql(schema, false)?),
        ("FIXTURES_SQL", openb2c_schema::sql::seed_sql(schema, true)?),
    ] {
        source.push_str(&format!("pub(crate) const {name}:&str={sql:?};\n"));
    }
    let worker = schema.workflows.values().any(|w| w.event == "tick")
        || schema
            .operations
            .values()
            .flat_map(|o| o.values())
            .any(|op| op.enabled && !op.effects.is_empty());
    source.push_str(&format!("pub(crate) const WORKER_ENABLED:bool={worker};\n"));
    source.push_str(&format!("pub(crate) const MODEL_HASH:&str={fingerprint:?};\npub(crate) fn check_model(schema:&openb2c_schema::Schema)->anyhow::Result<()>{{anyhow::ensure!(serde_json::to_string(schema)?=={model:?},\"The model differs from this compiled application; regenerate and rebuild the service\");Ok(())}}\n"));
    source.push_str("impl Service {pub fn assert_current_model(&self)->Result<()>{let current:String=self.db.query_row(\"SELECT sha256 FROM openb2c_application_revision WHERE id=1\",[],|r|r.get(0))?;if current!=MODEL_HASH{return Err(state(\"Application model changed; restart this process before serving requests\"));}Ok(())}}\n");
    source.push_str("pub(crate) fn has_table(table:&str)->bool{match table{\n");
    for table in schema.tables.keys() {
        source.push_str(&format!("{table:?}=>true,\n"));
    }
    source.push_str(
        "_=>false}}\npub(crate) fn api_table(path:&str)->Option<&'static str>{match path{\n",
    );
    for table in schema.tables.keys() {
        source.push_str(&format!("{:?}=>Some({table:?}),\n", format!("{table}s")));
    }
    source.push_str("_=>None}}\n");
    source.push_str("pub(crate) fn entity(capability:&str,role:&str)->Option<&'static str>{match (capability,role){\n");
    for (cap, c) in &schema.capabilities {
        for (role, table) in &c.entities {
            source.push_str(&format!("({cap:?},{role:?})=>Some({table:?}),\n"));
        }
    }
    source.push_str("_=>None}}\npub(crate) fn enabled(capability:&str)->bool{matches!(capability,");
    source.push_str(
        &schema
            .capabilities
            .keys()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join("|"),
    );
    if schema.capabilities.is_empty() {
        source.push_str("\"\"");
    }
    source.push_str(")}\n");
    source.push_str("pub(crate) fn setting(capability:&str,key:&str)->Option<&'static str>{match(capability,key){\n");
    for (cap, c) in &schema.capabilities {
        for (key, value) in c.settings.as_object().into_iter().flatten() {
            if let Some(value) = value.as_str() {
                source.push_str(&format!("({cap:?},{key:?})=>Some({value:?}),\n"));
            }
        }
    }
    source.push_str("_=>None}}\n");
    let mut workflows = schema.workflows.iter().collect::<Vec<_>>();
    workflows.sort_by_key(|(name, w)| (w.order, *name));
    for (name, workflow) in &workflows {
        ensure!(
            openb2c_schema::identifier(name),
            "Invalid workflow name {name}"
        );
        if workflow.event == "tick" {
            let mut compiler = Compiler {
                schema,
                bindings: BTreeMap::new(),
                serial: 0,
            };
            source.push_str(&format!("fn workflow_{name}(service:&Service,auth:&AuthContext,now:DateTime<Utc>)->Result<()>{{{} Ok(())}}\n",compiler.steps(&workflow.steps)?));
            continue;
        }
        let (cap, role, _) = event_role(&workflow.event)?;
        let table = entity(schema, cap, role).context("Workflow event provider is absent")?;
        let mut compiler = Compiler {
            schema,
            bindings: BTreeMap::from([("event".into(), binding(schema, table)?)]),
            serial: 0,
        };
        source.push_str(&format!("fn workflow_{name}(service:&Service,event:&{},auth:&AuthContext,now:DateTime<Utc>)->Result<()>{{\n{}\nOk(())\n}}\n",record_name(table),compiler.steps(&workflow.steps).with_context(||format!("Compiling workflow {name}"))?));
    }
    source.push_str("pub(crate) fn publish(service:&Service,event:Event,auth:&AuthContext,now:DateTime<Utc>)->Result<()> { match event {\n");
    for event in [
        "payment",
        "order",
        "subscription",
        "bank_transaction",
        "payment_allocation",
        "subscription_due",
    ] {
        let (cap, role, variant) = event_role(event)?;
        if entity(schema, cap, role).is_none() {
            continue;
        }
        if event == "payment_allocation" {
            source.push_str(&format!("Event::{variant}{{after}}=>{{\n"));
        } else {
            source.push_str(&format!("Event::{variant}{{before,after}}=>{{\n"));
        }
        if event != "subscription_due" && event != "payment_allocation" {
            source.push_str("if before==after {return Ok(());}\n");
        }
        let selected = workflows
            .iter()
            .filter(|(_, w)| w.event == event)
            .collect::<Vec<_>>();
        if !selected.is_empty() {
            source.push_str("let event = after;\n");
            for (name, _) in selected {
                source.push_str(&format!("workflow_{name}(service,&event,auth,now)?;\n"));
            }
        }
        source.push_str("}\n");
    }
    source.push_str("} Ok(()) }\npub(crate) fn tick(service:&Service)->Result<()>{let auth=AuthContext::system();let now=Utc::now();\n");
    for (name, w) in &workflows {
        if w.event == "tick" {
            source.push_str(&format!("workflow_{name}(service,&auth,now)?;\n"));
        }
    }
    source.push_str("Ok(())}\n");
    Ok(source)
}

/// Protocol contracts are resolved once by the compiler, never scanned per request.
pub fn rest(schema: &Schema) -> Result<String> {
    let mut out = String::from(
        "// Generated REST wiring.\npub(crate) fn rest(service:&crate::Service,method:&axum::http::Method,path:&str,headers:&axum::http::HeaderMap,bytes:&[u8],auth:&crate::AuthContext)->crate::service::Result<axum::response::Response>{\nuse axum::response::IntoResponse;\nuse crate::service::ServiceError;\nlet parts=path.trim_matches('/').split('/').collect::<Vec<_>>();\nlet id=||parts.get(2).and_then(|id|id.parse::<i64>().ok()).filter(|id|*id>0).ok_or_else(||crate::b2c::invalid(\"A positive record ID is required\"));\nmatch (method.as_str(),parts.as_slice()){\n",
    );
    for e in openb2c_schema::capabilities::endpoints(schema)
        .into_iter()
        .filter(|e| e.path.starts_with("/b2c/") || e.path.starts_with("/backoffice/"))
    {
        let parts = e
            .path
            .trim_matches('/')
            .split('/')
            .map(|s| {
                if s == "{id}" {
                    "_".into()
                } else {
                    format!("{s:?}")
                }
            })
            .collect::<Vec<String>>()
            .join(",");
        out.push_str(&format!("({:?},[{parts}])=>{{\n", e.method.to_uppercase()));
        if let Some(scope) = e.scope {
            out.push_str(&format!(
                "service.cap_command({:?},{scope:?},auth)?;\n",
                e.capability
            ));
        }
        if e.method == "post" {
            out.push_str("let input=if bytes.is_empty(){serde_json::json!({})}else{crate::web::json_body(headers,bytes)?};\n");
        }
        let input = "&input";
        let call = match (e.method, e.path) {
            ("get", "/b2c/catalog") => "service.b2c_catalog()".into(),
            ("get", "/b2c/downloads") => "service.b2c_downloads(auth)".into(),
            ("get", "/b2c/downloads/manage") => "service.b2c_managed_downloads(auth)".into(),
            ("post", "/b2c/downloads") => format!("service.b2c_publish_download({input},auth)"),
            ("post", "/b2c/downloads/{id}/withdraw") => {
                out.push_str("crate::b2c::decode::<crate::b2c::Empty>(&input)?;\n");
                "service.b2c_withdraw_download(id()?,auth)".into()
            }
            ("get", "/b2c/downloads/{id}/content") => {
                out.push_str("let file=service.b2c_download(id()?,auth)?;let mut response=file.bytes.into_response();\nfor(name,value)in[(\"content-type\",file.media_type),(\"content-disposition\",format!(\"attachment; filename=\\\"{}\\\"\",file.name)),(\"etag\",format!(\"\\\"{}\\\"\",file.sha256)),(\"cache-control\",\"private, no-store\".into())]{response.headers_mut().insert(axum::http::header::HeaderName::from_static(name),axum::http::HeaderValue::from_str(&value).map_err(|_|ServiceError::new(\"internal_error\",\"Invalid stored file metadata\"))?);}return Ok(response);\n},\n");
                continue;
            }
            ("post", "/b2c/orders") => format!("service.b2c_create_order({input},auth)"),
            ("get", "/b2c/orders/{id}") => "service.b2c_order(id()?,auth)".into(),
            ("post", "/b2c/orders/{id}/cancel") => {
                out.push_str("crate::b2c::decode::<crate::b2c::Empty>(&input)?;\n");
                "service.b2c_cancel_order(id()?,auth)".into()
            }
            ("post", "/b2c/orders/{id}/payments") => {
                format!("service.b2c_order_payment(id()?,{input},auth)")
            }
            ("post", "/b2c/payments") => format!("service.b2c_create_payment({input},auth)"),
            ("get", "/b2c/payments/pending") => "service.b2c_pending_payments(auth)".into(),
            ("get", "/b2c/payments/{id}") => "service.b2c_payment(id()?,auth)".into(),
            ("post", "/backoffice/transfers") => format!("service.import_simulated_transfer({input},auth)"),
            ("get", "/backoffice/transfers") => "service.bank_transfers(auth)".into(),
            ("post", "/backoffice/transfers/{id}/allocate") => format!("service.allocate_transfer(id()?,{input},auth)"),
            ("post", "/backoffice/transfers/{id}/review") => format!("service.review_transfer(id()?,{input},auth)"),
            ("post", "/backoffice/transfers/{id}/reconcile") => {
                out.push_str("crate::b2c::decode::<crate::b2c::Empty>(&input)?;\n");
                "service.reconcile_transfer(id()?,auth)".into()
            }
            ("post", "/backoffice/transfers/sync") => format!("service.sync_bank_feed({input},auth)"),
            ("post", "/backoffice/transfers/feed-item") => "service.starling_webhook(bytes,headers.get(\"x-hook-signature\").and_then(|v|v.to_str().ok()).unwrap_or(\"\"))".into(),
            ("post", "/b2c/subscriptions") => {
                format!("service.b2c_create_subscription({input},auth)")
            }
            ("get", "/b2c/subscriptions/{id}") => "service.b2c_subscription(id()?,auth)".into(),
            ("post", "/b2c/subscriptions/{id}/cancel") => {
                format!("service.b2c_cancel_subscription(id()?,{input},auth)")
            }
            ("post", "/b2c/subscriptions/renew-due") => {
                out.push_str("crate::b2c::decode::<crate::b2c::Empty>(&input)?;\n");
                "service.b2c_renew_due(auth)".into()
            }
            ("get", "/b2c/entitlements") => "service.b2c_entitlements(auth)".into(),
            ("post", "/b2c/entitlements") => format!("service.b2c_grant_entitlement({input},auth)"),
            ("post", "/b2c/entitlements/{id}/revoke") => {
                out.push_str("crate::b2c::decode::<crate::b2c::Empty>(&input)?;\n");
                "service.b2c_revoke_entitlement(id()?,auth)".into()
            }
            ("get", "/backoffice/accounts") => {
                "service.backoffice_list(\"accounting\",\"account\",auth)".into()
            }
            ("post", "/backoffice/accounts") => format!("service.backoffice_account({input},auth)"),
            ("post", "/backoffice/journals") => format!("service.backoffice_journal({input},auth)"),
            ("get", "/backoffice/journals/{id}") => {
                "service.backoffice_journal_view(id()?,auth)".into()
            }
            ("post", "/backoffice/journals/{id}/reverse") => {
                format!("service.backoffice_reverse(id()?,{input},auth)")
            }
            ("post", "/backoffice/periods/close") => {
                format!("service.backoffice_close({input},auth)")
            }
            ("post", "/backoffice/reports/financial") => {
                format!("service.backoffice_financial_report({input},auth)")
            }
            ("get", "/backoffice/invoices") => {
                "service.backoffice_list(\"billing\",\"invoice\",auth)".into()
            }
            ("get", "/backoffice/invoices/{id}") => {
                "service.backoffice_invoice_view(id()?,auth)".into()
            }
            ("post", "/backoffice/invoices") => format!("service.backoffice_invoice({input},auth)"),
            ("post", "/backoffice/invoices/{id}/receive") => {
                format!("service.backoffice_receive(id()?,{input},auth)")
            }
            ("post", "/backoffice/invoices/{id}/credit") => {
                format!("service.backoffice_credit(id()?,{input},auth)")
            }
            ("post", "/backoffice/invoices/{id}/refund") => {
                format!("service.backoffice_refund(id()?,{input},auth)")
            }
            ("get", "/backoffice/billing-events") => {
                "service.backoffice_list(\"billing\",\"event\",auth)".into()
            }
            ("post", "/backoffice/billing-events/post") => {
                format!("service.backoffice_post_billing_events({input},auth)")
            }
            ("post", "/backoffice/revenue/recognize") => {
                format!("service.backoffice_recognize({input},auth)")
            }
            ("post", "/backoffice/documents") => {
                format!("service.backoffice_document({input},auth)")
            }
            ("get", "/backoffice/documents/{id}") => {
                "service.backoffice_record(\"documents\",\"document\",id()?,auth)".into()
            }
            ("get", "/backoffice/suppliers") => {
                "service.backoffice_list(\"purchasing\",\"supplier\",auth)".into()
            }
            ("post", "/backoffice/suppliers") => {
                format!("service.backoffice_supplier({input},auth)")
            }
            ("get", "/backoffice/expenses") => {
                "service.backoffice_list(\"purchasing\",\"expense\",auth)".into()
            }
            ("get", "/backoffice/expenses/{id}") => {
                "service.backoffice_record(\"purchasing\",\"expense\",id()?,auth)".into()
            }
            ("post", "/backoffice/expenses") => format!("service.backoffice_expense({input},auth)"),
            ("post", "/backoffice/expenses/{id}/pay") => {
                format!("service.backoffice_pay_expense(id()?,{input},auth)")
            }
            ("post", "/backoffice/expenses/{id}/void") => {
                format!("service.backoffice_void_expense(id()?,{input},auth)")
            }
            ("get", "/backoffice/bank-lines") => {
                "service.backoffice_list(\"banking\",\"statement\",auth)".into()
            }
            ("post", "/backoffice/bank-lines") => {
                format!("service.backoffice_bank_line({input},auth)")
            }
            ("post", "/backoffice/bank-lines/{id}/match") => {
                format!("service.backoffice_match(id()?,{input},auth)")
            }
            ("post", "/backoffice/reports/vat") => {
                format!("service.backoffice_vat_report({input},auth)")
            }
            _ => bail!(
                "No compiled REST implementation for {} {}",
                e.method,
                e.path
            ),
        };
        out.push_str(&format!("Ok(axum::Json({call}?).into_response())\n}},\n"));
    }
    out.push_str(
        "_=>Err(ServiceError::new(\"not_found\",\"Unknown application endpoint\")),\n}}\n",
    );
    Ok(out)
}
