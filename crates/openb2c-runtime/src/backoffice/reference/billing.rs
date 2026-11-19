use super::accounting::Line;
use super::*;

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
impl Service {
    pub(super) fn issue(
        &self,
        source: &str,
        i: &Invoice,
        inclusive_vat: Option<i64>,
        auth: &AuthContext,
    ) -> Result<Value> {
        let t = self.office_table("billing", "invoice")?;
        let request = serde_json::to_value(i).map_err(|_| invalid("Invalid invoice"))?;
        if let Some(row) = self.replay(t, source, &request)? {
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
        let row=self.cap_insert(t,json!({"source_key":source,"request_json":request.to_string(),"issuer":self.schema.organization.name,"customer":i.customer,"description":i.description,"date":i.date,"due_date":i.due_date,"posting_date":posting_date,"currency":"GBP","net_minor":i.net_minor,"tax_minor":vat,"tax_code":i.tax_code,"period_start":i.period_start,"period_end":i.period_end,"recognized_minor":if deferred {0} else {i.net_minor},"journal_id":j["id"]}))?;
        self.cap_audit(t, "billing.issue", auth, None, &row)?;
        Ok(row)
    }
    pub fn backoffice_invoice(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.write", auth)?;
        let i: Invoice = decode(input)?;
        key(&i.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        let row = self.issue(&format!("manual:{}", i.idempotency_key), &i, None, auth)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub(super) fn invoice_row(&self, id: i64) -> Result<Value> {
        self.raw(self.office_table("billing", "invoice")?, &json!(id))
    }
    fn credit_row(&self, id: i64) -> Result<Option<Value>> {
        self.cap_find(
            self.office_table("billing", "credit")?,
            "invoice_id",
            SqlValue::Integer(id),
        )
    }
    pub fn backoffice_invoice_view(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.read", auth)?;
        let mut row = self.invoice_row(id)?;
        let credit = self.credit_row(id)?;
        let receipt = self.cap_find(
            self.office_table("billing", "receipt")?,
            "invoice_id",
            SqlValue::Integer(id),
        )?;
        row["outstanding_minor"] = json!(if credit.is_some() || receipt.is_some() {
            0
        } else {
            total(&row)?
        });
        row["credit_note"] = json!(&credit);
        row["receipt"] = json!(&receipt);
        let refund = self.cap_find(
            self.office_table("billing", "refund")?,
            "invoice_id",
            SqlValue::Integer(id),
        )?;
        row["refund_due_minor"] = json!(
            if credit.is_some() && receipt.is_some() && refund.is_none() {
                total(&row)?
            } else {
                0
            }
        );
        row["refund"] = json!(refund);
        row["number"] = json!(format!("INV-{id:06}"));
        Ok(row)
    }
    pub(super) fn receive(&self, id: i64, day: &str, auth: &AuthContext) -> Result<Value> {
        let row = self.invoice_row(id)?;
        let t = self.office_table("billing", "receipt")?;
        if let Some(old) = self.cap_find(t, "invoice_id", SqlValue::Integer(id))? {
            if old["date"] != day {
                return Err(state("Invoice was already received on a different date"));
            }
            return Ok(old);
        }
        if self.credit_row(id)?.is_some() {
            return Err(state("A credited invoice cannot receive payment"));
        }
        if date(day)? < date(text(&row, "date")?)? {
            return Err(invalid("Receipt date cannot precede invoice date"));
        }
        let j = self.post(
            &format!("receipt:{id}"),
            day,
            "Invoice payment received",
            vec![
                Line::debit("clearing", total(&row)?),
                Line::credit("receivables", total(&row)?),
            ],
            None,
            auth,
        )?;
        let r = self.cap_insert(t, json!({"invoice_id":id,"date":day,"journal_id":j["id"]}))?;
        self.cap_audit(t, "billing.receive", auth, None, &r)?;
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
    ) -> Result<Value> {
        if let Some(row) = self.credit_row(id)? {
            if row["date"] != day || row["reason"] != reason {
                return Err(state("Invoice already has a different credit note"));
            }
            return Ok(row);
        }
        let invoice = self.invoice_row(id)?;
        let j = self.reverse_journal(integer(&invoice, "journal_id")?, day, reason, auth)?;
        let recognized = integer(&invoice, "recognized_minor")?;
        if invoice["period_start"].is_string() && recognized > 0 {
            self.post(
                &format!("credit-revenue:{id}"),
                day,
                "Reverse recognised subscription revenue",
                vec![
                    Line::debit("sales", recognized),
                    Line::credit("deferred", recognized),
                ],
                None,
                auth,
            )?;
        }
        let t = self.office_table("billing", "credit")?;
        let row = self.cap_insert(
            t,
            json!({"invoice_id":id,"date":day,"reason":reason,"journal_id":j["id"]}),
        )?;
        self.cap_audit(t, "billing.credit", auth, None, &row)?;
        Ok(row)
    }
    pub fn backoffice_credit(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.write", auth)?;
        let i: Reason = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        // Managed sale credits must follow their payment refund so access/payment state stays aligned.
        if !text(&self.invoice_row(id)?, "source_key")?.starts_with("manual:") {
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
    ) -> Result<Value> {
        let t = self.office_table("billing", "refund")?;
        if let Some(old) = self.cap_find(t, "invoice_id", SqlValue::Integer(id))? {
            if old["date"] != day || old["reason"] != reason {
                return Err(state("Invoice already has a different recorded refund"));
            }
            return Ok(old);
        }
        let invoice = self.invoice_row(id)?;
        let credit = self
            .credit_row(id)?
            .ok_or_else(|| state("Issue a credit note before recording a refund"))?;
        let receipt = self
            .cap_find(
                self.office_table("billing", "receipt")?,
                "invoice_id",
                SqlValue::Integer(id),
            )?
            .ok_or_else(|| state("An unpaid invoice has no cash to refund"))?;
        if date(day)? < date(text(&credit, "date")?)? || date(day)? < date(text(&receipt, "date")?)?
        {
            return Err(invalid("Refund cannot precede its receipt or credit note"));
        }
        description(reason)?;
        let j = self.post(
            &format!("refund:{id}"),
            day,
            "Record confirmed invoice refund",
            vec![
                Line::debit("receivables", total(&invoice)?),
                Line::credit("clearing", total(&invoice)?),
            ],
            None,
            auth,
        )?;
        let row = self.cap_insert(
            t,
            json!({"invoice_id":id,"date":day,"reason":reason,"journal_id":j["id"]}),
        )?;
        self.cap_audit(t, "billing.refund", auth, None, &row)?;
        Ok(row)
    }
    pub fn backoffice_refund(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("billing", "billing.refund", auth)?;
        let i: Reason = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        if !text(&self.invoice_row(id)?, "source_key")?.starts_with("manual:") {
            return Err(state("Record managed refunds through the original payment"));
        }
        self.refund_invoice(id, &i.date, &i.reason, auth)?;
        let row = self.backoffice_invoice_view(id, &AuthContext::system())?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub fn backoffice_recognize(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        if self.db.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE status='pending')",
                quote(self.office_table("billing", "event")?)
            ),
            [],
            |r| r.get::<_, bool>(0),
        )? {
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
        let t = self.office_table("billing", "invoice")?;
        let invoices = self.query(
            &format!(
                "SELECT * FROM {} WHERE period_start IS NOT NULL ORDER BY id",
                quote(t)
            ),
            &[],
        )?;
        let mut recognized = 0i64;
        for old in invoices {
            let id = integer(&old, "id")?;
            if self.credit_row(id)?.is_some() {
                continue;
            }
            let start = date(text(&old, "period_start")?)?;
            let end = date(text(&old, "period_end")?)?;
            if through < date(text(&old, "date")?)? {
                continue;
            }
            // Recognise through the end of the requested day. The service end is exclusive.
            let cutoff = through.succ_opt().unwrap_or(through).min(end).max(start);
            let earned = (i128::from(integer(&old, "net_minor")?)
                * i128::from((cutoff - start).num_days())
                / i128::from((end - start).num_days())) as i64;
            let delta = earned - integer(&old, "recognized_minor")?;
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
            let next = self.cap_update(t, id, json!({"recognized_minor":earned}))?;
            self.cap_audit(t, "billing.recognize", auth, Some(&old), &next)?;
            recognized = recognized
                .checked_add(delta)
                .ok_or_else(|| invalid("Revenue total overflow"))?;
        }
        crate::service::commit(tx)?;
        Ok(json!({"recognized_minor":recognized}))
    }
}

impl Service {
    pub(crate) fn preserve_legacy_refunds(&self, old: &openb2c_schema::Schema) -> Result<()> {
        if old
            .capabilities
            .get("billing")
            .is_none_or(|b| b.entities.contains_key("refund"))
            || !self.schema.capabilities.contains_key("billing")
        {
            return Ok(());
        }
        let invoice = self.office_table("billing", "invoice")?;
        let credits = self.office_table("billing", "credit")?;
        let journals = self.office_table("accounting", "journal")?;
        let refunds = self.office_table("billing", "refund")?;
        let historical=self.query(&format!("SELECT i.id AS invoice_id,j.id AS journal_id,j.date FROM {} i JOIN {} c ON c.invoice_id=i.id JOIN {} j ON j.source_key='refund:'||i.id",quote(invoice),quote(credits),quote(journals)),&[])?;
        for row in historical {
            if self
                .cap_find(refunds, "invoice_id", sql_value(&row["invoice_id"])?)?
                .is_some()
            {
                continue;
            }
            let saved=self.cap_insert(refunds,json!({"invoice_id":row["invoice_id"],"journal_id":row["journal_id"],"date":row["date"],"reason":"Preserved refund already recorded by billing version 1"}))?;
            self.cap_audit(
                refunds,
                "billing.migrate_refund",
                &AuthContext::system(),
                None,
                &saved,
            )?;
        }
        Ok(())
    }
}
