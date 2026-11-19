//! Typed accepted-sale facts and durable posting commands.
use super::accounting::Line;
use super::billing::Invoice;
use super::*;
use crate::records::{
    AccountsAccount, BillingEvent as EventRow, BillingEventStatus, BillingEventWrite,
    BillingInvoice, Record,
};
use chrono::{DateTime, Utc};
pub(crate) struct SaleFacts {
    pub user_id: i64,
    pub amount_minor: i64,
    pub currency: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(super) enum BillingTask {
    Cash {
        source: String,
        amount_minor: i64,
        currency: String,
        debit: String,
        credit: String,
    },
    Sale {
        source: String,
        invoice: Invoice,
        inclusive_vat: i64,
        #[serde(default)]
        cash_account: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        historical_backfill: bool,
    },
    Refund {
        source: String,
        #[serde(default)]
        cash_account: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        historical_backfill: bool,
    },
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl BillingTask {
    fn source(&self) -> &str {
        match self {
            Self::Sale { source, .. } | Self::Refund { source, .. } | Self::Cash { source, .. } => {
                source
            }
        }
    }
    fn kind(&self) -> &str {
        match self {
            Self::Cash { .. } => "cash",
            Self::Sale { .. } => "sale",
            Self::Refund { .. } => "refund",
        }
    }
}
impl Service {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture_sale(
        &self,
        source: &str,
        sale: SaleFacts,
        description: &str,
        period: Option<(String, String)>,
        cash_account: Option<String>,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let mut payload = self.sale_task(source, &sale, description, period, now)?;
        if let BillingTask::Sale {
            cash_account: cash, ..
        } = &mut payload
        {
            *cash = cash_account;
        }
        self.billing_queue(payload, auth, now)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture_cash(
        &self,
        source: &str,
        amount_minor: i64,
        currency: &str,
        debit: &str,
        credit: &str,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if currency != "GBP" || amount_minor <= 0 || debit == credit {
            return Err(invalid("Invalid allocated cash posting"));
        }
        money(amount_minor)?;
        self.billing_queue(
            BillingTask::Cash {
                source: source.into(),
                amount_minor,
                currency: currency.into(),
                debit: debit.into(),
                credit: credit.into(),
            },
            auth,
            now,
        )
    }
    pub(crate) fn capture_refund(
        &self,
        source: &str,
        cash_account: Option<String>,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.billing_queue(
            BillingTask::Refund {
                source: source.into(),
                cash_account,
                historical_backfill: false,
            },
            auth,
            now,
        )
    }
    pub(super) fn sale_task(
        &self,
        source: &str,
        sale: &SaleFacts,
        summary: &str,
        period: Option<(String, String)>,
        now: DateTime<Utc>,
    ) -> Result<BillingTask> {
        if sale.currency != "GBP" {
            return Err(invalid("UK accounting supports GBP sales only"));
        }
        let account = AccountsAccount::get(self, sale.user_id)?;
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
            customer: format!("{} <{}>", account.name, account.email),
            description: summary.into(),
            date: day.clone(),
            due_date: day,
            net_minor: gross - vat,
            tax_code: code.into(),
            period_start: period.as_ref().map(|p| p.0.clone()),
            period_end: period.map(|p| p.1),
            posting_date: None,
        };
        Ok(BillingTask::Sale {
            source: source.into(),
            invoice,
            inclusive_vat: vat,
            cash_account: None,
            historical_backfill: false,
        })
    }
    pub(super) fn billing_queue(
        &self,
        payload: BillingTask,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let source = format!("{}:{}", payload.kind(), payload.source());
        if EventRow::first(self, "WHERE source_key=?", &[&source])?.is_some() {
            return Ok(());
        }
        let snapshot = serde_json::to_string(&payload).map_err(|e| invalid(e.to_string()))?;
        let row = BillingEventWrite {
            source_key: source.into(),
            payload_json: snapshot.into(),
            occurred_at: now.to_rfc3339().into(),
            status: BillingEventStatus::Pending.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "billing.capture", auth, None)?;
        if matches!(payload, BillingTask::Refund { .. })
            && BillingInvoice::first(self, "WHERE source_key=?", &[&payload.source()])?.is_none()
        {
            return Ok(());
        }
        let day = now.date_naive().to_string();
        match self.posted_date(&day) {
            Ok(()) => self.billing_apply(&row, payload, &day, auth),
            Err(e) if e.code == "bad_state" => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn billing_apply(
        &self,
        event: &EventRow,
        payload: BillingTask,
        day: &str,
        auth: &AuthContext,
    ) -> Result<()> {
        if event.status == BillingEventStatus::Posted {
            return Ok(());
        }
        let occurred = crate::b2c::instant(&event.occurred_at)?.date_naive();
        if date(day)? < occurred {
            return Err(invalid("Posting cannot precede the source event"));
        }
        match payload {
            BillingTask::Cash {
                source,
                amount_minor,
                currency,
                debit,
                credit,
            } => {
                if currency != "GBP" {
                    return Err(invalid("UK accounting supports GBP only"));
                }
                self.post(
                    &format!("cash:{source}"),
                    day,
                    "Allocated bank transfer",
                    vec![
                        Line::debit(&debit, amount_minor),
                        Line::credit(&credit, amount_minor),
                    ],
                    None,
                    auth,
                )?;
            }
            BillingTask::Sale {
                source,
                mut invoice,
                inclusive_vat,
                cash_account,
                ..
            } => {
                invoice.posting_date = Some(day.into());
                let row = self.issue(&source, &invoice, Some(inclusive_vat), auth)?;
                self.receive_from(row.id, day, cash_account.as_deref().unwrap_or("bank"), auth)?;
            }
            BillingTask::Refund {
                source,
                cash_account,
                ..
            } => {
                let invoice = BillingInvoice::first(self, "WHERE source_key=?", &[&source])?
                    .ok_or_else(|| state("Original sale must be posted before its refund"))?;
                self.credit_invoice(invoice.id, day, "Full payment refund", auth)?;
                self.refund_invoice_from(
                    invoice.id,
                    day,
                    "Confirmed bank transfer refund or reversal",
                    cash_account.as_deref(),
                    auth,
                )?;
            }
        }
        let posted = BillingEventWrite {
            status: BillingEventStatus::Posted.into(),
            ..Default::default()
        }
        .update(self, event.id)?;
        posted.audit(self, "billing.post_event", auth, Some(event))?;
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
        let rows = EventRow::select(self, "WHERE status='pending' ORDER BY id", &[])?;
        for row in &rows {
            let payload: BillingTask = serde_json::from_str(&row.payload_json)
                .map_err(|_| invalid("Invalid saved billing event"))?;
            self.billing_apply(row, payload, &i.date, auth)?;
        }
        crate::service::commit(tx)?;
        Ok(json!({"posted":rows.len()}))
    }
}
