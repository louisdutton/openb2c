use super::*;
use crate::records::{
    AccountingAccount as AccountRow, AccountingAccountKind, AccountingAccountWrite,
    AccountingJournal as JournalRow, AccountingJournalWrite, AccountingLine, AccountingLineWrite,
    AccountingPeriod, AccountingPeriodWrite, Record,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Line {
    pub account: String,
    pub debit: i64,
    pub credit: i64,
}
impl Line {
    pub(super) fn debit(account: &str, amount: i64) -> Self {
        Self {
            account: account.into(),
            debit: amount,
            credit: 0,
        }
    }
    pub(super) fn credit(account: &str, amount: i64) -> Self {
        Self {
            account: account.into(),
            debit: 0,
            credit: amount,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    code: String,
    name: String,
    kind: AccountingAccountKind,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    idempotency_key: String,
    date: String,
    #[serde(default = "gbp")]
    currency: String,
    description: String,
    lines: Vec<Line>,
}
fn gbp() -> String {
    "GBP".into()
}
impl Service {
    pub fn backoffice_account(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.write", auth)?;
        let a: Account = decode(input)?;
        key(&a.code)?;
        description(&a.name)?;
        let tx = crate::service::transaction(&self.db)?;
        if let Some(old) = AccountRow::first(self, "WHERE code=?", &[&a.code])? {
            if old.name != a.name || old.kind != a.kind {
                return Err(state("Account code already exists with different details"));
            }
            return old.wire(self);
        }
        let row = AccountingAccountWrite {
            code: a.code.into(),
            name: a.name.into(),
            kind: a.kind.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "accounting.account", auth, None)?;
        crate::service::commit(tx)?;
        row.wire(self)
    }
    pub(super) fn account(&self, code: &str) -> Result<AccountRow> {
        AccountRow::first(self, "WHERE code=?", &[&code])?
            .ok_or_else(|| invalid(format!("Unknown account {code}")))
    }
    pub(super) fn post(
        &self,
        source: &str,
        day: &str,
        summary: &str,
        lines: Vec<Line>,
        reverse: Option<i64>,
        auth: &AuthContext,
    ) -> Result<JournalRow> {
        let lines = lines
            .into_iter()
            .filter(|l| l.debit != 0 || l.credit != 0)
            .collect::<Vec<_>>();
        let request = json!({"date":day,"description":summary,"lines":lines,"reverses_id":reverse})
            .to_string();
        if let Some(row) = JournalRow::first(self, "WHERE source_key=?", &[&source])? {
            if row.request_json != request {
                return Err(ServiceError::new(
                    "conflict",
                    "Idempotency key already used with different details",
                ));
            }
            return Ok(row);
        }
        self.posted_date(day)?;
        description(summary)?;
        if !(2..=100).contains(&lines.len()) {
            return Err(invalid("A journal requires 2–100 nonzero lines"));
        }
        let mut balance = 0i128;
        let mut rows = Vec::new();
        for l in lines {
            money(l.debit)?;
            money(l.credit)?;
            if (l.debit == 0) == (l.credit == 0) {
                return Err(invalid(
                    "Each line must contain exactly one debit or credit",
                ));
            }
            balance += i128::from(l.debit) - i128::from(l.credit);
            rows.push((self.account(&l.account)?, l));
        }
        if balance != 0 {
            return Err(invalid("Journal debits and credits must balance"));
        }
        let journal = AccountingJournalWrite {
            source_key: source.to_owned().into(),
            request_json: request.into(),
            date: day.to_owned().into(),
            currency: "GBP".to_owned().into(),
            description: summary.to_owned().into(),
            reverses_id: reverse.into(),
            ..Default::default()
        }
        .insert(self)?;
        for (account, l) in rows {
            AccountingLineWrite {
                journal_id: journal.id.into(),
                account_id: account.id.into(),
                debit: l.debit.into(),
                credit: l.credit.into(),
                ..Default::default()
            }
            .insert(self)?;
        }
        journal.audit(self, "accounting.post", auth, None)?;
        Ok(journal)
    }
    pub fn backoffice_journal(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.write", auth)?;
        let i: Journal = decode(input)?;
        key(&i.idempotency_key)?;
        if i.currency != "GBP" {
            return Err(invalid("The UK pilot ledger supports GBP only"));
        }
        let tx = crate::service::transaction(&self.db)?;
        let j = self.post(
            &format!("manual:{}", i.idempotency_key),
            &i.date,
            &i.description,
            i.lines,
            None,
            auth,
        )?;
        let result = self.journal_view(j.id)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    fn journal_view(&self, id: i64) -> Result<Value> {
        #[derive(Serialize)]
        struct VisibleLine {
            #[serde(flatten)]
            line: AccountingLine,
            account: String,
        }
        let row = JournalRow::get(self, id)?;
        let mut statement=self.db.prepare(&format!("SELECT l.*,a.code AS account FROM {} l JOIN {} a ON a.id=l.account_id WHERE l.journal_id=? ORDER BY l.id",quote(AccountingLine::TABLE),quote(AccountRow::TABLE)))?;
        let lines = statement
            .query_map([id], |r| {
                Ok(VisibleLine {
                    line: AccountingLine::read(r)?,
                    account: r.get("account")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"journal":row.wire(self)?,"lines":lines}))
    }
    pub fn backoffice_journal_view(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.read", auth)?;
        self.journal_view(id)
    }
    pub(super) fn reverse_journal(
        &self,
        id: i64,
        day: &str,
        reason: &str,
        auth: &AuthContext,
    ) -> Result<JournalRow> {
        let original = JournalRow::get(self, id)?;
        description(reason)?;
        if day < original.date.as_str() {
            return Err(invalid("A reversal cannot precede its original journal"));
        }
        let mut statement=self.db.prepare(&format!("SELECT a.code,l.credit,l.debit FROM {} l JOIN {} a ON a.id=l.account_id WHERE l.journal_id=? ORDER BY l.id",quote(AccountingLine::TABLE),quote(AccountRow::TABLE)))?;
        let lines = statement
            .query_map([id], |r| {
                Ok(Line {
                    account: r.get(0)?,
                    debit: r.get(1)?,
                    credit: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.post(
            &format!("reversal:{id}"),
            day,
            reason,
            lines,
            Some(id),
            auth,
        )
    }
    pub fn backoffice_reverse(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.write", auth)?;
        let i: Reason = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        if !JournalRow::get(self, id)?.source_key.starts_with("manual:") {
            return Err(state(
                "Use the owning workflow to correct a managed journal",
            ));
        }
        let result = self
            .reverse_journal(id, &i.date, &i.reason, auth)?
            .wire(self)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub fn backoffice_close(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.close", auth)?;
        let i: Through = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        if let Some(row) =
            AccountingPeriod::first(self, "WHERE through_date=?", &[&i.through_date])?
        {
            return row.wire(self);
        }
        self.posted_date(&i.through_date)?;
        let row = AccountingPeriodWrite {
            through_date: i.through_date.into(),
            created_at: Utc::now().to_rfc3339().into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "accounting.close", auth, None)?;
        crate::service::commit(tx)?;
        row.wire(self)
    }
}
