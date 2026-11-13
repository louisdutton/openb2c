use crate::*;
use anyhow::{Context, Result, bail};
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Role {
    pub entity: String,
    pub fields: BTreeMap<String, String>,
}
impl Role {
    pub fn field<'a>(&'a self, role: &'a str) -> &'a str {
        self.fields.get(role).map(String::as_str).unwrap_or(role)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commerce {
    pub catalog: Role,
    pub order: Role,
    pub line_item: Role,
    pub order_line: Role,
    pub transaction: Role,
    pub transaction_line: Role,
    pub capacity: Option<(FieldRef, Option<FieldRef>)>,
    pub options: Vec<String>,
    pub available: String,
    pub fulfilled: String,
    pub currency: String,
    pub expiry_minutes: i64,
    pub payment_deadline_minutes: i64,
    pub allow_async_payments: bool,
    pub max_quantity: i64,
    pub max_lines: usize,
}
pub fn foreign_keys<'a>(schema: &'a Schema, table: &str) -> Vec<(&'a str, &'a str, &'a str)> {
    schema
        .tables
        .get(table)
        .into_iter()
        .flatten()
        .filter_map(|(f, c)| reference(c.references.as_deref()?).map(|(t, k)| (f.as_str(), t, k)))
        .collect()
}
pub fn fk(schema: &Schema, table: &str, target: &str) -> Result<String> {
    let keys: Vec<_> = foreign_keys(schema, table)
        .into_iter()
        .filter(|(_, t, _)| *t == target)
        .collect();
    if keys.len() != 1 {
        bail!(
            "{table}: expected exactly one foreign key to {target}; found {}",
            keys.len()
        );
    }
    Ok(keys[0].0.into())
}
fn named(schema: &Schema, table: &str, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find(|f| schema.tables[table].contains_key(**f))
        .map(|s| (*s).into())
}
fn enums(schema: &Schema, table: &str, values: &[&str]) -> Option<String> {
    schema.tables[table]
        .iter()
        .find(|(_, c)| {
            values
                .iter()
                .all(|v| c.constraints.values.iter().any(|s| s == v))
        })
        .map(|(f, _)| f.clone())
}
fn money(schema: &Schema, table: &str) -> Option<String> {
    named(schema, table, &["price_pence", "amount_pence"]).or_else(|| {
        schema.tables[table]
            .iter()
            .find(|(f, c)| f.ends_with("_pence") || c.metadata["format"] == "money")
            .map(|(f, _)| f.clone())
    })
}
fn choose(label: &str, candidates: Vec<String>) -> Result<String> {
    if candidates.len() != 1 {
        bail!(
            "commerce.inference.{label}: expected one matching entity; found {} ({})",
            candidates.len(),
            candidates.join(", ")
        );
    }
    Ok(candidates[0].clone())
}
fn role(entity: &str, entries: &[(&str, String)]) -> Role {
    Role {
        entity: entity.into(),
        fields: entries
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    }
}
pub fn commerce(schema: &Schema) -> Result<Option<Commerce>> {
    let config = schema
        .metadata
        .get("commerce")
        .cloned()
        .unwrap_or(Value::Null);
    if config["enabled"] != true {
        return Ok(None);
    }
    let order = choose(
        "order",
        schema
            .tables
            .keys()
            .filter(|t| {
                fk(schema, t, "user").is_ok()
                    && enums(
                        schema,
                        t,
                        &[
                            "checkout_pending",
                            "payment_pending",
                            "paid",
                            "expired",
                            "cancelled",
                            "refund_required",
                            "refunded",
                        ],
                    )
                    .is_some()
                    && [
                        "amount_pence",
                        "currency",
                        "expires_at",
                        "payment_deadline_at",
                        "payment_reference",
                    ]
                    .iter()
                    .all(|f| schema.tables[*t].contains_key(*f))
            })
            .cloned()
            .collect(),
    )?;
    let transaction = choose(
        "transaction",
        schema
            .tables
            .keys()
            .filter(|t| {
                fk(schema, t, "user").is_ok()
                    && enums(
                        schema,
                        t,
                        &[
                            "pending",
                            "authorized",
                            "completed",
                            "failed",
                            "cancelled",
                            "refund_required",
                            "refunded",
                        ],
                    )
                    .is_some()
                    && money(schema, t).is_some()
                    && named(schema, t, &["reference", "payment_reference"]).is_some()
            })
            .cloned()
            .collect(),
    )?;
    let line = choose(
        "lineItem",
        schema
            .tables
            .keys()
            .filter(|t| {
                enums(schema, t, &["reserved", "cancelled"]).is_some()
                    && named(schema, t, &["price_pence"]).is_some()
                    && foreign_keys(schema, t)
                        .iter()
                        .any(|(_, target, _)| *target != "user")
            })
            .cloned()
            .collect(),
    )?;
    let catalog_fks: Vec<_> = foreign_keys(schema, &line)
        .into_iter()
        .filter(|(_, t, _)| {
            !["user", order.as_str(), transaction.as_str()].contains(t)
                && money(schema, t).is_some()
        })
        .collect();
    if catalog_fks.len() != 1 {
        bail!("commerce.inference.catalog: expected one catalog foreign key");
    }
    let catalog = catalog_fks[0].1.to_string();
    let order_line = choose(
        "orderLine",
        schema
            .tables
            .keys()
            .filter(|t| fk(schema, t, &order).is_ok() && fk(schema, t, &line).is_ok())
            .cloned()
            .collect(),
    )?;
    let transaction_line = choose(
        "transactionLine",
        schema
            .tables
            .keys()
            .filter(|t| fk(schema, t, &transaction).is_ok() && fk(schema, t, &line).is_ok())
            .cloned()
            .collect(),
    )?;
    let mut catalog_role = role(
        &catalog,
        &[(
            "price",
            money(schema, &catalog).context("Catalog price missing")?,
        )],
    );
    for (key, names) in [
        ("label", vec!["name", "title"]),
        ("description", vec!["description", "summary"]),
        ("status", vec!["status"]),
    ] {
        if let Some(field) = named(schema, &catalog, &names) {
            catalog_role.fields.insert(key.into(), field);
        }
    }
    let catalog_keys = foreign_keys(schema, &catalog);
    if !catalog_role.fields.contains_key("label")
        && let Some((field, _, _)) = catalog_keys
            .iter()
            .find(|(_, t, _)| *t != "user" && *t != "venue")
            .or_else(|| catalog_keys.first())
            .cloned()
    {
        catalog_role.fields.insert("label".into(), field.into());
    }
    let capacity_candidates: Vec<_> = schema.tables[&catalog]
        .keys()
        .filter(|f| f.as_str() == "capacity" || f.ends_with("_capacity"))
        .map(|f| {
            (
                FieldRef {
                    table: catalog.clone(),
                    field: f.clone(),
                    references: None,
                },
                None,
            )
        })
        .collect();
    let capacity = if capacity_candidates.len() == 1 {
        capacity_candidates.first().cloned()
    } else {
        let catalog_name = &catalog;
        let candidates: Vec<_> = foreign_keys(schema, &catalog)
            .into_iter()
            .flat_map(|(via, t, _)| {
                schema.tables[t]
                    .keys()
                    .filter(|f| f.as_str() == "capacity" || f.ends_with("_capacity"))
                    .map(move |f| {
                        (
                            FieldRef {
                                table: t.into(),
                                field: f.clone(),
                                references: None,
                            },
                            Some(FieldRef {
                                table: catalog_name.clone(),
                                field: via.into(),
                                references: Some(format!("{t}(id)")),
                            }),
                        )
                    })
            })
            .collect();
        if candidates.len() == 1 {
            candidates.first().cloned()
        } else {
            None
        }
    };
    let mut order_role = role(
        &order,
        &[
            ("user", fk(schema, &order, "user")?),
            (
                "status",
                enums(schema, &order, &["checkout_pending", "paid"])
                    .context("Order status missing")?,
            ),
        ],
    );
    for (key, field) in [
        ("amount", "amount_pence"),
        ("currency", "currency"),
        ("expiresAt", "expires_at"),
        ("paymentDeadlineAt", "payment_deadline_at"),
        ("paymentReference", "payment_reference"),
    ] {
        order_role.fields.insert(key.into(), field.into());
    }
    if schema.tables[&order].contains_key("client") {
        order_role.fields.insert("client".into(), "client".into());
    }
    let status = enums(schema, &line, &["reserved", "cancelled"]).context("Line status missing")?;
    let fulfilled = if schema.tables[&line][&status]
        .constraints
        .values
        .iter()
        .any(|v| v == "confirmed")
    {
        "confirmed"
    } else {
        "fulfilled"
    }
    .into();
    let mut line_role = role(
        &line,
        &[
            ("catalogItem", catalog_fks[0].0.into()),
            ("price", money(schema, &line).context("Line price missing")?),
            ("status", status),
        ],
    );
    if let Ok(field) = fk(schema, &line, "user") {
        line_role.fields.insert("user".into(), field);
    }
    if schema.tables[&line].contains_key("quantity") {
        line_role
            .fields
            .insert("quantity".into(), "quantity".into());
    }
    let options = schema.tables[&line]
        .iter()
        .filter(|(f, c)| {
            !c.pk
                && !line_role.fields.values().any(|v| v == *f)
                && c.references.is_none()
                && (!c.constraints.values.is_empty()
                    || c.constraints
                        .range
                        .as_ref()
                        .is_some_and(|b| b.min.is_some() || b.max.is_some()))
        })
        .map(|(f, _)| f.clone())
        .collect();
    let mut tx_role = role(
        &transaction,
        &[
            ("user", fk(schema, &transaction, "user")?),
            (
                "amount",
                money(schema, &transaction).context("Transaction amount missing")?,
            ),
            (
                "status",
                enums(schema, &transaction, &["pending", "completed"])
                    .context("Transaction status missing")?,
            ),
            (
                "reference",
                named(schema, &transaction, &["reference", "payment_reference"])
                    .context("Transaction reference missing")?,
            ),
        ],
    );
    for field in ["type", "client"] {
        if schema.tables[&transaction].contains_key(field) {
            tx_role.fields.insert(field.into(), field.into());
        }
    }
    let available = catalog_role
        .fields
        .get("status")
        .map(|f| &schema.tables[&catalog][f].constraints.values)
        .and_then(|v| {
            ["scheduled", "available", "open", "active"]
                .into_iter()
                .find(|s| v.iter().any(|v| v == s))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "available".into());
    let cfg = &config["checkout"];
    let max_quantity = cfg["maxQuantity"].as_i64().unwrap_or(20);
    let max_lines = cfg["maxLines"].as_u64().unwrap_or(50) as usize;
    if max_quantity < 1 || max_lines < 1 {
        bail!("Commerce limits must be positive");
    }
    Ok(Some(Commerce {
        catalog: catalog_role,
        order: order_role,
        line_item: line_role,
        order_line: role(
            &order_line,
            &[
                ("order", fk(schema, &order_line, &order)?),
                ("lineItem", fk(schema, &order_line, &line)?),
            ],
        ),
        transaction: tx_role,
        transaction_line: role(
            &transaction_line,
            &[
                ("transaction", fk(schema, &transaction_line, &transaction)?),
                ("lineItem", fk(schema, &transaction_line, &line)?),
            ],
        ),
        capacity,
        options,
        available,
        fulfilled,
        currency: cfg["currency"].as_str().unwrap_or("GBP").into(),
        expiry_minutes: cfg["expiryMinutes"].as_i64().unwrap_or(15),
        payment_deadline_minutes: cfg["paymentDeadlineMinutes"].as_i64().unwrap_or(30),
        allow_async_payments: cfg["allowAsyncPayments"].as_bool().unwrap_or(false),
        max_quantity,
        max_lines,
    }))
}

pub fn report(schema: &Schema) -> Value {
    let relationships:Vec<_>=schema.tables.keys().flat_map(|table|foreign_keys(schema,table).into_iter().map(move |(field,target,key)|json!({"entity":table,"field":field,"target":target,"key":key,"reason":"Declared foreign key","evidence":[format!("{table}.{field} references {target}({key})")],"metadata":schema.tables[table][field].relationship}))).collect();
    let profiles:Vec<_>=schema.tables.keys().filter_map(|table|{
        let fks=foreign_keys(schema,table);if fks.len()!=1{return None;}let (field,parent,_)=fks[0];let c=&schema.tables[table][field];let unique=c.unique||schema.constraints.get(table).is_some_and(|g|g.unique.values().any(|c|c.fields.len()==1&&c.fields[0].field==field&&c.condition.is_none()&&c.enforcement!=Enforcement::Application));
        (c.required&&unique).then(||json!({"entity":table,"parentEntity":parent,"field":field,"reason":"One required unique foreign key defines a profile extension","evidence":[format!("{table}.{field}: required, unique, references {parent}")]}))
    }).collect();
    let entities:serde_json::Map<String,Value>=schema.tables.iter().map(|(table,cols)|{
        let label=["name","title","email","reference"].into_iter().find(|f|cols.contains_key(*f));
        let temporal:Vec<_>=cols.iter().filter(|(f,c)|!["created_at","updated_at"].contains(&f.as_str())&&(f.ends_with("_at")||["date","time","date-time"].iter().any(|format|c.metadata["format"]==*format))).map(|(f,_)|f).collect();
        let money:Vec<_>=cols.iter().filter(|(f,c)|f.ends_with("_pence")||c.metadata["format"]=="money").map(|(f,_)|f).collect();
        let status:Vec<_>=cols.iter().filter(|(_,c)|!c.constraints.values.is_empty()).map(|(f,_)|f).collect();
        (table.clone(),json!({"label":{"value":label,"reason":"Prefer a stored human-readable label; use temporal fields and foreign keys otherwise","evidence":label.into_iter().map(|f|format!("{table}.{f}")).collect::<Vec<_>>()},"foreignKeys":relationships.iter().filter(|r|r["entity"]==*table).collect::<Vec<_>>(),"inboundReferences":relationships.iter().filter(|r|r["target"]==*table).collect::<Vec<_>>(),"temporalFields":temporal,"moneyFields":money,"statusFields":status,"uniqueFields":cols.iter().filter(|(_,c)|c.pk||c.unique).map(|(f,_)|f).collect::<Vec<_>>()}))
    }).collect();
    let surfaces:serde_json::Map<String,Value>=schema.tables.keys().map(|table|{
        let mut panels=vec![json!({"type":"overview","entity":table,"reason":"Entity fields and operations provide the record overview","evidence":[format!("tables.{table}")]})];
        for edge in relationships.iter().filter(|r|r["target"]==*table){let child=edge["entity"].as_str().unwrap_or("");let kind=if profiles.iter().any(|p|p["entity"]==child){"profile"}else if schema.tables[child].keys().any(|f|["storage_uri","document_uri","version_label"].contains(&f.as_str())){"materials"}else if entities[child]["temporalFields"].as_array().is_some_and(|f|!f.is_empty()){"schedule"}else if foreign_keys(schema,child).len()>1{"matrix"}else{"list"};panels.push(json!({"type":kind,"entity":child,"field":edge["field"],"reason":"Inbound foreign key connects these records to the workspace","evidence":edge["evidence"]}));}
        (table.clone(),json!({"recordPanels":panels}))
    }).collect();
    let (config, diagnostics) = match commerce(schema) {
        Ok(config) => (config, Vec::<String>::new()),
        Err(error) => (None, vec![error.to_string()]),
    };
    let mut commerce = config.as_ref().map(|c| json!(c)).unwrap_or(json!({}));
    commerce["enabled"] = json!(config.is_some());
    commerce["diagnostics"] = json!(diagnostics);
    if let Some(c) = config.as_ref() {
        commerce["roles"] = json!({"catalog":{"value":c.catalog.entity,"reason":"Line-item foreign key targets an entity with a price"},"order":{"value":c.order.entity,"reason":"Customer FK, order status enum, amount and payment deadlines"},"lineItem":{"value":c.line_item.entity,"reason":"Reserved status, price and catalog FK"},"transaction":{"value":c.transaction.entity,"reason":"Customer FK, payment states, amount and provider reference"},"orderLine":{"value":c.order_line.entity,"reason":"Foreign-key bridge between orders and line items"},"transactionLine":{"value":c.transaction_line.entity,"reason":"Foreign-key bridge between transactions and line items"}});
    }
    json!({"version":1,"summary":{"tables":schema.tables.len(),"relationships":relationships.len(),"profileExtensions":profiles.len(),"commerceEnabled":config.is_some(),"diagnostics":diagnostics.len()},"diagnostics":diagnostics,"organization":schema.organization,"entities":entities,"relationships":relationships,"profileExtensions":profiles,"profiles":profiles,"surfaces":surfaces,"operations":schema.operations,"commerce":commerce,"auth":schema.metadata.get("auth"),"presentation":schema.metadata.get("presentation")})
}
