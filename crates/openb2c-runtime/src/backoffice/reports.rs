use super::*;

#[derive(Serialize)]
struct BalanceRow {
    id: i64,
    code: String,
    name: String,
    kind: String,
    balance_minor: i64,
    period_movement_minor: i64,
}
impl BalanceRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            code: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
            balance_minor: row.get(4)?,
            period_movement_minor: row.get(5)?,
        })
    }
}
#[derive(Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Sale,
    Purchase,
}
#[derive(Clone, Serialize)]
struct Evidence {
    direction: Direction,
    record_id: i64,
    date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_date: Option<String>,
    net_minor: i64,
    tax_minor: i64,
    tax_code: String,
    correction: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    recoverable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<i64>,
}
struct Source {
    evidence: Evidence,
    correction_date: Option<String>,
}
impl Source {
    fn read(row: &rusqlite::Row<'_>, direction: Direction) -> rusqlite::Result<Self> {
        let source_date: String = row.get("date")?;
        Ok(Self {
            evidence: Evidence {
                direction,
                record_id: row.get("id")?,
                date: row
                    .get::<_, Option<String>>("posting_date")?
                    .unwrap_or_else(|| source_date.clone()),
                source_date: Some(source_date),
                net_minor: row.get("net_minor")?,
                tax_minor: row.get("tax_minor")?,
                tax_code: row.get("tax_code")?,
                correction: false,
                recoverable: if direction == Direction::Purchase {
                    Some(row.get::<_, i64>("recoverable")? == 1)
                } else {
                    None
                },
                document_id: if direction == Direction::Purchase {
                    Some(row.get("document_id")?)
                } else {
                    None
                },
            },
            correction_date: row.get("correction_date")?,
        })
    }
    fn append(self, range: &Range, evidence: &mut Vec<Evidence>) -> Result<()> {
        let inside = |day: &str| day >= range.from.as_str() && day <= range.through.as_str();
        if inside(&self.evidence.date) {
            evidence.push(self.evidence.clone());
        }
        if let Some(day) = self.correction_date
            && inside(&day)
        {
            let mut correction = self.evidence;
            correction.date = day;
            correction.net_minor = negative(correction.net_minor)?;
            correction.tax_minor = negative(correction.tax_minor)?;
            correction.correction = true;
            if correction.direction == Direction::Sale {
                correction.source_date = None;
            }
            evidence.push(correction);
        }
        Ok(())
    }
}
#[derive(Serialize)]
struct Blocker {
    reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direction: Option<Direction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_id: Option<i64>,
}
impl Blocker {
    fn new(reason: &'static str) -> Self {
        Self {
            reason,
            count: None,
            direction: None,
            record_id: None,
            journal_id: None,
        }
    }
}
fn add(total: &mut i64, amount: i64) -> Result<()> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| invalid("Report total overflow"))?;
    Ok(())
}
fn negative(amount: i64) -> Result<i64> {
    amount
        .checked_neg()
        .ok_or_else(|| invalid("Report amount overflow"))
}
impl Service {
    fn bank_review_count(&self) -> Result<i64> {
        #[cfg(all(not(feature = "reference"), feature = "cap_payments"))]
        {
            self.unresolved_transfer_count()
        }
        #[cfg(any(feature = "reference", not(feature = "cap_payments")))]
        {
            Ok(0)
        }
    }
    fn unposted_billing_count(&self) -> Result<i64> {
        if let Ok(table) = self.office_table("billing", "event") {
            Ok(self.db.query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE status='pending'",
                    quote(table)
                ),
                [],
                |r| r.get(0),
            )?)
        } else {
            Ok(0)
        }
    }
    pub fn backoffice_financial_report(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("accounting", "accounting.read", auth)?;
        let r: Range = decode(input)?;
        r.check()?;
        let accounts = self.office_table("accounting", "account")?;

        let lines = self.office_table("accounting", "line")?;
        let journals = self.office_table("accounting", "journal")?;
        let mut statement=self.db.prepare(&format!("SELECT a.id,a.code,a.name,a.kind,COALESCE(SUM(CASE WHEN j.date<=? THEN l.debit-l.credit ELSE 0 END),0) AS balance_minor,COALESCE(SUM(CASE WHEN j.date>=? AND j.date<=? THEN l.debit-l.credit ELSE 0 END),0) AS period_movement_minor FROM {} a LEFT JOIN {} l ON l.account_id=a.id LEFT JOIN {} j ON j.id=l.journal_id GROUP BY a.id ORDER BY a.code",quote(accounts),quote(lines),quote(journals)))?;
        let rows = statement
            .query_map(
                rusqlite::params![r.through, r.from, r.through],
                BalanceRow::read,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let (
            mut income,
            mut expenses,
            mut assets,
            mut liabilities,
            mut equity,
            mut earnings,
            mut balance,
        ) = (0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
        for row in &rows {
            let b = row.balance_minor;
            let movement = row.period_movement_minor;
            add(&mut balance, b)?;
            match row.kind.as_str() {
                "asset" => add(&mut assets, b)?,
                "liability" => add(&mut liabilities, negative(b)?)?,
                "equity" => add(&mut equity, negative(b)?)?,
                "income" => {
                    add(&mut income, negative(movement)?)?;
                    add(&mut earnings, negative(b)?)?;
                }
                "expense" => {
                    add(&mut expenses, movement)?;
                    add(&mut earnings, negative(b)?)?;
                }
                _ => return Err(invalid("Unknown ledger account kind")),
            }
        }
        let profit = income
            .checked_sub(expenses)
            .ok_or_else(|| invalid("Profit overflow"))?;
        Ok(
            json!({"currency":"GBP","from":r.from,"through":r.through,"trial_balance":rows,"unposted_billing_events":self.unposted_billing_count()?,"unresolved_bank_transactions":self.bank_review_count()?,"balanced":balance==0,"profit_and_loss":{"income_minor":income,"expenses_minor":expenses,"profit_minor":profit},"balance_sheet":{"assets_minor":assets,"liabilities_minor":liabilities,"equity_minor":equity,"accumulated_earnings_minor":earnings}}),
        )
    }
    pub fn backoffice_vat_report(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("tax", "tax.read", auth)?;
        let r: Range = decode(input)?;
        r.check()?;
        let mut evidence = Vec::new();
        if let Ok(t) = self.office_table("billing", "invoice") {
            let credits = self.office_table("billing", "credit")?;
            let mut statement=self.db.prepare(&format!("SELECT i.id,i.date,i.posting_date,i.net_minor,i.tax_minor,i.tax_code,c.date AS correction_date FROM {} i LEFT JOIN {} c ON c.invoice_id=i.id ORDER BY i.id",quote(t),quote(credits)))?;
            let sources = statement
                .query_map([], |row| Source::read(row, Direction::Sale))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for source in sources {
                source.append(&r, &mut evidence)?;
            }
        }
        if let Ok(t) = self.office_table("purchasing", "expense") {
            let journals = self.office_table("accounting", "journal")?;
            let mut statement=self.db.prepare(&format!("SELECT e.id,e.date,e.posting_date,e.net_minor,e.tax_minor,e.tax_code,e.recoverable,e.document_id,j.date AS correction_date FROM {} e LEFT JOIN {} j ON j.id=e.void_journal_id ORDER BY e.id",quote(t),quote(journals)))?;
            let sources = statement
                .query_map([], |row| Source::read(row, Direction::Purchase))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for source in sources {
                source.append(&r, &mut evidence)?;
            }
        }
        let registration = self
            .cap_setting("tax", "vatRegistration")
            .unwrap_or("unassessed");
        let mut blockers = Vec::new();
        let unresolved = self.bank_review_count()?;
        if unresolved > 0 {
            let mut blocker =
                Blocker::new("Bank transactions require allocation or explicit accounting review");
            blocker.count = Some(unresolved);
            blockers.push(blocker);
        }
        let pending = self.unposted_billing_count()?;
        if pending > 0 {
            let mut b = Blocker::new("Accepted billing events await posting in an open period");
            b.count = Some(pending);
            blockers.push(b);
        }
        if registration != "registered" {
            blockers.push(Blocker::new(
                "VAT return preparation requires confirmed VAT registration",
            ));
        }
        let (mut outputs, mut inputs, mut sales, mut purchases) = (0i64, 0i64, 0i64, 0i64);
        for e in &evidence {
            if ["unassessed", "outside", "not_registered"].contains(&e.tax_code.as_str()) {
                let mut b = Blocker::new(
                    "VAT treatment needs review or falls outside the supported domestic standard-accounting scope",
                );
                b.direction = Some(e.direction);
                b.record_id = Some(e.record_id);
                blockers.push(b);
                continue;
            }
            if e.direction == Direction::Sale {
                add(&mut sales, e.net_minor)?;
                add(&mut outputs, e.tax_minor)?;
            } else {
                add(&mut purchases, e.net_minor)?;
                if e.recoverable == Some(true) {
                    add(&mut inputs, e.tax_minor)?;
                }
            }
        }
        let journals = self.office_table("accounting", "journal")?;
        let lines = self.office_table("accounting", "line")?;
        let accounts = self.office_table("accounting", "account")?;
        let deposits: (i64, i64) = self.db.query_row(
            &format!("SELECT coalesce(sum(CASE WHEN j.date<? THEN l.debit-l.credit ELSE 0 END),0),coalesce(sum(CASE WHEN j.date<=? THEN l.debit-l.credit ELSE 0 END),0) FROM {} j JOIN {} l ON l.journal_id=j.id JOIN {} a ON a.id=l.account_id WHERE a.code='customer_deposits'",quote(journals),quote(lines),quote(accounts)),
            rusqlite::params![r.from,r.through], |row| Ok((row.get(0)?,row.get(1)?)))?;
        if deposits != (0, 0) {
            blockers.push(Blocker::new("Customer deposits cross this reporting boundary; review advance-payment VAT tax points"));
        }
        if let Ok(events) = self.office_table("billing", "event") {
            let deferred: i64 = self.db.query_row(
                &format!("SELECT count(*) FROM {} j JOIN {} e ON e.source_key=j.source_key WHERE j.source_key LIKE 'cash:%' AND j.date>=? AND j.date<=? AND substr(e.occurred_at,1,10)<?",quote(journals),quote(events)),
                rusqlite::params![r.from,r.through,r.from], |row| row.get(0))?;
            if deferred > 0 {
                let mut blocker = Blocker::new(
                    "Allocated cash originates before this reporting period; review advance-payment VAT and prior returns",
                );
                blocker.count = Some(deferred);
                blockers.push(blocker);
            }
        }
        let mut statement=self.db.prepare(&format!("SELECT DISTINCT j.id,j.source_key FROM {} j JOIN {} l ON l.journal_id=j.id JOIN {} a ON a.id=l.account_id WHERE j.date>=? AND j.date<=? AND (j.source_key LIKE 'manual:%' OR j.source_key LIKE 'reversal:%') AND a.code IN ('tax_input','tax_output','sales','expenses')",quote(journals),quote(lines),quote(accounts)))?;
        let manual = statement
            .query_map(rusqlite::params![r.from, r.through], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, source) in manual {
            if let Some(original) = source.strip_prefix("reversal:") {
                let original = original
                    .parse::<i64>()
                    .map_err(|_| invalid("Invalid reversal"))?;
                let source: String = self.db.query_row(
                    &format!("SELECT source_key FROM {} WHERE id=?", quote(journals)),
                    [original],
                    |row| row.get(0),
                )?;
                if !source.starts_with("manual:") {
                    continue;
                }
            }
            let mut b =
                Blocker::new("Manual financial posting has no VAT source document classification");
            b.journal_id = Some(id);
            blockers.push(b);
        }
        let net = outputs
            .checked_sub(inputs)
            .ok_or_else(|| invalid("VAT total overflow"))?;
        Ok(
            json!({"jurisdiction":"GB","rules_version":1,"scheme":"standard_accrual_domestic","currency":"GBP","from":r.from,"through":r.through,"draft":true,"requires_review":true,"filing_supported":false,"blockers":blockers,"evidence":evidence,"boxes":{"1_vat_due_sales_minor":outputs,"2_vat_due_acquisitions_minor":0,"3_total_vat_due_minor":outputs,"4_vat_reclaimed_minor":inputs,"5_net_vat_minor":net.unsigned_abs(),"6_sales_whole_pounds":sales/100,"7_purchases_whole_pounds":purchases/100,"8_dispatches_whole_pounds":0,"9_acquisitions_whole_pounds":0},"net_position":if net>=0{"payable"}else{"repayable"},"scope":"Domestic GBP transactions under standard VAT accounting only. No imports, reverse charge, Northern Ireland/EU trade, special schemes or HMRC submission."}),
        )
    }
}
