//! Billing consumes accepted sale events. Closed periods defer posting, never erase payment facts.
use super::billing::Invoice;
use super::*;
#[cfg(feature = "reference")]
use crate::b2c::{Consumer, Event};
use chrono::{DateTime, Utc};
#[cfg(feature = "reference")]
pub(crate) struct OrderBilling;
#[cfg(feature = "reference")]
pub(crate) struct SubscriptionBilling;
pub(crate) struct SaleFacts {
    pub user_id: i64,
    pub amount_minor: i64,
    pub currency: String,
}
impl SaleFacts {
    fn from_row(row: &Value) -> Result<Self> {
        Ok(Self {
            user_id: integer(row, "user_id")?,
            amount_minor: integer(row, "amount_minor")?,
            currency: text(row, "currency")?.into(),
        })
    }
}
impl Service {
    #[cfg(not(feature = "reference"))]
    pub(crate) fn capture_sale(
        &self,
        source: &str,
        sale: SaleFacts,
        description: &str,
        period: Option<(String, String)>,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let payload = self.sale_task(source, &sale, description, period, now)?;
        self.billing_queue(payload, auth, now)
    }
    fn sale_task(
        &self,
        source: &str,
        sale: &SaleFacts,
        summary: &str,
        period: Option<(String, String)>,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        if sale.currency != "GBP" {
            return Err(invalid("UK accounting supports GBP sales only"));
        }
        let account = self.raw("user", &json!(sale.user_id))?;
        let code = self
            .cap_setting("billing", "defaultTaxCode")
            .ok_or_else(|| invalid("Missing sales VAT treatment"))?;
        let gross = sale.amount_minor;
        let rate = match code {
            "standard" => 20,
            "reduced" => 5,
            _ => 0,
        };
        let vat = ((i128::from(gross) * rate + (100 + rate) / 2) / (100 + rate)) as i64;
        let day = now.date_naive().to_string();
        let invoice = Invoice {
            idempotency_key: source.into(),
            customer: format!("{} <{}>", text(&account, "name")?, text(&account, "email")?),
            description: summary.into(),
            date: day.clone(),
            due_date: day,
            net_minor: gross - vat,
            tax_code: code.into(),
            period_start: period.as_ref().map(|p| p.0.clone()),
            period_end: period.map(|p| p.1),
            posting_date: None,
        };
        Ok(json!({"kind":"sale","source":source,"invoice":invoice,"inclusive_vat":vat}))
    }
    pub(crate) fn billing_queue(
        &self,
        payload: Value,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let source = format!("{}:{}", text(&payload, "kind")?, text(&payload, "source")?);
        let table = self.office_table("billing", "event")?;
        if self
            .cap_find(table, "source_key", SqlValue::Text(source.clone()))?
            .is_some()
        {
            return Ok(());
        }
        let row=self.cap_insert(table,json!({"source_key":source,"payload_json":payload.to_string(),"occurred_at":now.to_rfc3339(),"status":"pending"}))?;
        self.cap_audit(table, "billing.capture", auth, None, &row)?;
        if payload["kind"] == "refund"
            && self
                .cap_find(
                    self.office_table("billing", "invoice")?,
                    "source_key",
                    SqlValue::Text(text(&payload, "source")?.into()),
                )?
                .is_none()
        {
            return Ok(());
        }
        let day = now.date_naive().to_string();
        // Accepted provider facts survive a closed ledger; an operator posts them in an open period.
        match self.posted_date(&day) {
            Ok(()) => self.billing_apply(&row, &day, auth),
            Err(e) if e.code == "bad_state" => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn billing_apply(&self, event: &Value, day: &str, auth: &AuthContext) -> Result<()> {
        if event["status"] == "posted" {
            return Ok(());
        }
        let payload: Value = serde_json::from_str(text(event, "payload_json")?)
            .map_err(|_| invalid("Invalid saved billing event"))?;
        let occurred = crate::b2c::instant(text(event, "occurred_at")?)?.date_naive();
        if date(day)? < occurred {
            return Err(invalid("Posting cannot precede the source event"));
        }
        let source = text(&payload, "source")?;
        match text(&payload, "kind")? {
            "sale" => {
                let mut invoice: Invoice = decode(&payload["invoice"])?;
                invoice.posting_date = Some(day.into());
                let row = self.issue(
                    source,
                    &invoice,
                    Some(integer(&payload, "inclusive_vat")?),
                    auth,
                )?;
                self.receive(integer(&row, "id")?, day, auth)?;
            }
            "refund" => {
                let invoice = self
                    .cap_find(
                        self.office_table("billing", "invoice")?,
                        "source_key",
                        SqlValue::Text(source.into()),
                    )?
                    .ok_or_else(|| state("Original sale must be posted before its refund"))?;
                let id = integer(&invoice, "id")?;
                self.credit_invoice(id, day, "Full payment refund", auth)?;
                self.refund_invoice(id, day, "Confirmed payment provider refund", auth)?;
            }
            _ => return Err(invalid("Unknown saved billing event")),
        }
        let table = self.office_table("billing", "event")?;
        let posted = self.cap_update(table, integer(event, "id")?, json!({"status":"posted"}))?;
        self.cap_audit(table, "billing.post_event", auth, Some(event), &posted)?;
        Ok(())
    }
    pub fn backoffice_post_billing_events(
        &self,
        input: &Value,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command("billing", "billing.write", auth)?;
        let i: Dated = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        self.posted_date(&i.date)?;
        let table = self.office_table("billing", "event")?;
        let rows = self.query(
            &format!(
                "SELECT * FROM {} WHERE status='pending' ORDER BY id",
                quote(table)
            ),
            &[],
        )?;
        for row in &rows {
            self.billing_apply(row, &i.date, auth)?;
        }
        crate::service::commit(tx)?;
        Ok(json!({"posted":rows.len()}))
    }
}
#[cfg(feature = "reference")]
impl Consumer for OrderBilling {
    fn enabled(&self, s: &Service) -> bool {
        s.schema.capabilities.contains_key("billing")
            && s.schema.capabilities.contains_key("orders")
            && s.schema.capabilities.contains_key("order_payments")
    }
    fn capability(&self) -> &'static str {
        "billing_orders"
    }
    fn handle(
        &self,
        s: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Event::Order { after: order, .. } = event else {
            return Ok(());
        };
        let source = format!("order:{}", integer(order, "id")?);
        if !s.schema.capabilities.contains_key("billing_orders")
            && (order["status"] != "refunded"
                || s.cap_find(
                    s.office_table("billing", "invoice")?,
                    "source_key",
                    SqlValue::Text(source.clone()),
                )?
                .is_none())
        {
            return Ok(());
        }
        let payload = match text(order, "status")? {
            "paid" => s.sale_task(
                &source,
                &SaleFacts::from_row(order)?,
                &format!("Order {}", order["id"]),
                None,
                now,
            )?,
            "refunded" => json!({"kind":"refund","source":source}),
            _ => return Ok(()),
        };
        s.billing_queue(payload, auth, now)
    }
}
#[cfg(feature = "reference")]
impl Consumer for SubscriptionBilling {
    fn enabled(&self, s: &Service) -> bool {
        s.schema.capabilities.contains_key("billing")
            && s.schema.capabilities.contains_key("subscriptions")
    }
    fn capability(&self) -> &'static str {
        "billing_subscriptions"
    }
    fn handle(
        &self,
        s: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Event::Payment { after: payment, .. } = event else {
            return Ok(());
        };
        let Some(link) = s.cap_find(
            s.cap_table("subscriptions", "attempt")?,
            "payment_id",
            sql_value(&payment["id"])?,
        )?
        else {
            return Ok(());
        };
        let source = format!("subscription-payment:{}", payment["id"]);
        if !s.schema.capabilities.contains_key("billing_subscriptions")
            && (payment["status"] != "refunded"
                || s.cap_find(
                    s.office_table("billing", "invoice")?,
                    "source_key",
                    SqlValue::Text(source.clone()),
                )?
                .is_none())
        {
            return Ok(());
        }
        let payload = match text(payment, "status")? {
            "succeeded" => {
                let sub = s.raw(
                    s.cap_table("subscriptions", "subscription")?,
                    &link["subscription_id"],
                )?;
                let start = crate::b2c::instant(text(&sub, "current_period_start")?)?
                    .date_naive()
                    .to_string();
                let end = crate::b2c::instant(text(&sub, "current_period_end")?)?
                    .date_naive()
                    .to_string();
                s.sale_task(
                    &source,
                    &SaleFacts::from_row(payment)?,
                    &format!(
                        "Subscription {} cycle {}",
                        link["subscription_id"], link["cycle"]
                    ),
                    Some((start, end)),
                    now,
                )?
            }
            "refunded" => json!({"kind":"refund","source":source}),
            _ => return Ok(()),
        };
        s.billing_queue(payload, auth, now)
    }
}

impl Service {
    /// Reconstruct newly enabled integrations from durable source event history.
    pub(crate) fn backfill_billing(&self, old: &openb2c_schema::Schema) -> anyhow::Result<()> {
        use chrono::Months;
        let activate_orders = self.schema.capabilities.contains_key("billing_orders")
            && !old.capabilities.contains_key("billing_orders");
        let activate_subscriptions = self
            .schema
            .capabilities
            .contains_key("billing_subscriptions")
            && !old.capabilities.contains_key("billing_subscriptions");
        if !activate_orders && !activate_subscriptions {
            return Ok(());
        }
        let payments = self.cap_table("payments", "payment")?;
        let history = self.cap_table("payments", "event")?;
        let rows = self.query(
            &format!(
                "SELECT * FROM {} WHERE status IN ('succeeded','refunded') ORDER BY id",
                quote(payments)
            ),
            &[],
        )?;
        for payment in rows {
            let mut target = None;
            if activate_orders
                && let Some(link) = self.cap_find(
                    self.cap_table("order_payments", "link")?,
                    "payment_id",
                    sql_value(&payment["id"])?,
                )?
            {
                let order = self.raw(self.cap_table("orders", "order")?, &link["order_id"])?;
                target = Some((
                    format!("order:{}", order["id"]),
                    order,
                    format!("Historical order {}", link["order_id"]),
                    None,
                ));
            }
            if activate_subscriptions
                && let Some(link) = self.cap_find(
                    self.cap_table("subscriptions", "attempt")?,
                    "payment_id",
                    sql_value(&payment["id"])?,
                )?
            {
                let sub = self.raw(
                    self.cap_table("subscriptions", "subscription")?,
                    &link["subscription_id"],
                )?;
                let anchor = crate::b2c::instant(text(&sub, "started_at")?)?;
                let cycle = u32::try_from(integer(&link, "cycle")?)?;
                let factor = if sub["billing_interval"] == "year" {
                    12u32
                } else {
                    1
                };
                let start = anchor
                    .checked_add_months(Months::new(
                        cycle
                            .checked_mul(factor)
                            .ok_or_else(|| invalid("Billing cycle overflow"))?,
                    ))
                    .ok_or_else(|| invalid("Billing date overflow"))?;
                let end = anchor
                    .checked_add_months(Months::new(
                        cycle
                            .checked_add(1)
                            .and_then(|n| n.checked_mul(factor))
                            .ok_or_else(|| invalid("Billing cycle overflow"))?,
                    ))
                    .ok_or_else(|| invalid("Billing date overflow"))?;
                target = Some((
                    format!("subscription-payment:{}", payment["id"]),
                    payment.clone(),
                    format!(
                        "Historical subscription {} cycle {}",
                        link["subscription_id"], cycle
                    ),
                    Some((start.date_naive().to_string(), end.date_naive().to_string())),
                ));
            }
            let Some((source, sale, summary, period)) = target else {
                continue;
            };
            let events=self.query(&format!("SELECT * FROM {} WHERE payment_id=? AND outcome IN ('succeeded','refunded') ORDER BY id",quote(history)),&[sql_value(&payment["id"]) ?])?;
            let succeeded=events.iter().find(|e|e["outcome"]=="succeeded").ok_or_else(||invalid(format!("Historical payment {} has no settlement evidence; supply reviewed source history before enabling billing",payment["id"])))?;
            let when = crate::b2c::instant(text(succeeded, "created_at")?)?;
            let mut payload = self.sale_task(
                &source,
                &SaleFacts::from_row(&sale)?,
                &summary,
                period,
                when,
            )?;
            // No source tax assessment existed: do not retroactively apply today's sales policy.
            payload["invoice"]["tax_code"] = json!("unassessed");
            payload["invoice"]["net_minor"] = sale["amount_minor"].clone();
            payload["inclusive_vat"] = json!(0);
            payload["historical_backfill"] = json!(true);
            self.billing_queue(payload, &AuthContext::system(), when)?;
            if payment["status"] == "refunded" {
                let refund = events
                    .iter()
                    .find(|e| e["outcome"] == "refunded")
                    .ok_or_else(|| invalid("Historical refund has no source evidence"))?;
                self.billing_queue(
                    json!({"kind":"refund","source":source,"historical_backfill":true}),
                    &AuthContext::system(),
                    crate::b2c::instant(text(refund, "created_at")?)?,
                )?;
            }
        }
        Ok(())
    }
}
