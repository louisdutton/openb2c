use super::*;
use crate::records::{
    PaymentsPayment as Payment, PaymentsPaymentWrite as PaymentWrite, Record,
    SubscriptionsAttempt as Attempt, SubscriptionsAttemptWrite as AttemptWrite,
    SubscriptionsSubscription as Subscription,
    SubscriptionsSubscriptionBillingInterval as Interval,
    SubscriptionsSubscriptionStatus as Status, SubscriptionsSubscriptionWrite as SubscriptionWrite,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSubscription {
    idempotency_key: String,
    price_id: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    #[serde(default = "period_end")]
    at_period_end: bool,
}
fn period_end() -> bool {
    true
}

impl Service {
    pub fn b2c_create_subscription(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.b2c_create_subscription_at(input, auth, Utc::now())
    }
    pub fn b2c_create_subscription_at(
        &self,
        input: &Value,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        self.cap_command("subscriptions", "subscriptions.create", auth)?;
        let user = self.cap_customer(auth)?;
        let input: CreateSubscription = decode(input)?;
        key(&input.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        if let Some(row) = Subscription::first(
            self,
            "WHERE user_id=? AND request_key=?",
            &[&user, &input.idempotency_key],
        )? {
            if row.price_id != input.price_id {
                return Err(ServiceError::new(
                    "conflict",
                    "Subscription key already used with a different price",
                ));
            }
            return self.cap_subscription_view(row);
        }
        let price = self.cap_price(input.price_id)?;
        let interval = Interval::try_from(price.billing_interval.as_str())
            .map_err(|_| invalid("Subscriptions require a monthly or yearly price"))?;
        let row = SubscriptionWrite {
            user_id: user.into(),
            price_id: price.id.into(),
            product_code: price.product_code.into(),
            amount_minor: price.amount_minor.into(),
            currency: price.currency.into(),
            billing_interval: interval.into(),
            status: Status::Pending.into(),
            cancel_at_period_end: 0.into(),
            cycle: (-1).into(),
            request_key: input.idempotency_key.into(),
            created_at: now.to_rfc3339().into(),
            ..Default::default()
        }
        .insert(self)?;
        self.cap_subscription_attempt(&row, 0, 0, "initial", auth, now)?;
        row.audit(self, "subscriptions.create", auth, None)?;
        let result = self.cap_subscription_view(row)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub(super) fn cap_subscription_view(&self, subscription: Subscription) -> Result<Value> {
        let payment = Attempt::first(
            self,
            "WHERE subscription_id=? ORDER BY cycle DESC,attempt DESC LIMIT 1",
            &[&subscription.id],
        )?
        .map(|a| Payment::get(self, a.payment_id))
        .transpose()?;
        self.cap_subscription_response(subscription, payment)
    }
    fn cap_subscription_response(
        &self,
        subscription: Subscription,
        payment: Option<Payment>,
    ) -> Result<Value> {
        Ok(
            json!({"subscription":subscription.wire(self)?,"payment":payment.as_ref().map(|p|self.payment_view(p)).transpose()?,"standing_order_notice":"You control any standing order. Cancel it with your bank when ending this subscription."}),
        )
    }
    pub fn b2c_subscription(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_subscription_view(self.owned::<Subscription>(
            id,
            "subscriptions",
            "subscriptions.read",
            auth,
        )?)
    }
    fn cap_subscription_attempt(
        &self,
        subscription: &Subscription,
        cycle: i64,
        attempt: i64,
        request: &str,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Payment> {
        let id = subscription.id;
        let mut payment = self.cap_payment(
            subscription.user_id,
            subscription.amount_minor,
            &subscription.currency,
            &format!("subscription:{id}:{cycle}:{attempt}"),
            auth,
            now,
        )?;
        // A standing order keeps the first request's reference across renewals.
        // Each cycle still has a distinct request and can receive funds only once.
        if let Some(first) = Attempt::first(
            self,
            "WHERE subscription_id=? ORDER BY cycle,attempt LIMIT 1",
            &[&id],
        )? {
            let original = Payment::get(self, first.payment_id)?;
            if payment.bank_account != original.bank_account {
                return Err(state(
                    "Subscription receiving account changed; review bank instructions before renewal",
                ));
            }
            payment = PaymentWrite {
                bank_reference: original.bank_reference.into(),
                ..Default::default()
            }
            .update(self, payment.id)?;
        }
        AttemptWrite {
            subscription_id: id.into(),
            payment_id: payment.id.into(),
            cycle: cycle.into(),
            attempt: attempt.into(),
            request_key: request.to_owned().into(),
            ..Default::default()
        }
        .insert(self)?;
        Ok(payment)
    }
    pub(crate) fn cap_cancel_subscription_payments(
        &self,
        id: i64,
        auth: &AuthContext,
    ) -> Result<()> {
        for link in Attempt::select(self, "WHERE subscription_id=?", &[&id])? {
            self.cap_cancel_payment(link.payment_id, auth)?;
        }
        Ok(())
    }
    pub fn b2c_cancel_subscription(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
    ) -> Result<Value> {
        let input: Cancel = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let old = self.owned::<Subscription>(id, "subscriptions", "subscriptions.cancel", auth)?;
        if old.status == Status::Cancelled {
            return Err(state("Subscription is already cancelled"));
        }
        let now = Utc::now();
        let future = old
            .current_period_end
            .as_deref()
            .map(instant)
            .transpose()?
            .is_some_and(|t| t > now);
        let changes = if input.at_period_end && future {
            SubscriptionWrite {
                cancel_at_period_end: 1.into(),
                ..Default::default()
            }
        } else {
            SubscriptionWrite {
                status: Status::Cancelled.into(),
                cancel_at_period_end: 0.into(),
                ..Default::default()
            }
        };
        self.cap_cancel_subscription_payments(id, auth)?;
        let next = changes.update(self, id)?;
        next.audit(self, "subscriptions.cancel", auth, Some(&old))?;
        self.cap_publish(
            Event::Subscription {
                before: old,
                after: next.clone(),
            },
            auth,
            now,
        )?;
        let result = self.cap_subscription_view(next)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub fn b2c_renew_due(&self, auth: &AuthContext) -> Result<Value> {
        self.b2c_renew_due_at(auth, Utc::now())
    }
    pub fn b2c_renew_due_at(&self, auth: &AuthContext, now: DateTime<Utc>) -> Result<Value> {
        self.cap_command("subscriptions", "subscriptions.renew", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let sql = format!(
            "SELECT s.* FROM {} s WHERE s.status IN ('active','past_due') AND s.current_period_end IS NOT NULL AND julianday(s.current_period_end)<=julianday(?) AND (s.cancel_at_period_end=1 OR NOT EXISTS (SELECT 1 FROM {} a WHERE a.subscription_id=s.id AND a.cycle=s.cycle+1)) ORDER BY s.current_period_end,s.id LIMIT 100",
            quote(Subscription::TABLE),
            quote(Attempt::TABLE)
        );
        let rows = Subscription::query(self, &sql, &[&now.to_rfc3339()])?;
        let mut payments = Vec::new();
        let mut cancelled = 0;
        for old in rows {
            let id = old.id;
            self.cap_publish(
                Event::SubscriptionDue {
                    before: old.clone(),
                    after: old,
                },
                auth,
                now,
            )?;
            let old = Subscription::get(self, id)?;
            if old.status == Status::Cancelled {
                cancelled += 1;
                continue;
            }
            if !matches!(old.status, Status::Active | Status::PastDue) {
                continue;
            }
            let cycle = old
                .cycle
                .checked_add(1)
                .ok_or_else(|| invalid("Cycle overflow"))?;
            if Attempt::first(
                self,
                "WHERE subscription_id=? AND cycle=? ORDER BY attempt DESC LIMIT 1",
                &[&id, &cycle],
            )?
            .is_none()
            {
                payments.push(self.cap_subscription_attempt(
                    &old,
                    cycle,
                    0,
                    &format!("cycle:{cycle}"),
                    auth,
                    now,
                )?);
            }
        }
        let payments = payments
            .iter()
            .map(|p| self.payment_view(p))
            .collect::<Result<Vec<_>>>()?;
        crate::service::commit(tx)?;
        Ok(json!({"payments":payments,"cancelled":cancelled}))
    }
}
