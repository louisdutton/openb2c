use super::accounting::Line;
use super::*;
use crate::records::{
    AccountingAccount, AccountingLine, BillingCredit, BillingCreditWrite, BillingEvent,
    BillingInvoice as InvoiceRow, BillingInvoiceWrite, BillingReceipt, BillingReceiptWrite,
    BillingRefund, BillingRefundWrite, Record,
};
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Invoice {
    pub idempotency_key: String,
    pub customer: String,
    pub description: String,
    pub date: String,
    pub due_date: String,
    pub net_minor: i64,
    pub tax_code: String,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    #[serde(default)]
    pub posting_date: Option<String>,
}
fn gross(row: &InvoiceRow) -> Result<i64> {
    row.net_minor
        .checked_add(row.tax_minor)
        .ok_or_else(|| invalid("Amount overflow"))
}
impl Service {
    pub(super) fn issue(
        &self,
        source: &str,
        i: &Invoice,
        inclusive_vat: Option<i64>,
        auth: &AuthContext,
    ) -> Result<InvoiceRow> {
        let request = serde_json::to_value(i)
            .map_err(|e| invalid(e.to_string()))?
            .to_string();
        if let Some(row) = InvoiceRow::first(self, "WHERE source_key=?", &[&source])? {
            if row.request_json != request {
                return Err(ServiceError::new(
                    "conflict",
                    "Idempotency key already used with different details",
                ));
            }
            return Ok(row);
        }
        description(&i.customer)?;
        description(&i.description)?;
        let posting_date = i.posting_date.as_deref().unwrap_or(&i.date);
        date(&i.date)?;
        if posting_date < i.date.as_str() {
            return Err(invalid(
                "Posting date cannot precede the source invoice date",
            ));
        }
        self.posted_date(posting_date)?;
        if date(&i.due_date)? < date(&i.date)? {
            return Err(invalid("Due date cannot precede invoice date"));
        }
        let deferred = match (&i.period_start, &i.period_end) {
            (None, None) => false,
            (Some(start), Some(end)) if date(start)? < date(end)? => true,
            _ => {
                return Err(invalid(
                    "A service period requires a start and a later exclusive end",
                ));
            }
        };
        let calculated = tax(i.net_minor, &i.tax_code)?;
        let vat = inclusive_vat.unwrap_or(calculated);
        money(vat)?;
        if self.cap_setting("tax", "vatRegistration") == Some("not_registered") && vat > 0 {
            return Err(invalid(
                "A business that is not VAT registered cannot charge VAT",
            ));
        }
        let gross = i
            .net_minor
            .checked_add(vat)
            .ok_or_else(|| invalid("Amount overflow"))?;
        money(gross)?;
        if gross == 0 {
            return Err(invalid("An invoice must have a positive total"));
        }
        let j = self.post(
            &format!("invoice:{source}"),
            posting_date,
            &i.description,
            vec![
                Line::debit("receivables", gross),
                Line::credit(if deferred { "deferred" } else { "sales" }, i.net_minor),
                Line::credit("tax_output", vat),
            ],
            None,
            auth,
        )?;
        let row = BillingInvoiceWrite {
            source_key: source.to_owned().into(),
            request_json: request.into(),
            issuer: self.schema.organization.name.clone().into(),
            customer: i.customer.clone().into(),
            description: i.description.clone().into(),
            date: i.date.clone().into(),
            due_date: i.due_date.clone().into(),
            posting_date: Some(posting_date.to_owned()).into(),
            currency: "GBP".to_owned().into(),
            net_minor: i.net_minor.into(),
            tax_minor: vat.into(),
            tax_code: i.tax_code.clone().into(),
            period_start: i.period_start.clone().into(),
            period_end: i.period_end.clone().into(),
            recognized_minor: (if deferred { 0 } else { i.net_minor }).into(),
            journal_id: j.id.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "billing.issue", auth, None)?;
        Ok(row)
    }
    pub fn backoffice_invoice(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.write", auth)?;
        let i: Invoice = decode(input)?;
        key(&i.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        let row = self.issue(&format!("manual:{}", i.idempotency_key), &i, None, auth)?;
        crate::service::commit(tx)?;
        row.wire(self)
    }
    pub(super) fn invoice_row(&self, id: i64) -> Result<InvoiceRow> {
        InvoiceRow::get(self, id)
    }
    fn credit_row(&self, id: i64) -> Result<Option<BillingCredit>> {
        BillingCredit::first(self, "WHERE invoice_id=?", &[&id])
    }
    pub fn backoffice_invoice_view(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.read", auth)?;
        let invoice = self.invoice_row(id)?;
        let credit = self.credit_row(id)?;
        let receipt = BillingReceipt::first(self, "WHERE invoice_id=?", &[&id])?;
        let refund = BillingRefund::first(self, "WHERE invoice_id=?", &[&id])?;
        let outstanding = if credit.is_some() || receipt.is_some() {
            0
        } else {
            gross(&invoice)?
        };
        let refund_due = if credit.is_some() && receipt.is_some() && refund.is_none() {
            gross(&invoice)?
        } else {
            0
        };
        // Assemble the public representation after the typed calculation has finished.
        let mut response = invoice.wire(self)?;
        response["outstanding_minor"] = json!(outstanding);
        response["credit_note"] = json!(credit);
        response["receipt"] = json!(receipt);
        response["refund_due_minor"] = json!(refund_due);
        response["refund"] = json!(refund);
        response["number"] = json!(format!("INV-{id:06}"));
        Ok(response)
    }
    pub(super) fn receive(&self, id: i64, day: &str, auth: &AuthContext) -> Result<BillingReceipt> {
        self.receive_from(id, day, "bank", auth)
    }
    pub(super) fn receive_from(
        &self,
        id: i64,
        day: &str,
        cash_account: &str,
        auth: &AuthContext,
    ) -> Result<BillingReceipt> {
        let row = self.invoice_row(id)?;
        if let Some(old) = BillingReceipt::first(self, "WHERE invoice_id=?", &[&id])? {
            if old.date != day {
                return Err(state("Invoice was already received on a different date"));
            }
            return Ok(old);
        }
        if self.credit_row(id)?.is_some() {
            return Err(state("A credited invoice cannot receive payment"));
        }
        if date(day)? < date(&row.date)? {
            return Err(invalid("Receipt date cannot precede invoice date"));
        }
        let j = self.post(
            &format!("receipt:{id}"),
            day,
            "Invoice payment received",
            vec![
                Line::debit(cash_account, gross(&row)?),
                Line::credit("receivables", gross(&row)?),
            ],
            None,
            auth,
        )?;
        let r = BillingReceiptWrite {
            invoice_id: id.into(),
            date: day.to_owned().into(),
            journal_id: j.id.into(),
            ..Default::default()
        }
        .insert(self)?;
        r.audit(self, "billing.receive", auth, None)?;
        Ok(r)
    }
    pub fn backoffice_receive(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.write", auth)?;
        let i: Dated = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        self.receive(id, &i.date, auth)?;
        let row = self.backoffice_invoice_view(id, &AuthContext::system())?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub(super) fn credit_invoice(
        &self,
        id: i64,
        day: &str,
        reason: &str,
        auth: &AuthContext,
    ) -> Result<BillingCredit> {
        if let Some(row) = self.credit_row(id)? {
            if row.date != day || row.reason != reason {
                return Err(state("Invoice already has a different credit note"));
            }
            return Ok(row);
        }
        let invoice = self.invoice_row(id)?;
        let j = self.reverse_journal(invoice.journal_id, day, reason, auth)?;
        if invoice.period_start.is_some() && invoice.recognized_minor > 0 {
            self.post(
                &format!("credit-revenue:{id}"),
                day,
                "Reverse recognised subscription revenue",
                vec![
                    Line::debit("sales", invoice.recognized_minor),
                    Line::credit("deferred", invoice.recognized_minor),
                ],
                None,
                auth,
            )?;
        }
        let row = BillingCreditWrite {
            invoice_id: id.into(),
            date: day.to_owned().into(),
            reason: reason.to_owned().into(),
            journal_id: j.id.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "billing.credit", auth, None)?;
        Ok(row)
    }
    pub fn backoffice_credit(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.write", auth)?;
        let i: Reason = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        if !self.invoice_row(id)?.source_key.starts_with("manual:") {
            return Err(state(
                "Refund the original sale payment to credit this invoice",
            ));
        }
        self.credit_invoice(id, &i.date, &i.reason, auth)?;
        let row = self.backoffice_invoice_view(id, &AuthContext::system())?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub(super) fn refund_invoice(
        &self,
        id: i64,
        day: &str,
        reason: &str,
        auth: &AuthContext,
    ) -> Result<BillingRefund> {
        self.refund_invoice_from(id, day, reason, None, auth)
    }
    pub(super) fn refund_invoice_from(
        &self,
        id: i64,
        day: &str,
        reason: &str,
        cash_account: Option<&str>,
        auth: &AuthContext,
    ) -> Result<BillingRefund> {
        if let Some(old) = BillingRefund::first(self, "WHERE invoice_id=?", &[&id])? {
            if old.date != day || old.reason != reason {
                return Err(state("Invoice already has a different recorded refund"));
            }
            return Ok(old);
        }
        let invoice = self.invoice_row(id)?;
        let credit = self
            .credit_row(id)?
            .ok_or_else(|| state("Issue a credit note before recording a refund"))?;
        let receipt = BillingReceipt::first(self, "WHERE invoice_id=?", &[&id])?
            .ok_or_else(|| state("An unpaid invoice has no cash to refund"))?;
        if date(day)? < date(&credit.date)? || date(day)? < date(&receipt.date)? {
            return Err(invalid("Refund cannot precede its receipt or credit note"));
        }
        description(reason)?;
        // Refund the asset account that actually received this invoice. This
        // preserves historical clearing-account receipts across bank-feed upgrades.
        let cash = AccountingLine::first(
            self,
            "WHERE journal_id=? AND debit=?",
            &[&receipt.journal_id, &gross(&invoice)?],
        )?
        .ok_or_else(|| state("Receipt has no matching cash ledger line"))?;
        let cash = AccountingAccount::get(self, cash.account_id)?;
        let j = self.post(
            &format!("refund:{id}"),
            day,
            "Record confirmed invoice refund",
            vec![
                Line::debit("receivables", gross(&invoice)?),
                Line::credit(cash_account.unwrap_or(&cash.code), gross(&invoice)?),
            ],
            None,
            auth,
        )?;
        let row = BillingRefundWrite {
            invoice_id: id.into(),
            date: day.to_owned().into(),
            reason: reason.to_owned().into(),
            journal_id: j.id.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "billing.refund", auth, None)?;
        Ok(row)
    }
    pub fn backoffice_refund(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.refund", auth)?;
        let i: Reason = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        if !self.invoice_row(id)?.source_key.starts_with("manual:") {
            return Err(state("Record managed refunds through the original payment"));
        }
        self.refund_invoice(id, &i.date, &i.reason, auth)?;
        let row = self.backoffice_invoice_view(id, &AuthContext::system())?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub fn backoffice_recognize(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        if BillingEvent::first(self, "WHERE status='pending'", &[])?.is_some() {
            return Err(state(
                "Post captured billing events before recognising revenue",
            ));
        }
        self.cap_command("billing", "billing.write", auth)?;
        let i: Through = decode(input)?;
        let through = date(&i.through_date)?;
        if through > Utc::now().date_naive() {
            return Err(invalid("Cannot recognise future revenue"));
        }
        let tx = crate::service::transaction(&self.db)?;
        let mut recognized = 0i64;
        for old in InvoiceRow::select(self, "WHERE period_start IS NOT NULL ORDER BY id", &[])? {
            let id = old.id;
            if self.credit_row(id)?.is_some() {
                continue;
            }
            let start = date(
                old.period_start
                    .as_deref()
                    .ok_or_else(|| invalid("Missing service period start"))?,
            )?;
            let end = date(
                old.period_end
                    .as_deref()
                    .ok_or_else(|| invalid("Missing service period end"))?,
            )?;
            if through < date(&old.date)? {
                continue;
            }
            let cutoff = through.succ_opt().unwrap_or(through).min(end).max(start);
            let earned = (i128::from(old.net_minor) * i128::from((cutoff - start).num_days())
                / i128::from((end - start).num_days())) as i64;
            let delta = earned - old.recognized_minor;
            if delta <= 0 {
                continue;
            }
            self.post(
                &format!("revenue:{id}:{}", i.through_date),
                &i.through_date,
                "Recognise subscription revenue",
                vec![Line::debit("deferred", delta), Line::credit("sales", delta)],
                None,
                auth,
            )?;
            let next = BillingInvoiceWrite {
                recognized_minor: earned.into(),
                ..Default::default()
            }
            .update(self, id)?;
            next.audit(self, "billing.recognize", auth, Some(&old))?;
            recognized = recognized
                .checked_add(delta)
                .ok_or_else(|| invalid("Revenue total overflow"))?;
        }
        crate::service::commit(tx)?;
        Ok(json!({"recognized_minor":recognized}))
    }
}
