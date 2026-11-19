use super::*;

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
    kind: String,
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
        let t = self.office_table("accounting", "account")?;
        if let Some(old) = self.cap_find(t, "code", SqlValue::Text(a.code.clone()))? {
            if old["name"] != a.name || old["kind"] != a.kind {
                return Err(state("Account code already exists with different details"));
            }
            return Ok(old);
        }
        let row = self.cap_insert(t, json!({"code":a.code,"name":a.name,"kind":a.kind}))?;
        self.cap_audit(t, "accounting.account", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
    pub(super) fn account(&self, code: &str) -> Result<Value> {
        self.cap_find(
            self.office_table("accounting", "account")?,
            "code",
            SqlValue::Text(code.into()),
        )?
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
    ) -> Result<Value> {
        let t = self.office_table("accounting", "journal")?;
        let lines = lines
            .into_iter()
            .filter(|l| l.debit != 0 || l.credit != 0)
            .collect::<Vec<_>>();
        let request = json!({"date":day,"description":summary,"lines":lines,"reverses_id":reverse});
        if let Some(row) = self.replay(t, source, &request)? {
            return Ok(row);
        }
        self.posted_date(day)?;
        description(summary)?;
        if !(2..=100).contains(&lines.len()) {
            return Err(invalid("A journal requires 2–100 nonzero lines"));
        }
        let mut balance = 0i128;
        let mut rows = vec![];
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
        let journal=self.cap_insert(t,json!({"source_key":source,"request_json":request.to_string(),"date":day,"currency":"GBP","description":summary,"reverses_id":reverse}))?;
        for (account, l) in rows {
            self.cap_insert(self.office_table("accounting","line")?,json!({"journal_id":journal["id"],"account_id":account["id"],"debit":l.debit,"credit":l.credit}))?;
        }
        self.cap_audit(t, "accounting.post", auth, None, &journal)?;
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
        let result = self.journal_view(integer(&j, "id")?)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    fn journal_view(&self, id: i64) -> Result<Value> {
        let row = self.raw(self.office_table("accounting", "journal")?, &json!(id))?;
        let lines=self.query(&format!("SELECT l.*,a.code AS account FROM {} l JOIN {} a ON a.id=l.account_id WHERE l.journal_id=? ORDER BY l.id",quote(self.office_table("accounting","line")?),quote(self.office_table("accounting","account")?)),&[SqlValue::Integer(id)])?;
        Ok(json!({"journal":row,"lines":lines}))
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
    ) -> Result<Value> {
        let original = self.raw(self.office_table("accounting", "journal")?, &json!(id))?;
        description(reason)?;
        if day < text(&original, "date")? {
            return Err(invalid("A reversal cannot precede its original journal"));
        }
        let lines = self.journal_view(id)?["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                Ok(Line {
                    account: text(l, "account")?.into(),
                    debit: integer(l, "credit")?,
                    credit: integer(l, "debit")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
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
        let old = self.raw(self.office_table("accounting", "journal")?, &json!(id))?;
        if !text(&old, "source_key")?.starts_with("manual:") {
            return Err(state(
                "Use the owning workflow to correct a managed journal",
            ));
        }
        let result = self.reverse_journal(id, &i.date, &i.reason, auth)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub fn backoffice_close(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.close", auth)?;
        let i: Through = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("accounting", "period")?;
        if let Some(row) =
            self.cap_find(t, "through_date", SqlValue::Text(i.through_date.clone()))?
        {
            return Ok(row);
        }
        self.posted_date(&i.through_date)?;
        let row = self.cap_insert(
            t,
            json!({"through_date":i.through_date,"created_at":Utc::now().to_rfc3339()}),
        )?;
        self.cap_audit(t, "accounting.close", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
}
