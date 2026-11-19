//! The order payment command; event orchestration is compiled from Nix.
use super::*;
use crate::records::{
    OrderPaymentsLink as Link, OrderPaymentsLinkWrite, OrdersOrder, OrdersOrderStatus,
    PaymentsPayment as Payment, PaymentsPaymentStatus, Record,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderPayment {
    idempotency_key: String,
}
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
        let order = self.owned::<OrdersOrder>(id, "orders", "orders.pay", auth)?;
        if let Some(link) = Link::first(
            self,
            "WHERE order_id=? AND request_key=?",
            &[&id, &input.idempotency_key],
        )? {
            return self.payment_view(&Payment::get(self, link.payment_id)?);
        }
        if order.status != OrdersOrderStatus::Pending {
            return Err(state("Only pending orders accept payments"));
        }
        if let Some(link) = Link::first(self, "WHERE order_id=? ORDER BY id DESC LIMIT 1", &[&id])?
        {
            if Payment::get(self, link.payment_id)?.status != PaymentsPaymentStatus::Failed {
                return Err(state(
                    "An existing payment must finish before creating another attempt",
                ));
            }
        }
        let payment = self.cap_payment(
            order.user_id,
            order.amount_minor,
            &order.currency,
            &format!("order:{id}:{}", input.idempotency_key),
            auth,
            now,
        )?;
        OrderPaymentsLinkWrite {
            order_id: id.into(),
            payment_id: payment.id.into(),
            request_key: input.idempotency_key.into(),
            ..Default::default()
        }
        .insert(self)?;
        let result = self.payment_view(&payment)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
}
