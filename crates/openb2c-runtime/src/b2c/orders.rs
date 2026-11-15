use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Item {
    price_id: i64,
    quantity: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateOrder {
    idempotency_key: String,
    items: Vec<Item>,
}

impl Service {
    pub fn b2c_create_order(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("orders", "orders.create", auth)?;
        let user = self.cap_customer(auth)?;
        let mut input: CreateOrder = decode(input)?;
        key(&input.idempotency_key)?;
        if input.items.is_empty()
            || input.items.len() > 20
            || input
                .items
                .iter()
                .any(|i| i.price_id < 1 || !(1..=100).contains(&i.quantity))
        {
            return Err(invalid(
                "An order requires 1–20 lines with quantities between 1 and 100",
            ));
        }
        input.items.sort_by_key(|i| i.price_id);
        if input
            .items
            .windows(2)
            .any(|w| w[0].price_id == w[1].price_id)
        {
            return Err(invalid("Duplicate price lines are not allowed"));
        }
        let fingerprint =
            serde_json::to_string(&input.items).map_err(|_| invalid("Invalid items"))?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("orders", "order")?;
        let existing = self.query(
            &format!(
                "SELECT * FROM {} WHERE user_id=? AND request_key=?",
                quote(table)
            ),
            &[
                SqlValue::Integer(user),
                SqlValue::Text(input.idempotency_key.clone()),
            ],
        )?;
        if let Some(order) = existing.first() {
            if order["request_json"] != fingerprint {
                return Err(ServiceError::new(
                    "conflict",
                    "Idempotency key already used with different items",
                ));
            }
            return self.cap_order_view(order.clone());
        }
        let mut amount = 0i64;
        let mut currency = None;
        let mut lines = Vec::new();
        for item in input.items {
            let price = self.cap_price(item.price_id)?;
            if price.billing_interval != "one_off" {
                return Err(invalid("Recurring prices require a subscription"));
            }
            if currency.as_ref().is_some_and(|c| c != &price.currency) {
                return Err(invalid("An order must use a single currency"));
            }
            amount = amount
                .checked_add(
                    price
                        .amount_minor
                        .checked_mul(item.quantity)
                        .ok_or_else(|| invalid("Order total overflow"))?,
                )
                .ok_or_else(|| invalid("Order total overflow"))?;
            currency = Some(price.currency.clone());
            lines.push((item, price));
        }
        let order=self.cap_insert(table,json!({"user_id":user,"status":"pending","amount_minor":amount,"currency":currency,"request_key":input.idempotency_key,"request_json":fingerprint,"created_at":Utc::now().to_rfc3339()}))?;
        for (item, price) in lines {
            self.cap_insert(self.cap_table("orders","item")?,json!({"order_id":order["id"],"product_id":price.product_id,"price_id":price.id,"product_code":price.product_code,"name":price.name,"quantity":item.quantity,"unit_amount_minor":price.amount_minor}))?;
        }
        self.cap_audit(table, "orders.create", auth, None, &order)?;
        let result = self.cap_order_view(order)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub(super) fn cap_order_view(&self, order: Value) -> Result<Value> {
        let items = self.query(
            &format!(
                "SELECT * FROM {} WHERE order_id=? ORDER BY id",
                quote(self.cap_table("orders", "item")?)
            ),
            &[sql_value(&order["id"])?],
        )?;
        let order = self.cap_response(self.cap_table("orders", "order")?, order)?;
        let items = items
            .into_iter()
            .map(|row| self.cap_response(self.cap_table("orders", "item")?, row))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"order":order,"items":items}))
    }
    pub fn b2c_order(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_order_view(self.cap_owned("orders", "order", id, "orders.read", auth)?)
    }
    pub fn b2c_cancel_order(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        let tx = crate::service::transaction(&self.db)?;
        let old = self.cap_owned("orders", "order", id, "orders.cancel", auth)?;
        if old["status"] != "pending" {
            return Err(state("Only pending orders can be cancelled"));
        }
        let table = self.cap_table("orders", "order")?;
        let order = self.cap_update(table, id, json!({"status":"cancelled"}))?;
        self.cap_audit(table, "orders.cancel", auth, Some(&old), &order)?;
        self.cap_publish(
            Event::Order {
                before: old,
                after: order.clone(),
            },
            auth,
            Utc::now(),
        )?;
        let result = self.cap_order_view(order)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
}
