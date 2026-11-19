use super::accounting::Line;
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Supplier {
    idempotency_key: String,
    name: String,
    email: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Expense {
    idempotency_key: String,
    supplier_id: i64,
    document_id: i64,
    supplier_reference: String,
    description: String,
    date: String,
    #[serde(default)]
    posting_date: Option<String>,
    net_minor: i64,
    tax_code: String,
    #[serde(default)]
    recoverable: bool,
}
impl Service {
    pub fn backoffice_supplier(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("purchasing", "purchasing.write", auth)?;
        let i: Supplier = decode(input)?;
        key(&i.idempotency_key)?;
        description(&i.name)?;
        description(&i.email)?;
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("purchasing", "supplier")?;
        if let Some(row) =
            self.cap_find(t, "source_key", SqlValue::Text(i.idempotency_key.clone()))?
        {
            if row["name"] != i.name || row["email"] != i.email {
                return Err(state("Supplier key already used with different details"));
            }
            return Ok(row);
        }
        let row = self.cap_insert(
            t,
            json!({"source_key":i.idempotency_key,"name":i.name,"email":i.email}),
        )?;
        self.cap_audit(t, "purchasing.supplier", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub fn backoffice_expense(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("purchasing", "purchasing.write", auth)?;
        let i: Expense = decode(input)?;
        key(&i.idempotency_key)?;
        description(&i.supplier_reference)?;
        description(&i.description)?;
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("purchasing", "expense")?;
        let request = serde_json::to_value(&i).map_err(|_| invalid("Invalid expense"))?;
        if let Some(row) = self.replay(t, &i.idempotency_key, &request)? {
            return Ok(row);
        }
        let supplier = self.raw(
            self.office_table("purchasing", "supplier")?,
            &json!(i.supplier_id),
        )?;
        self.raw(
            self.office_table("documents", "document")?,
            &json!(i.document_id),
        )?;
        let posting_day = i.posting_date.as_deref().unwrap_or(&i.date);
        date(&i.date)?;
        if posting_day < i.date.as_str() {
            return Err(invalid("Posting cannot precede the source expense date"));
        }
        let vat = tax(i.net_minor, &i.tax_code)?;
        let gross = i
            .net_minor
            .checked_add(vat)
            .ok_or_else(|| invalid("Amount overflow"))?;
        money(gross)?;
        if gross == 0 {
            return Err(invalid("An expense must have a positive total"));
        }
        if i.recoverable && self.cap_setting("tax", "vatRegistration") != Some("registered") {
            return Err(invalid(
                "Input VAT recovery requires explicit VAT registration",
            ));
        }
        let j = self.post(
            &format!("expense:{}", i.idempotency_key),
            posting_day,
            &i.description,
            vec![
                Line::debit("expenses", if i.recoverable { i.net_minor } else { gross }),
                Line::debit("tax_input", if i.recoverable { vat } else { 0 }),
                Line::credit("payables", gross),
            ],
            None,
            auth,
        )?;
        let row=self.cap_insert(t,json!({"source_key":i.idempotency_key,"request_json":request.to_string(),"supplier_id":i.supplier_id,"supplier_name":supplier["name"],"posting_date":posting_day,"document_id":i.document_id,"supplier_reference":i.supplier_reference,"description":i.description,"date":i.date,"currency":"GBP","net_minor":i.net_minor,"tax_minor":vat,"tax_code":i.tax_code,"recoverable":i.recoverable as i64,"journal_id":j["id"]}))?;
        self.cap_audit(t, "purchasing.expense", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub fn backoffice_pay_expense(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command("purchasing", "purchasing.write", auth)?;
        let i: Dated = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("purchasing", "expense")?;
        let old = self.raw(t, &json!(id))?;
        if old["void_journal_id"].is_number() {
            return Err(state("A void expense cannot be paid"));
        }
        if i.date.as_str() < text(&old, "date")? {
            return Err(invalid("Payment cannot precede expense date"));
        }
        // Posting the same source also validates replay dates, descriptions and amounts.
        let j = self.post(
            &format!("expense-payment:{id}"),
            &i.date,
            "Record supplier payment",
            vec![
                Line::debit("payables", total(&old)?),
                Line::credit("bank", total(&old)?),
            ],
            None,
            auth,
        )?;
        let row = self.cap_update(t, id, json!({"payment_journal_id":j["id"]}))?;
        if old != row {
            self.cap_audit(t, "purchasing.pay", auth, Some(&old), &row)?;
        }
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub fn backoffice_void_expense(
        &self,
        id: i64,
        input: &Value,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command("purchasing", "purchasing.write", auth)?;
        let i: Reason = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("purchasing", "expense")?;
        let old = self.raw(t, &json!(id))?;
        if old["payment_journal_id"].is_number() {
            return Err(state(
                "Paid expenses need a supplier credit/refund workflow; cannot void",
            ));
        }
        let j = self.reverse_journal(integer(&old, "journal_id")?, &i.date, &i.reason, auth)?;
        let row = self.cap_update(t, id, json!({"void_journal_id":j["id"]}))?;
        if old != row {
            self.cap_audit(t, "purchasing.void", auth, Some(&old), &row)?;
        }
        crate::service::commit(tx)?;
        Ok(row)
    }
}
