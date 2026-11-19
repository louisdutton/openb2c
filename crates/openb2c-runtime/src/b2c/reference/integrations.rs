//! Consumers subscribe to domain events; providers do not depend on consumers.
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderPayment {
    idempotency_key: String,
}
#[cfg(feature = "reference")]
pub(super) struct OrderPayments;
#[cfg(feature = "reference")]
pub(super) struct OrderEntitlements;
#[cfg(feature = "reference")]
pub(super) struct SubscriptionEntitlements;

impl Service {
    pub fn b2c_order_payment(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.b2c_order_payment_at(id, input, auth, Utc::now())
    }
    pub fn b2c_order_payment_at(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        self.cap_command("order_payments", "orders.pay", auth)?;
        let input: OrderPayment = decode(input)?;
        key(&input.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        let order = self.cap_owned("orders", "order", id, "orders.pay", auth)?;
        let links = self.cap_table("order_payments", "link")?;
        let previous = self.query(
            &format!(
                "SELECT * FROM {} WHERE order_id=? AND request_key=?",
                quote(links)
            ),
            &[
                SqlValue::Integer(id),
                SqlValue::Text(input.idempotency_key.clone()),
            ],
        )?;
        if let Some(link) = previous.first() {
            return self.cap_response(
                self.cap_table("payments", "payment")?,
                self.raw(self.cap_table("payments", "payment")?, &link["payment_id"])?,
            );
        }
        if order["status"] != "pending" {
            return Err(state("Only pending orders accept payments"));
        }
        for link in self.query(
            &format!(
                "SELECT * FROM {} WHERE order_id=? ORDER BY id DESC LIMIT 1",
                quote(links)
            ),
            &[SqlValue::Integer(id)],
        )? {
            let previous = self.raw(self.cap_table("payments", "payment")?, &link["payment_id"])?;
            if previous["status"] != "failed" {
                return Err(state(
                    "An existing payment must finish before creating another attempt",
                ));
            }
        }
        let payment = self.cap_payment(
            integer(&order, "user_id")?,
            integer(&order, "amount_minor")?,
            text(&order, "currency")?,
            &format!("order:{id}:{}", input.idempotency_key),
            auth,
            now,
        )?;
        self.cap_insert(
            links,
            json!({"order_id":id,"payment_id":payment["id"],"request_key":input.idempotency_key}),
        )?;
        let result = self.cap_response(self.cap_table("payments", "payment")?, payment)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
}

#[cfg(feature = "reference")]
impl Consumer for OrderPayments {
    fn capability(&self) -> &'static str {
        "order_payments"
    }
    fn handle(
        &self,
        service: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let links = service.cap_table("order_payments", "link")?;
        match event {
            Event::Payment {
                before: _,
                after: payment,
            } => {
                let Some(link) =
                    service.cap_find(links, "payment_id", sql_value(&payment["id"])?)?
                else {
                    return Ok(());
                };
                let table = service.cap_table("orders", "order")?;
                let old = service.raw(table, &link["order_id"])?;
                if old["user_id"] != payment["user_id"]
                    || old["amount_minor"] != payment["amount_minor"]
                    || old["currency"] != payment["currency"]
                {
                    return Err(state("Payment does not match its order"));
                }
                let status = match text(payment, "status")? {
                    "succeeded" if old["status"] == "pending" => "paid",
                    "refunded" if old["status"] == "paid" => "refunded",
                    "failed" => return Ok(()),
                    _ => return Err(state("Payment conflicts with its order state")),
                };
                let next =
                    service.cap_update(table, integer(&old, "id")?, json!({"status":status}))?;
                service.cap_audit(table, "orders.payment", auth, Some(&old), &next)?;
                service.cap_publish(
                    Event::Order {
                        before: old,
                        after: next,
                    },
                    auth,
                    now,
                )
            }
            Event::Order {
                before: _,
                after: order,
            } if order["status"] == "cancelled" => {
                for link in service.query(
                    &format!("SELECT * FROM {} WHERE order_id=?", quote(links)),
                    &[sql_value(&order["id"])?],
                )? {
                    service.cap_cancel_payment(integer(&link, "payment_id")?, auth)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(feature = "reference")]
impl Consumer for OrderEntitlements {
    fn enabled(&self, s: &Service) -> bool {
        s.schema.capabilities.contains_key("entitlements")
            && s.schema.capabilities.contains_key("orders")
    }
    fn capability(&self) -> &'static str {
        "order_entitlements"
    }
    fn handle(
        &self,
        service: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Event::Order {
            before: _,
            after: order,
        } = event
        else {
            return Ok(());
        };
        let id = integer(order, "id")?;
        let table = service.cap_table("orders", "item")?;
        for item in service.query(
            &format!("SELECT * FROM {} WHERE order_id=?", quote(table)),
            &[SqlValue::Integer(id)],
        )? {
            let source = format!("order:{id}:item:{}", item["id"]);
            if order["status"] == "paid"
                && service
                    .schema
                    .capabilities
                    .contains_key("order_entitlements")
            {
                service.cap_grant(
                    integer(order, "user_id")?,
                    text(&item, "product_code")?,
                    &source,
                    None,
                    auth,
                    now,
                )?;
            } else if [json!("refunded"), json!("cancelled")].contains(&order["status"]) {
                service.cap_revoke_source(&source, auth)?;
            }
        }
        Ok(())
    }
}
#[cfg(feature = "reference")]
impl Consumer for SubscriptionEntitlements {
    fn enabled(&self, s: &Service) -> bool {
        s.schema.capabilities.contains_key("entitlements")
            && s.schema.capabilities.contains_key("subscriptions")
    }
    fn capability(&self) -> &'static str {
        "subscription_entitlements"
    }
    fn handle(
        &self,
        service: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Event::Subscription {
            before: _,
            after: subscription,
        } = event
        else {
            return Ok(());
        };
        let source = format!("subscription:{}", subscription["id"]);
        if subscription["status"] == "active"
            && service
                .schema
                .capabilities
                .contains_key("subscription_entitlements")
        {
            service.cap_grant(
                integer(subscription, "user_id")?,
                text(subscription, "product_code")?,
                &source,
                subscription["current_period_end"].as_str(),
                auth,
                now,
            )?;
        } else if subscription["status"] != "active" {
            service.cap_revoke_source(&source, auth)?;
        }
        Ok(())
    }
}
