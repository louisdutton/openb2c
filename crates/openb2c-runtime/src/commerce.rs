use crate::{
    AuthContext, Service,
    identity::{after, fake_allowed, secret},
    service::{Result, ServiceError, sql_value},
};
use chrono::Utc;
use openb2c_schema::{
    inference::{self, Commerce},
    sql::quote,
};
use rusqlite::{params, params_from_iter, types::Value as SqlValue};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

impl Service {
    pub fn commerce(&self) -> Result<Commerce> {
        inference::commerce(&self.schema)?
            .ok_or_else(|| ServiceError::new("not_found", "Commerce is not enabled"))
    }
    pub(crate) fn insert_record(&self, table: &str, data: &Map<String, Value>) -> Result<Value> {
        self.validate_input(table, data, true)?;
        let fields = data.keys().map(|f| quote(f)).collect::<Vec<_>>();
        let values = data.values().map(sql_value).collect::<Result<Vec<_>>>()?;
        let sql = if fields.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES RETURNING *", quote(table))
        } else {
            format!(
                "INSERT INTO {} ({}) VALUES ({}) RETURNING *",
                quote(table),
                fields.join(","),
                vec!["?"; fields.len()].join(",")
            )
        };
        self.query(&sql, &values)?
            .into_iter()
            .next()
            .ok_or_else(|| ServiceError::new("internal_error", "Insert did not return a record"))
    }
    pub(crate) fn update_record(
        &self,
        table: &str,
        id: &Value,
        data: &Map<String, Value>,
    ) -> Result<()> {
        self.validate_input(table, data, false)?;
        if data.is_empty() {
            return Ok(());
        }
        let set = data
            .keys()
            .map(|f| format!("{}=?", quote(f)))
            .collect::<Vec<_>>()
            .join(",");
        let mut values = data.values().map(sql_value).collect::<Result<Vec<_>>>()?;
        values.push(sql_value(id)?);
        self.db.execute(
            &format!(
                "UPDATE {} SET {} WHERE {}=?",
                quote(table),
                set,
                quote(self.primary_key(table)?)
            ),
            params_from_iter(&values),
        )?;
        Ok(())
    }
    pub fn catalog(&self) -> Result<Value> {
        let c = self.commerce()?;
        let mut items = self.query(
            &format!("SELECT * FROM {} ORDER BY id", quote(&c.catalog.entity)),
            &[],
        )?;
        items.retain(|r| {
            !c.catalog.fields.contains_key("status") || r[c.catalog.field("status")] == c.available
        });
        let mut lookups = Map::new();
        for (field, target, key) in inference::foreign_keys(&self.schema, &c.catalog.entity) {
            let mut values = Map::new();
            for item in &items {
                let id = &item[field];
                if id.is_null() {
                    continue;
                }
                if let Some(row) = self
                    .query(
                        &format!("SELECT * FROM {} WHERE {}=?", quote(target), quote(key)),
                        &[sql_value(id)?],
                    )?
                    .first()
                {
                    values.insert(display(id), json!(label(row)));
                }
            }
            lookups.insert(field.into(), Value::Object(values));
        }
        for row in &mut items {
            self.redact(&c.catalog.entity, row);
        }
        Ok(json!({"items":items,"lookups":lookups,"config":c}))
    }
    fn capacity(&self, c: &Commerce, item: &Value) -> Result<Option<i64>> {
        let Some((field, via)) = &c.capacity else {
            return Ok(None);
        };
        let row = if let Some(via) = via {
            let target_id = &item[&via.field];
            self.raw(&field.table, target_id)?
        } else {
            item.clone()
        };
        Ok(row[&field.field]
            .as_f64()
            .filter(|v| *v >= 0.0)
            .map(|v| v.floor() as i64))
    }
    fn order_lines(&self, c: &Commerce, order: &Value) -> Result<Vec<Value>> {
        self.query(
            &format!(
                "SELECT li.* FROM {} li JOIN {} ol ON ol.{}=li.id WHERE ol.{}=? ORDER BY li.id",
                quote(&c.line_item.entity),
                quote(&c.order_line.entity),
                quote(c.order_line.field("lineItem")),
                quote(c.order_line.field("order"))
            ),
            &[sql_value(order)?],
        )
    }
    fn allocated(&self, c: &Commerce, item_id: &Value, exclude: &[i64]) -> Result<i64> {
        let lines = self.query(
            &format!(
                "SELECT * FROM {} WHERE {}=?",
                quote(&c.line_item.entity),
                quote(c.line_item.field("catalogItem"))
            ),
            &[sql_value(item_id)?],
        )?;
        let mut total = 0i64;
        for line in lines {
            if exclude.contains(&line["id"].as_i64().unwrap_or(0)) {
                continue;
            }
            let status = line[c.line_item.field("status")].as_str().unwrap_or("");
            let quantity = if c.line_item.fields.contains_key("quantity") {
                line[c.line_item.field("quantity")].as_i64().unwrap_or(0)
            } else {
                1
            };
            let active = if status == c.fulfilled {
                true
            } else if status == "reserved" {
                let orders = self.query(
                    &format!(
                        "SELECT o.* FROM {} o JOIN {} ol ON o.id=ol.{} WHERE ol.{}=?",
                        quote(&c.order.entity),
                        quote(&c.order_line.entity),
                        quote(c.order_line.field("order")),
                        quote(c.order_line.field("lineItem"))
                    ),
                    &[sql_value(&line["id"])?],
                )?;
                orders.is_empty()
                    || orders.iter().any(|order| {
                        let status = order[c.order.field("status")].as_str().unwrap_or("");
                        let field = if status == "checkout_pending" {
                            c.order.field("expiresAt")
                        } else {
                            c.order.field("paymentDeadlineAt")
                        };
                        ["checkout_pending", "payment_pending"].contains(&status)
                            && chrono::DateTime::parse_from_rfc3339(
                                order[field].as_str().unwrap_or(""),
                            )
                            .is_ok_and(|t| t > Utc::now())
                    })
            } else {
                false
            };
            if active {
                total = total
                    .checked_add(quantity)
                    .ok_or_else(|| ServiceError::new("invalid", "Allocated quantity overflow"))?;
            }
        }
        Ok(total)
    }
    fn check_capacity(
        &self,
        c: &Commerce,
        items: &BTreeMap<i64, i64>,
        exclude: &[i64],
    ) -> Result<()> {
        for (id, requested) in items {
            let item = self.raw(&c.catalog.entity, &json!(id))?;
            if let Some(capacity) = self.capacity(c, &item)? {
                let allocated = self.allocated(c, &json!(id), exclude)?;
                if allocated
                    .checked_add(*requested)
                    .is_none_or(|n| n > capacity)
                {
                    return Err(ServiceError::new(
                        "bad_state",
                        "Not enough capacity available",
                    ));
                }
            }
        }
        Ok(())
    }
    fn order_capacity(&self, c: &Commerce, order: &Value) -> Result<()> {
        let lines = self.order_lines(c, order)?;
        let mut items = BTreeMap::new();
        let mut exclude = Vec::new();
        for line in lines {
            let id = line[c.line_item.field("catalogItem")]
                .as_i64()
                .ok_or_else(|| ServiceError::new("invalid", "Invalid catalog ID"))?;
            let quantity = if c.line_item.fields.contains_key("quantity") {
                line[c.line_item.field("quantity")].as_i64().unwrap_or(0)
            } else {
                1
            };
            *items.entry(id).or_insert(0) += quantity;
            exclude.push(line["id"].as_i64().unwrap_or(0));
        }
        self.check_capacity(c, &items, &exclude)
    }
    pub fn checkout(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        let c = self.commerce()?;
        let lines = input["items"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= c.max_lines)
            .ok_or_else(|| {
                ServiceError::new("invalid", "Cart must contain a valid number of lines")
            })?;
        let user = if auth.has_scope("*") {
            input["user_id"].as_i64().or(auth.user_id)
        } else {
            auth.user_id
        }
        .ok_or_else(|| ServiceError::new("unauthorized", "Authenticated user required"))?;
        if input["user_id"].as_i64().is_some_and(|u| u != user) && !auth.has_scope("*") {
            return Err(ServiceError::new(
                "forbidden",
                "Cannot checkout for another customer",
            ));
        }
        self.authorize(
            &c.order.entity,
            "create",
            auth,
            Some(&json!({c.order.field("user"):user})),
        )?;
        let tx = crate::service::transaction(&self.db)?;
        self.raw("user", &json!(user))?;
        let mut prepared = Vec::new();
        let mut requested = BTreeMap::new();
        let mut amount = 0i64;
        for line in lines {
            let id = line["item_id"]
                .as_i64()
                .filter(|id| *id > 0)
                .ok_or_else(|| {
                    ServiceError::new("invalid", "item_id must be a positive integer")
                })?;
            let quantity = if line.get("quantity").is_none() {
                1
            } else {
                line["quantity"].as_i64().unwrap_or(0)
            };
            if !(1..=c.max_quantity).contains(&quantity) {
                return Err(ServiceError::new(
                    "invalid",
                    "Quantity outside checkout limits",
                ));
            }
            let item = self.raw(&c.catalog.entity, &json!(id))?;
            if c.catalog.fields.contains_key("status")
                && item[c.catalog.field("status")] != c.available
            {
                return Err(ServiceError::new(
                    "bad_state",
                    "Catalog item is not available",
                ));
            }
            let price = item[c.catalog.field("price")]
                .as_i64()
                .filter(|v| *v > 0)
                .ok_or_else(|| {
                    ServiceError::new("invalid", "Catalog item has no configured price")
                })?;
            amount = price
                .checked_mul(quantity)
                .and_then(|v| amount.checked_add(v))
                .ok_or_else(|| ServiceError::new("invalid", "Cart total exceeds integer range"))?;
            let options = line["options"].as_object().cloned().unwrap_or_default();
            if options.keys().any(|f| !c.options.contains(f)) {
                return Err(ServiceError::new("invalid", "Unknown cart option"));
            }
            let mut values = Map::new();
            for field in &c.options {
                if let Some(value) = options.get(field) {
                    values.insert(field.clone(), value.clone());
                }
            }
            self.validate_input(&c.line_item.entity, &values, false)?;
            *requested.entry(id).or_insert(0i64) += quantity;
            prepared.push((id, quantity, price, values));
        }
        self.check_capacity(&c, &requested, &[])?;
        let expires = after(c.expiry_minutes * 60);
        let mut data = Map::from_iter([
            (c.order.field("user").into(), json!(user)),
            (c.order.field("status").into(), json!("checkout_pending")),
            (c.order.field("amount").into(), json!(amount)),
            (c.order.field("currency").into(), json!(c.currency)),
            (c.order.field("expiresAt").into(), json!(expires)),
        ]);
        if c.order.fields.contains_key("client") {
            data.insert(
                c.order.field("client").into(),
                json!(input["client"].as_str().unwrap_or("web")),
            );
        }
        let order = self.insert_record(&c.order.entity, &data)?;
        let mut ids = Vec::new();
        for (id, quantity, price, options) in prepared {
            let repetitions = if c.line_item.fields.contains_key("quantity") {
                1
            } else {
                quantity
            };
            for _ in 0..repetitions {
                let mut data = options.clone();
                data.insert(c.line_item.field("catalogItem").into(), json!(id));
                data.insert(c.line_item.field("price").into(), json!(price));
                data.insert(c.line_item.field("status").into(), json!("reserved"));
                if c.line_item.fields.contains_key("user") {
                    data.insert(c.line_item.field("user").into(), json!(user));
                }
                if c.line_item.fields.contains_key("quantity") {
                    data.insert(c.line_item.field("quantity").into(), json!(quantity));
                }
                let line = self.insert_record(&c.line_item.entity, &data)?;
                ids.push(line["id"].clone());
                self.insert_record(
                    &c.order_line.entity,
                    &Map::from_iter([
                        (c.order_line.field("order").into(), order["id"].clone()),
                        (c.order_line.field("lineItem").into(), line["id"].clone()),
                    ]),
                )?;
            }
        }
        self.queue_effect(&format!("checkout:{}",order["id"]),"emit",&json!({"event":"commerce.order_created","order_id":order["id"],"line_item_ids":ids,"amount_pence":amount}),&json!({"operation":"commerce.checkout"}))?;
        crate::service::commit(tx)?;
        Ok(
            json!({"order_id":order["id"],"booking_id":order["id"],"line_item_ids":ids,"ticket_ids":ids,"amount_pence":amount,"currency":c.currency,"expires_at":expires,"status":"checkout_pending"}),
        )
    }
    pub fn reserve_booking(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        let mut input = input.clone();
        let item = input["performance_id"].clone();
        let items = if let Some(tickets) = input["tickets"].as_array().filter(|t| !t.is_empty()) {
            tickets.iter().map(|ticket|json!({"item_id":item,"quantity":1,"options":{"ticket_type":ticket["ticket_type"].as_str().or(input["ticket_type"].as_str()).unwrap_or("standard"),"seat":ticket["seat"]}})).collect::<Vec<_>>()
        } else {
            vec![
                json!({"item_id":item,"quantity":input["quantity"].as_i64().unwrap_or(1),"options":{"ticket_type":input["ticket_type"].as_str().unwrap_or("standard")}}),
            ]
        };
        input["items"] = json!(items);
        self.checkout(&input, auth)
    }
    pub fn payment_session(&self, id: &Value, auth: &AuthContext) -> Result<Value> {
        let c = self.commerce()?;
        let tx = crate::service::transaction(&self.db)?;
        let order = self.raw(&c.order.entity, id)?;
        self.authorize(&c.order.entity, "update", auth, Some(&order))?;
        let status = order[c.order.field("status")].as_str().unwrap_or("");
        if !["checkout_pending", "payment_pending"].contains(&status) {
            return Err(ServiceError::new(
                "bad_state",
                "Order is not awaiting payment",
            ));
        }
        let deadline = if status == "checkout_pending" {
            c.order.field("expiresAt")
        } else {
            c.order.field("paymentDeadlineAt")
        };
        if chrono::DateTime::parse_from_rfc3339(order[deadline].as_str().unwrap_or(""))
            .map_or(true, |t| t <= Utc::now())
        {
            self.expire_order(&c, id)?;
            crate::service::commit(tx)?;
            return Err(ServiceError::new("bad_state", "Order expired"));
        }
        self.order_capacity(&c, id)?;
        let provider = std::env::var("PAYMENT_PROVIDER").unwrap_or_else(|_| "local".into());
        if !["local", "fake"].contains(&provider.as_str()) || !fake_allowed() {
            return Err(ServiceError::new(
                "invalid",
                "Local/fake payment providers require an explicit production opt-in",
            ));
        }
        let existing = order[c.order.field("paymentReference")]
            .as_str()
            .filter(|s| !s.is_empty());
        let reference = existing.map(str::to_string).unwrap_or(secret("fake_ps_")?);
        let expires = if existing.is_some() {
            order[c.order.field("paymentDeadlineAt")]
                .as_str()
                .unwrap_or("")
                .to_string()
        } else {
            after(c.payment_deadline_minutes * 60)
        };
        let transaction = if existing.is_some() {
            self.query(
                &format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(&c.transaction.entity),
                    quote(c.transaction.field("reference"))
                ),
                &[SqlValue::Text(reference.clone())],
            )?
            .into_iter()
            .next()
            .ok_or_else(|| ServiceError::new("bad_state", "Payment reference has no transaction"))?
        } else {
            let mut data = Map::from_iter([
                (
                    c.transaction.field("user").into(),
                    order[c.order.field("user")].clone(),
                ),
                (
                    c.transaction.field("amount").into(),
                    order[c.order.field("amount")].clone(),
                ),
                (c.transaction.field("status").into(), json!("pending")),
                (c.transaction.field("reference").into(), json!(reference)),
            ]);
            if c.transaction.fields.contains_key("type") {
                data.insert(c.transaction.field("type").into(), json!("purchase"));
            }
            if c.transaction.fields.contains_key("client") {
                data.insert(c.transaction.field("client").into(), json!("web"));
            }
            let transaction = self.insert_record(&c.transaction.entity, &data)?;
            for line in self.order_lines(&c, id)? {
                self.insert_record(
                    &c.transaction_line.entity,
                    &Map::from_iter([
                        (
                            c.transaction_line.field("transaction").into(),
                            transaction["id"].clone(),
                        ),
                        (
                            c.transaction_line.field("lineItem").into(),
                            line["id"].clone(),
                        ),
                    ]),
                )?;
            }
            self.update_record(
                &c.order.entity,
                id,
                &Map::from_iter([
                    (c.order.field("status").into(), json!("payment_pending")),
                    (c.order.field("paymentReference").into(), json!(reference)),
                    (c.order.field("paymentDeadlineAt").into(), json!(expires)),
                ]),
            )?;
            self.queue_effect(&format!("payment-session:{}",id),"call",&json!({"service":"payment","action":"create_session","order_id":id,"reference":reference}),&json!({"operation":"commerce.create_payment_session"}))?;
            transaction
        };
        crate::service::commit(tx)?;
        Ok(
            json!({"order_id":id,"booking_id":id,"transaction_id":transaction["id"],"reference":reference,"client_secret":format!("{reference}_secret"),"amount_pence":order[c.order.field("amount")],"currency":order[c.order.field("currency")],"provider":provider,"status":"payment_pending","payment_deadline_at":expires,"next_action":{"type":"redirect","url":format!("/commerce/payments/local-simulator?reference={reference}&provider={provider}")}}),
        )
    }
    fn payment_tables(&self) -> Result<()> {
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_payment_event(id INTEGER PRIMARY KEY AUTOINCREMENT,provider TEXT NOT NULL,event_id TEXT NOT NULL,reference TEXT NOT NULL,status TEXT NOT NULL,occurred_at TEXT,order_id INTEGER,transaction_id INTEGER,result_status TEXT,created_at TEXT DEFAULT CURRENT_TIMESTAMP,UNIQUE(provider,event_id));")?;
        Ok(())
    }
    pub fn payment_webhook(&self, input: &Value) -> Result<Value> {
        let c = self.commerce()?;
        let event = input["event_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ServiceError::new("invalid", "event_id is required"))?;
        let reference = input["reference"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ServiceError::new("invalid", "reference is required"))?;
        let status = input["status"]
            .as_str()
            .filter(|s| ["authorized", "captured", "failed", "cancelled", "refunded"].contains(s))
            .ok_or_else(|| ServiceError::new("invalid", "Unsupported payment status"))?;
        if input.get("occurred_at").is_some_and(|v| {
            v.as_str()
                .is_none_or(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
        }) {
            return Err(ServiceError::new(
                "invalid",
                "occurred_at must be an ISO date-time",
            ));
        }
        self.payment_tables()?;
        let tx = crate::service::transaction(&self.db)?;
        let transaction = self
            .query(
                &format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(&c.transaction.entity),
                    quote(c.transaction.field("reference"))
                ),
                &[SqlValue::Text(reference.into())],
            )?
            .into_iter()
            .next()
            .ok_or_else(|| ServiceError::new("not_found", "Transaction not found"))?;
        let order = self
            .query(
                &format!(
                    "SELECT * FROM {} WHERE {}=?",
                    quote(&c.order.entity),
                    quote(c.order.field("paymentReference"))
                ),
                &[SqlValue::Text(reference.into())],
            )?
            .into_iter()
            .next()
            .ok_or_else(|| ServiceError::new("not_found", "Order not found"))?;
        let provider = input["provider"].as_str().unwrap_or("local");
        let inserted=self.db.execute("INSERT OR IGNORE INTO openb2c_payment_event(provider,event_id,reference,status,occurred_at) VALUES (?,?,?,?,?)",params![provider,event,reference,status,input["occurred_at"].as_str()])?;
        if inserted == 0 {
            let prior=self.query("SELECT reference,status FROM openb2c_payment_event WHERE provider=? AND event_id=?",&[SqlValue::Text(provider.into()),SqlValue::Text(event.into())])?;
            if prior[0]["reference"] != reference || prior[0]["status"] != status {
                return Err(ServiceError::new(
                    "conflict",
                    "Event ID was already used for a different payment",
                ));
            }
            crate::service::commit(tx)?;
            return Ok(
                json!({"order_id":order["id"],"booking_id":order["id"],"transaction_id":transaction["id"],"status":order[c.order.field("status")],"idempotent":true}),
            );
        }
        let current = order[c.order.field("status")].as_str().unwrap_or("");
        let result = if ["paid", "refunded"].contains(&current) && status != "refunded" {
            current
        } else if status == "refunded" {
            "refunded"
        } else if ["authorized", "captured"].contains(&status) {
            if self.order_capacity(&c, &order["id"]).is_ok() {
                "paid"
            } else if status == "captured" {
                "refund_required"
            } else {
                "cancelled"
            }
        } else {
            "cancelled"
        };
        let transaction_status = match result {
            "paid" => "completed",
            "refunded" => "refunded",
            "refund_required" => "refund_required",
            _ => {
                if status == "failed" {
                    "failed"
                } else {
                    "cancelled"
                }
            }
        };
        let line_status = if result == "paid" {
            &c.fulfilled
        } else {
            "cancelled"
        };
        self.update_record(
            &c.order.entity,
            &order["id"],
            &Map::from_iter([(c.order.field("status").into(), json!(result))]),
        )?;
        self.update_record(
            &c.transaction.entity,
            &transaction["id"],
            &Map::from_iter([(
                c.transaction.field("status").into(),
                json!(transaction_status),
            )]),
        )?;
        for line in self.order_lines(&c, &order["id"])? {
            self.update_record(
                &c.line_item.entity,
                &line["id"],
                &Map::from_iter([(c.line_item.field("status").into(), json!(line_status))]),
            )?;
        }
        self.db.execute("UPDATE openb2c_payment_event SET order_id=?,transaction_id=?,result_status=? WHERE provider=? AND event_id=?",params![order["id"].as_i64(),transaction["id"].as_i64(),result,provider,event])?;
        self.queue_effect(&format!("payment:{reference}:{result}"),"emit",&json!({"event":format!("commerce.order_{result}"),"order_id":order["id"],"transaction_id":transaction["id"],"reference":reference}),&json!({"operation":"commerce.payment_webhook"}))?;
        if result == "paid" && current != "paid" {
            let customer = self.raw("user", &order[c.order.field("user")])?;
            let lines = self.order_lines(&c, &order["id"])?;
            let mut receipt = Vec::new();
            for line in &lines {
                let catalog =
                    self.raw(&c.catalog.entity, &line[c.line_item.field("catalogItem")])?;
                receipt.push(json!({"id":line["id"],"catalog_item_id":catalog["id"],"catalog_label":self.catalog_label(&c,&catalog)?,"quantity":line.get(c.line_item.field("quantity")).cloned().unwrap_or(json!(1)),"unit_amount_pence":line[c.line_item.field("price")],"amount_pence":line[c.line_item.field("price")].as_i64().unwrap_or(0)*line.get(c.line_item.field("quantity")).and_then(Value::as_i64).unwrap_or(1)}));
            }
            self.queue_effect(&format!("receipt:{}",order["id"]),"notify",&json!({"channel":"email","template":"receipt","to":"customer","customer":{"id":customer["id"],"email":customer["email"],"name":customer["name"]},"line_items":receipt,"ticket_ids":lines.iter().map(|l|l["id"].clone()).collect::<Vec<_>>(),"amount_pence":order[c.order.field("amount")],"currency":order[c.order.field("currency")],"order_id":order["id"],"transaction_id":transaction["id"],"reference":reference}),&json!({"operation":"commerce.payment_webhook"}))?;
            self.queue_effect(&format!("analytics:{}",order["id"]),"call",&json!({"service":"analytics","action":"track_purchase","order_id":order["id"],"amount_pence":order[c.order.field("amount")]}),&json!({}))?;
        }
        if result == "refund_required" {
            self.queue_effect(&format!("refund-required:{reference}"),"call",&json!({"service":"payment","action":"refund_required","reference":reference,"order_id":order["id"]}),&json!({}))?;
        }
        crate::service::commit(tx)?;
        Ok(
            json!({"order_id":order["id"],"booking_id":order["id"],"transaction_id":transaction["id"],"status":result,"idempotent":current==result}),
        )
    }
    pub fn catalog_label(&self, c: &Commerce, item: &Value) -> Result<String> {
        let Some(field) = c.catalog.fields.get("label") else {
            return Ok(label(item));
        };
        let value = &item[field];
        if let Some((_, target, key)) = inference::foreign_keys(&self.schema, &c.catalog.entity)
            .into_iter()
            .find(|(f, _, _)| *f == field)
            && let Some(row) = self
                .query(
                    &format!("SELECT * FROM {} WHERE {}=?", quote(target), quote(key)),
                    &[sql_value(value)?],
                )?
                .first()
        {
            return Ok(label(row));
        }
        Ok(value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| label(item)))
    }
    fn expire_order(&self, c: &Commerce, id: &Value) -> Result<()> {
        let order = self.raw(&c.order.entity, id)?;
        if !["checkout_pending", "payment_pending"]
            .contains(&order[c.order.field("status")].as_str().unwrap_or(""))
        {
            return Ok(());
        }
        self.update_record(
            &c.order.entity,
            id,
            &Map::from_iter([(c.order.field("status").into(), json!("expired"))]),
        )?;
        for line in self.order_lines(c, id)? {
            self.update_record(
                &c.line_item.entity,
                &line["id"],
                &Map::from_iter([(c.line_item.field("status").into(), json!("cancelled"))]),
            )?;
        }
        if let Some(reference) = order[c.order.field("paymentReference")].as_str() {
            self.db.execute(
                &format!(
                    "UPDATE {} SET {}='cancelled' WHERE {}=? AND {} IN ('pending','authorized')",
                    quote(&c.transaction.entity),
                    quote(c.transaction.field("status")),
                    quote(c.transaction.field("reference")),
                    quote(c.transaction.field("status"))
                ),
                [reference],
            )?;
        }
        Ok(())
    }
    pub fn expire_orders(&self) -> Result<Value> {
        let c = self.commerce()?;
        let tx = crate::service::transaction(&self.db)?;
        let rows=self.query(&format!("SELECT * FROM {} WHERE ({}='checkout_pending' AND julianday({})<=julianday('now')) OR ({}='payment_pending' AND julianday({})<=julianday('now'))",quote(&c.order.entity),quote(c.order.field("status")),quote(c.order.field("expiresAt")),quote(c.order.field("status")),quote(c.order.field("paymentDeadlineAt"))),&[])?;
        for row in &rows {
            self.expire_order(&c, &row["id"])?;
        }
        crate::service::commit(tx)?;
        Ok(json!({"expired":rows.len()}))
    }
}
fn display(v: &Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}
pub fn label(row: &Value) -> String {
    for field in ["name", "title", "email", "reference", "starts_at", "date"] {
        if let Some(value) = row[field].as_str().filter(|s| !s.is_empty()) {
            return value.into();
        }
    }
    format!("#{}", row["id"])
}
