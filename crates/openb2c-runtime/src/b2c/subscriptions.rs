use super::*;
use chrono::Months;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSubscription {
    idempotency_key: String,
    price_id: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retry {
    idempotency_key: String,
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

pub(super) struct Subscriptions;
impl Consumer for Subscriptions {
    fn capability(&self) -> &'static str {
        "subscriptions"
    }
    fn handle(
        &self,
        service: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Event::Payment {
            before: _,
            after: payment,
        } = event
        else {
            return Ok(());
        };
        let links = service.cap_table("subscriptions", "attempt")?;
        let Some(link) = service.cap_find(links, "payment_id", sql_value(&payment["id"])?)? else {
            return Ok(());
        };
        let table = service.cap_table("subscriptions", "subscription")?;
        let old = service.raw(table, &link["subscription_id"])?;
        if old["user_id"] != payment["user_id"]
            || old["amount_minor"] != payment["amount_minor"]
            || old["currency"] != payment["currency"]
        {
            return Err(state("Payment does not match its subscription"));
        }
        let id = integer(&old, "id")?;
        let next = match text(payment, "status")? {
            "succeeded" => {
                if old["status"] == "cancelled"
                    || Some(integer(&link, "cycle")?) != integer(&old, "cycle")?.checked_add(1)
                {
                    return Err(state("Subscription no longer accepts this billing cycle"));
                }
                let cycle = integer(&link, "cycle")?;
                let anchor = old["started_at"]
                    .as_str()
                    .map(instant)
                    .transpose()?
                    .unwrap_or(now);
                let months = match text(&old, "billing_interval")? {
                    "month" => 1u32,
                    "year" => 12u32,
                    _ => return Err(invalid("Unsupported billing interval")),
                };
                let count = cycle
                    .checked_add(1)
                    .and_then(|c| u32::try_from(c).ok())
                    .and_then(|c| c.checked_mul(months))
                    .ok_or_else(|| invalid("Billing cycle overflow"))?;
                // Derive every boundary from the original anchor, preserving month-end/leap-day schedules.
                let end = anchor
                    .checked_add_months(Months::new(count))
                    .ok_or_else(|| invalid("Billing date overflow"))?;
                let start = if cycle == 0 {
                    anchor.to_rfc3339()
                } else {
                    text(&old, "current_period_end")?.to_string()
                };
                service.cap_update(table,id,json!({"status":"active","cycle":cycle,"started_at":anchor.to_rfc3339(),"current_period_start":start,"current_period_end":end.to_rfc3339()}))?
            }
            "failed" => {
                if old["status"] == "cancelled" {
                    return Ok(());
                }
                service.cap_update(table, id, json!({"status":"past_due"}))?
            }
            "refunded" => {
                // Full refunds cancel the agreement; historical billing attempts remain immutable.
                service.cap_cancel_subscription_payments(id, auth)?;
                service.cap_update(
                    table,
                    id,
                    json!({"status":"cancelled","cancel_at_period_end":0}),
                )?
            }
            _ => return Ok(()),
        };
        service.cap_audit(table, "subscriptions.payment", auth, Some(&old), &next)?;
        service.cap_publish(
            Event::Subscription {
                before: old,
                after: next,
            },
            auth,
            now,
        )
    }
}

impl Service {
    pub fn b2c_create_subscription(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("subscriptions", "subscriptions.create", auth)?;
        let user = self.cap_customer(auth)?;
        let input: CreateSubscription = decode(input)?;
        key(&input.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("subscriptions", "subscription")?;
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
        if let Some(row) = existing.into_iter().next() {
            if row["price_id"] != input.price_id {
                return Err(ServiceError::new(
                    "conflict",
                    "Subscription key already used with a different price",
                ));
            }
            return self.cap_subscription_view(row);
        }
        let price = self.cap_price(input.price_id)?;
        if !["month", "year"].contains(&price.billing_interval.as_str()) {
            return Err(invalid("Subscriptions require a monthly or yearly price"));
        }
        let now = Utc::now();
        let row=self.cap_insert(table,json!({"user_id":user,"price_id":price.id,"product_code":price.product_code,"amount_minor":price.amount_minor,"currency":price.currency,"billing_interval":price.billing_interval,"status":"pending","cancel_at_period_end":0,"cycle":-1,"request_key":input.idempotency_key,"created_at":now.to_rfc3339()}))?;
        self.cap_subscription_attempt(&row, 0, 0, "initial", auth, now)?;
        self.cap_audit(table, "subscriptions.create", auth, None, &row)?;
        let result = self.cap_subscription_view(row)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub(super) fn cap_subscription_view(&self, subscription: Value) -> Result<Value> {
        let attempts = self.query(
            &format!(
                "SELECT * FROM {} WHERE subscription_id=? ORDER BY cycle DESC,attempt DESC LIMIT 1",
                quote(self.cap_table("subscriptions", "attempt")?)
            ),
            &[sql_value(&subscription["id"])?],
        )?;
        let payment = attempts
            .first()
            .map(|a| self.raw(self.cap_table("payments", "payment")?, &a["payment_id"]))
            .transpose()?;
        self.cap_subscription_response(subscription, payment)
    }
    fn cap_subscription_response(
        &self,
        subscription: Value,
        payment: Option<Value>,
    ) -> Result<Value> {
        let subscription = self.cap_response(
            self.cap_table("subscriptions", "subscription")?,
            subscription,
        )?;
        let payment = payment
            .map(|row| self.cap_response(self.cap_table("payments", "payment")?, row))
            .transpose()?;
        Ok(json!({"subscription":subscription,"payment":payment}))
    }
    pub fn b2c_subscription(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_subscription_view(self.cap_owned(
            "subscriptions",
            "subscription",
            id,
            "subscriptions.read",
            auth,
        )?)
    }
    fn cap_subscription_attempt(
        &self,
        subscription: &Value,
        cycle: i64,
        attempt: i64,
        request: &str,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        let id = integer(subscription, "id")?;
        let payment = self.cap_payment(
            integer(subscription, "user_id")?,
            integer(subscription, "amount_minor")?,
            text(subscription, "currency")?,
            &format!("subscription:{id}:{cycle}:{attempt}"),
            auth,
            now,
        )?;
        self.cap_insert(self.cap_table("subscriptions","attempt")?,json!({"subscription_id":id,"payment_id":payment["id"],"cycle":cycle,"attempt":attempt,"request_key":request}))?;
        Ok(payment)
    }
    pub fn b2c_retry_subscription(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command("subscriptions", "subscriptions.retry", auth)?;
        let input: Retry = decode(input)?;
        key(&input.idempotency_key)?;
        let request = format!("retry:{}", input.idempotency_key);
        let tx = crate::service::transaction(&self.db)?;
        let row = self.cap_owned(
            "subscriptions",
            "subscription",
            id,
            "subscriptions.retry",
            auth,
        )?;
        let links = self.cap_table("subscriptions", "attempt")?;
        let previous = self.query(
            &format!(
                "SELECT * FROM {} WHERE subscription_id=? AND request_key=?",
                quote(links)
            ),
            &[SqlValue::Integer(id), SqlValue::Text(request.clone())],
        )?;
        if let Some(link) = previous.first() {
            return self.cap_subscription_response(
                row,
                Some(self.raw(self.cap_table("payments", "payment")?, &link["payment_id"])?),
            );
        }
        if row["status"] != "past_due" || row["cancel_at_period_end"] == 1 {
            return Err(state("Only past-due, continuing subscriptions can retry"));
        }
        let latest=self.query(&format!("SELECT * FROM {} WHERE subscription_id=? ORDER BY cycle DESC,attempt DESC LIMIT 1",quote(links)),&[SqlValue::Integer(id)])?.into_iter().next().ok_or_else(||state("Subscription has no billing attempt"))?;
        let payment = self.raw(
            self.cap_table("payments", "payment")?,
            &latest["payment_id"],
        )?;
        if payment["status"] != "failed" {
            return Err(state(
                "An existing billing attempt must finish before retrying",
            ));
        }
        let payment = self.cap_subscription_attempt(
            &row,
            integer(&latest, "cycle")?,
            integer(&latest, "attempt")?
                .checked_add(1)
                .ok_or_else(|| invalid("Attempt overflow"))?,
            &request,
            auth,
            Utc::now(),
        )?;
        let result = self.cap_subscription_response(row, Some(payment))?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub(super) fn cap_cancel_subscription_payments(
        &self,
        id: i64,
        auth: &AuthContext,
    ) -> Result<()> {
        let links = self.cap_table("subscriptions", "attempt")?;
        for link in self.query(
            &format!("SELECT * FROM {} WHERE subscription_id=?", quote(links)),
            &[SqlValue::Integer(id)],
        )? {
            self.cap_cancel_payment(integer(&link, "payment_id")?, auth)?;
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
        let old = self.cap_owned(
            "subscriptions",
            "subscription",
            id,
            "subscriptions.cancel",
            auth,
        )?;
        if old["status"] == "cancelled" {
            return Err(state("Subscription is already cancelled"));
        }
        let now = Utc::now();
        let future = old["current_period_end"]
            .as_str()
            .map(instant)
            .transpose()?
            .is_some_and(|t| t > now);
        let changes = if input.at_period_end && future {
            json!({"cancel_at_period_end":1})
        } else {
            json!({"status":"cancelled","cancel_at_period_end":0})
        };
        self.cap_cancel_subscription_payments(id, auth)?;
        let table = self.cap_table("subscriptions", "subscription")?;
        let next = self.cap_update(table, id, changes)?;
        self.cap_audit(table, "subscriptions.cancel", auth, Some(&old), &next)?;
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
    /// The HTTP adapter always uses server time; tests may advance this clock.
    pub fn b2c_renew_due_at(&self, auth: &AuthContext, now: DateTime<Utc>) -> Result<Value> {
        self.cap_command("subscriptions", "subscriptions.renew", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("subscriptions", "subscription")?;
        let attempts = self.cap_table("subscriptions", "attempt")?;
        let rows=self.query(&format!("SELECT s.* FROM {} s WHERE s.status IN ('active','past_due') AND s.current_period_end IS NOT NULL AND julianday(s.current_period_end)<=julianday(?) AND (s.cancel_at_period_end=1 OR NOT EXISTS (SELECT 1 FROM {} a WHERE a.subscription_id=s.id AND a.cycle=s.cycle+1)) ORDER BY s.current_period_end,s.id LIMIT 100",quote(table),quote(attempts)),&[SqlValue::Text(now.to_rfc3339())])?;
        let mut payments = Vec::new();
        let mut cancelled = 0;
        for old in rows {
            let id = integer(&old, "id")?;
            if old["cancel_at_period_end"] == 1 {
                self.cap_cancel_subscription_payments(id, auth)?;
                let next = self.cap_update(
                    table,
                    id,
                    json!({"status":"cancelled","cancel_at_period_end":0}),
                )?;
                self.cap_audit(table, "subscriptions.expire", auth, Some(&old), &next)?;
                self.cap_publish(
                    Event::Subscription {
                        before: old,
                        after: next,
                    },
                    auth,
                    now,
                )?;
                cancelled += 1;
                continue;
            }
            let cycle = integer(&old, "cycle")?
                .checked_add(1)
                .ok_or_else(|| invalid("Cycle overflow"))?;
            let links = self.cap_table("subscriptions", "attempt")?;
            let previous=self.query(&format!("SELECT * FROM {} WHERE subscription_id=? AND cycle=? ORDER BY attempt DESC LIMIT 1",quote(links)),&[SqlValue::Integer(id),SqlValue::Integer(cycle)])?;
            if previous.is_empty() {
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
            .into_iter()
            .map(|row| self.cap_response(self.cap_table("payments", "payment")?, row))
            .collect::<Result<Vec<_>>>()?;
        crate::service::commit(tx)?;
        Ok(json!({"payments":payments,"cancelled":cancelled}))
    }
}
