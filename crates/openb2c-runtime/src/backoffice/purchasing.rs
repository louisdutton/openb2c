use super::accounting::Line;
use super::*;
use crate::records::{
    DocumentsDocument, PurchasingExpense as ExpenseRow, PurchasingExpenseWrite,
    PurchasingSupplier as SupplierRow, PurchasingSupplierWrite, Record,
};
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
fn gross(row: &ExpenseRow) -> Result<i64> {
    row.net_minor
        .checked_add(row.tax_minor)
        .ok_or_else(|| invalid("Amount overflow"))
}
impl Service {
    pub fn backoffice_supplier(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("purchasing", "purchasing.write", auth)?;
        let i: Supplier = decode(input)?;
        key(&i.idempotency_key)?;
        description(&i.name)?;
        description(&i.email)?;
        let tx = crate::service::transaction(&self.db)?;
        if let Some(row) = SupplierRow::first(self, "WHERE source_key=?", &[&i.idempotency_key])? {
            if row.name != i.name || row.email != i.email {
                return Err(state("Supplier key already used with different details"));
            }
            return row.wire(self);
        }
        let row = PurchasingSupplierWrite {
            source_key: i.idempotency_key.into(),
            name: i.name.into(),
            email: i.email.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "purchasing.supplier", auth, None)?;
        crate::service::commit(tx)?;
        row.wire(self)
    }
    pub fn backoffice_expense(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("purchasing", "purchasing.write", auth)?;
        let i: Expense = decode(input)?;
        key(&i.idempotency_key)?;
        description(&i.supplier_reference)?;
        description(&i.description)?;
        let tx = crate::service::transaction(&self.db)?;
        let request = serde_json::to_value(&i)
            .map_err(|e| invalid(e.to_string()))?
            .to_string();
        if let Some(row) = ExpenseRow::first(self, "WHERE source_key=?", &[&i.idempotency_key])? {
            if row.request_json != request {
                return Err(ServiceError::new(
                    "conflict",
                    "Idempotency key already used with different details",
                ));
            }
            return row.wire(self);
        }
        let supplier = SupplierRow::get(self, i.supplier_id)?;
        DocumentsDocument::get(self, i.document_id)?;
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
        let row = PurchasingExpenseWrite {
            source_key: i.idempotency_key.clone().into(),
            request_json: request.into(),
            supplier_id: i.supplier_id.into(),
            supplier_name: supplier.name.into(),
            posting_date: Some(posting_day.to_owned()).into(),
            document_id: i.document_id.into(),
            supplier_reference: i.supplier_reference.into(),
            description: i.description.into(),
            date: i.date.into(),
            currency: "GBP".to_owned().into(),
            net_minor: i.net_minor.into(),
            tax_minor: vat.into(),
            tax_code: i.tax_code.into(),
            recoverable: (i.recoverable as i64).into(),
            journal_id: j.id.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "purchasing.expense", auth, None)?;
        crate::service::commit(tx)?;
        row.wire(self)
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
        let old = ExpenseRow::get(self, id)?;
        if old.void_journal_id.is_some() {
            return Err(state("A void expense cannot be paid"));
        }
        if i.date < old.date {
            return Err(invalid("Payment cannot precede expense date"));
        }
        let j = self.post(
            &format!("expense-payment:{id}"),
            &i.date,
            "Record supplier payment",
            vec![
                Line::debit("payables", gross(&old)?),
                Line::credit("bank", gross(&old)?),
            ],
            None,
            auth,
        )?;
        let row = PurchasingExpenseWrite {
            payment_journal_id: Some(j.id).into(),
            ..Default::default()
        }
        .update(self, id)?;
        if old != row {
            row.audit(self, "purchasing.pay", auth, Some(&old))?;
        }
        crate::service::commit(tx)?;
        row.wire(self)
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
        let old = ExpenseRow::get(self, id)?;
        if old.payment_journal_id.is_some() {
            return Err(state(
                "Paid expenses need a supplier credit/refund workflow; cannot void",
            ));
        }
        let j = self.reverse_journal(old.journal_id, &i.date, &i.reason, auth)?;
        let row = PurchasingExpenseWrite {
            void_journal_id: Some(j.id).into(),
            ..Default::default()
        }
        .update(self, id)?;
        if old != row {
            row.audit(self, "purchasing.void", auth, Some(&old))?;
        }
        crate::service::commit(tx)?;
        row.wire(self)
    }
}
