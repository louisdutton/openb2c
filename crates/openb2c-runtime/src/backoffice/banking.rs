use super::*;
use crate::records::{
    AccountingAccountKind, AccountingJournal, AccountingLine, BankingStatement as StatementRow,
    BankingStatementWrite, Record,
};
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
        let account = self.account(&i.account)?;
        if account.kind != AccountingAccountKind::Asset {
            return Err(invalid("Bank statements must use an asset account"));
        }
        if let Some(old) = StatementRow::first(self, "WHERE source_key=?", &[&i.idempotency_key])? {
            if old.account_id != account.id
                || old.date != i.date
                || old.amount_minor != i.amount_minor
                || old.description != i.description
            {
                return Err(state("Statement key already used with different details"));
            }
            return old.wire(self);
        }
        let row = BankingStatementWrite {
            source_key: i.idempotency_key.into(),
            account_id: account.id.into(),
            date: i.date.into(),
            currency: "GBP".to_owned().into(),
            amount_minor: i.amount_minor.into(),
            description: i.description.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "banking.import", auth, None)?;
        crate::service::commit(tx)?;
        row.wire(self)
    }
    pub fn backoffice_match(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("banking", "banking.write", auth)?;
        let i: Match = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let old = StatementRow::get(self, id)?;
        if let Some(line) = old.journal_line_id {
            if line == i.journal_line_id {
                return old.wire(self);
            }
            return Err(state("Statement line is already reconciled"));
        }
        let line = AccountingLine::get(self, i.journal_line_id)?;
        let j = AccountingJournal::get(self, line.journal_id)?;
        if line.account_id != old.account_id
            || j.currency != old.currency
            || line.debit - line.credit != old.amount_minor
        {
            return Err(invalid(
                "Reconciliation requires the same account, currency and signed amount",
            ));
        }
        if StatementRow::first(self, "WHERE journal_line_id=?", &[&i.journal_line_id])?.is_some() {
            return Err(state("Ledger line is already reconciled"));
        }
        let row = BankingStatementWrite {
            journal_line_id: Some(i.journal_line_id).into(),
            ..Default::default()
        }
        .update(self, id)?;
        row.audit(self, "banking.match", auth, Some(&old))?;
        crate::service::commit(tx)?;
        row.wire(self)
    }
}
