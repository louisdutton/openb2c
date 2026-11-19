use super::*;
use crate::records::{
    OrdersItem as OrderItem, OrdersItemWrite, OrdersOrder as Order, OrdersOrderStatus as Status,
    OrdersOrderWrite as OrderWrite, Record,
};
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
        self.b2c_create_order_at(input, auth, Utc::now())
    }
    pub fn b2c_create_order_at(
        &self,
        input: &Value,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
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
        // The persisted request fingerprint is a serialization boundary, not a business record.
        let fingerprint =
            serde_json::to_string(&input.items).map_err(|e| invalid(e.to_string()))?;
        let tx = crate::service::transaction(&self.db)?;
        if let Some(order) = Order::first(
            self,
            "WHERE user_id=? AND request_key=?",
            &[&user, &input.idempotency_key],
        )? {
            if order.request_json != fingerprint {
                return Err(ServiceError::new(
                    "conflict",
                    "Idempotency key already used with different items",
                ));
            }
            return self.cap_order_view(order);
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
        let order = OrderWrite {
            user_id: user.into(),
            status: Status::Pending.into(),
            amount_minor: amount.into(),
            currency: currency.expect("Validated nonempty order").into(),
            request_key: input.idempotency_key.into(),
            request_json: fingerprint.into(),
            created_at: now.to_rfc3339().into(),
            ..Default::default()
        }
        .insert(self)?;
        for (item, price) in lines {
            OrdersItemWrite {
                order_id: order.id.into(),
                product_id: price.product_id.into(),
                price_id: price.id.into(),
                product_code: price.product_code.into(),
                name: price.name.into(),
                quantity: item.quantity.into(),
                unit_amount_minor: price.amount_minor.into(),
                ..Default::default()
            }
            .insert(self)?;
        }
        order.audit(self, "orders.create", auth, None)?;
        let result = self.cap_order_view(order)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub(super) fn cap_order_view(&self, order: Order) -> Result<Value> {
        let items = OrderItem::select(self, "WHERE order_id=? ORDER BY id", &[&order.id])?
            .iter()
            .map(|r| r.wire(self))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"order":order.wire(self)?,"items":items}))
    }
    pub fn b2c_order(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_order_view(self.owned::<Order>(id, "orders", "orders.read", auth)?)
    }
    pub fn b2c_cancel_order(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        let tx = crate::service::transaction(&self.db)?;
        let old = self.owned::<Order>(id, "orders", "orders.cancel", auth)?;
        if old.status != Status::Pending {
            return Err(state("Only pending orders can be cancelled"));
        }
        let order = OrderWrite {
            status: Status::Cancelled.into(),
            ..Default::default()
        }
        .update(self, id)?;
        order.audit(self, "orders.cancel", auth, Some(&old))?;
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
