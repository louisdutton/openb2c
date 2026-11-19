use super::*;
use serde::Deserialize;

/// A payment adapter supplies provider references; settlement arrives as events.
/// The only adapter shipped here is local and cannot contact a payment network.
pub trait PaymentProvider {
    fn name(&self) -> &'static str;
    fn create_reference(&self) -> Result<String>;
}
pub struct FakePaymentProvider;
impl PaymentProvider for FakePaymentProvider {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn create_reference(&self) -> Result<String> {
        crate::identity::secret("fake_")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePayment {
    idempotency_key: String,
    amount_minor: i64,
    currency: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Simulate {
    event_id: String,
    outcome: String,
}

impl Service {
    pub fn b2c_pending_payments(&self, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.simulate", auth)?;
        let items = self.query(
            &format!(
                "SELECT * FROM {} WHERE status='pending' ORDER BY id LIMIT 100",
                quote(self.cap_table("payments", "payment")?)
            ),
            &[],
        )?;
        let items = items
            .into_iter()
            .map(|row| self.cap_response(self.cap_table("payments", "payment")?, row))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"items":items}))
    }
    pub(super) fn cap_payment(
        &self,
        user: i64,
        amount: i64,
        currency: &str,
        request_key: &str,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        let table = self.cap_table("payments", "payment")?;
        if amount < 1 || currency.len() != 3 || !currency.bytes().all(|c| c.is_ascii_uppercase()) {
            return Err(invalid(
                "Payment requires a positive integer amount_minor and a three-letter currency",
            ));
        }
        let rows = self.query(
            &format!(
                "SELECT * FROM {} WHERE user_id=? AND request_key=?",
                quote(table)
            ),
            &[SqlValue::Integer(user), SqlValue::Text(request_key.into())],
        )?;
        if let Some(existing) = rows.into_iter().next() {
            if existing["amount_minor"] != amount || existing["currency"] != currency {
                return Err(ServiceError::new(
                    "conflict",
                    "Payment idempotency key has different amount or currency",
                ));
            }
            return Ok(existing);
        }
        if self.cap_setting("payments", "provider") != Some("bank_transfer") {
            return Err(invalid("Only bank transfer composition is supported"));
        }
        let provider = FakePaymentProvider;
        let row=self.cap_insert(table,json!({"user_id":user,"amount_minor":amount,"currency":currency,"provider":provider.name(),"reference":provider.create_reference()?,"status":"pending","request_key":request_key,"created_at":now.to_rfc3339()}))?;
        self.cap_audit(table, "payments.create", auth, None, &row)?;
        Ok(row)
    }
    pub fn b2c_create_payment(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.create", auth)?;
        let user = self.cap_customer(auth)?;
        let input: CreatePayment = decode(input)?;
        key(&input.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        let payment = self.cap_payment(
            user,
            input.amount_minor,
            &input.currency,
            &format!("standalone:{}", input.idempotency_key),
            auth,
            Utc::now(),
        )?;
        let result = self.cap_response(self.cap_table("payments", "payment")?, payment)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub fn b2c_payment(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_response(
            self.cap_table("payments", "payment")?,
            self.cap_owned("payments", "payment", id, "payments.read", auth)?,
        )
    }
    pub fn b2c_simulate_payment(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.b2c_simulate_payment_at(id, input, auth, Utc::now())
    }
    /// Clock-injected entry point for deterministic local-provider tests.
    pub fn b2c_simulate_payment_at(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        self.cap_command("payments", "payments.simulate", auth)?;
        let input: Simulate = decode(input)?;
        key(&input.event_id)?;
        if !["succeeded", "failed", "refunded"].contains(&input.outcome.as_str()) {
            return Err(invalid("Unknown simulated outcome"));
        }
        let tx = crate::service::transaction(&self.db)?;
        let events = self.cap_table("payments", "event")?;
        if let Some(previous) =
            self.cap_find(events, "event_key", SqlValue::Text(input.event_id.clone()))?
        {
            if previous["payment_id"] != id || previous["outcome"] != input.outcome {
                return Err(ServiceError::new(
                    "conflict",
                    "Event ID is already bound to another outcome or payment",
                ));
            }
            let mut result: Value = serde_json::from_str(text(&previous, "result_json")?)
                .map_err(|_| invalid("Invalid recorded payment event"))?;
            self.redact(self.cap_table("payments", "payment")?, &mut result);
            return Ok(result);
        }
        let table = self.cap_table("payments", "payment")?;
        let old = self.raw(table, &json!(id))?;
        let current = text(&old, "status")?;
        let changed = current != input.outcome;
        if changed
            && !matches!(
                (current, input.outcome.as_str()),
                ("pending", "succeeded" | "failed") | ("succeeded", "refunded")
            )
        {
            return Err(state("Payment outcome conflicts with the current state"));
        }
        let payment = if changed {
            let payment = self.cap_update(table, id, json!({"status":input.outcome}))?;
            self.cap_audit(table, "payments.simulate", auth, Some(&old), &payment)?;
            self.cap_publish(
                Event::Payment {
                    before: old,
                    after: payment.clone(),
                },
                auth,
                now,
            )?;
            payment
        } else {
            old
        };
        let payment = self.cap_response(table, payment)?;
        self.cap_insert(events,json!({"payment_id":id,"event_key":input.event_id,"outcome":input.outcome,"result_json":payment.to_string(),"created_at":now.to_rfc3339()}))?;
        crate::service::commit(tx)?;
        Ok(payment)
    }
    pub(crate) fn cap_cancel_payment(&self, id: i64, auth: &AuthContext) -> Result<()> {
        let table = self.cap_table("payments", "payment")?;
        let old = self.raw(table, &json!(id))?;
        if old["status"] == "pending" {
            let payment = self.cap_update(table, id, json!({"status":"cancelled"}))?;
            self.cap_audit(table, "payments.cancel", auth, Some(&old), &payment)?;
        }
        Ok(())
    }
}
