use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Statement {
    idempotency_key: String,
    account: String,
    date: String,
    amount_minor: i64,
    description: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Match {
    journal_line_id: i64,
}

impl Service {
    pub fn backoffice_bank_line(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("banking", "banking.write", auth)?;
        let i: Statement = decode(input)?;
        key(&i.idempotency_key)?;
        date(&i.date)?;
        description(&i.description)?;
        if i.amount_minor == 0 || i.amount_minor.unsigned_abs() > MAX_MONEY as u64 {
            return Err(invalid("Bank amount must be nonzero and within range"));
        }
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("banking", "statement")?;
        let account = self.account(&i.account)?;
        if account["kind"] != "asset" {
            return Err(invalid("Bank statements must use an asset account"));
        }
        if let Some(old) =
            self.cap_find(t, "source_key", SqlValue::Text(i.idempotency_key.clone()))?
        {
            if old["account_id"] != account["id"]
                || old["date"] != i.date
                || old["amount_minor"] != i.amount_minor
                || old["description"] != i.description
            {
                return Err(state("Statement key already used with different details"));
            }
            return Ok(old);
        }
        let row=self.cap_insert(t,json!({"source_key":i.idempotency_key,"account_id":account["id"],"date":i.date,"currency":"GBP","amount_minor":i.amount_minor,"description":i.description}))?;
        self.cap_audit(t, "banking.import", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub fn backoffice_match(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("banking", "banking.write", auth)?;
        let i: Match = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("banking", "statement")?;
        let old = self.raw(t, &json!(id))?;
        if old["journal_line_id"].is_number() {
            if old["journal_line_id"] == i.journal_line_id {
                return Ok(old);
            }
            return Err(state("Statement line is already reconciled"));
        }
        let line = self.raw(
            self.office_table("accounting", "line")?,
            &json!(i.journal_line_id),
        )?;
        let j = self.raw(
            self.office_table("accounting", "journal")?,
            &line["journal_id"],
        )?;
        if line["account_id"] != old["account_id"]
            || j["currency"] != old["currency"]
            || integer(&line, "debit")? - integer(&line, "credit")?
                != integer(&old, "amount_minor")?
        {
            return Err(invalid(
                "Reconciliation requires the same account, currency and signed amount",
            ));
        }
        if self
            .cap_find(t, "journal_line_id", SqlValue::Integer(i.journal_line_id))?
            .is_some()
        {
            return Err(state("Ledger line is already reconciled"));
        }
        let row = self.cap_update(t, id, json!({"journal_line_id":i.journal_line_id}))?;
        self.cap_audit(t, "banking.match", auth, Some(&old), &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
}
