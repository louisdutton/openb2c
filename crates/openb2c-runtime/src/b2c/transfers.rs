//! Durable bank facts and bounded allocations; business matching is compiled from Nix.
use super::*;
use crate::records::{
    PaymentsAllocation as Allocation, PaymentsAllocationKind as Kind,
    PaymentsAllocationWrite as AllocationWrite, PaymentsPayment as Payment,
    PaymentsPaymentStatus as Status, PaymentsPaymentWrite as PaymentWrite, PaymentsSync,
    PaymentsTransaction as Transfer, PaymentsTransactionDirection as Direction,
    PaymentsTransactionDisposition as Disposition, PaymentsTransactionSource as Source,
    PaymentsTransactionStatus as TransferStatus, PaymentsTransactionWrite as TransferWrite, Record,
};
use serde::{Deserialize, Serialize};

pub(super) struct Destination {
    pub account: String,
    pub name: String,
    pub sort_code: String,
    pub account_number: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BankFact {
    pub transaction_id: String,
    pub account: String,
    pub amount_minor: i64,
    pub currency: String,
    pub direction: String,
    pub status: String,
    #[serde(default)]
    pub reference: String,
    pub settled_at: Option<String>,
    pub updated_at: String,
    #[serde(default)]
    pub bank_source: String,
    #[serde(default)]
    pub source_reference: String,
    #[serde(default)]
    pub narrative: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Allocate {
    payment_id: i64,
    amount_minor: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    exclude: bool,
    reason: String,
}

impl Service {
    pub(crate) fn unresolved_transfer_count(&self) -> Result<i64> {
        let partial_review = if self.cap_enabled("billing_payments") {
            ""
        } else {
            "a.amount_minor<>p.amount_minor OR "
        };
        let sql = format!(
            "SELECT count(*) FROM {} t WHERE t.disposition='open' AND (t.review_reason IS NOT NULL OR (t.status='settled' AND t.amount_minor>(SELECT coalesce(sum(a.amount_minor),0) FROM {} a WHERE a.transaction_id=t.id)) OR EXISTS (SELECT 1 FROM {} a JOIN {} p ON p.id=a.payment_id WHERE a.transaction_id=t.id AND a.kind='receipt' AND ({partial_review}p.status IN ('pending','cancelled','failed'))))",
            quote(Transfer::TABLE),
            quote(Allocation::TABLE),
            quote(Allocation::TABLE),
            quote(Payment::TABLE)
        );
        Ok(self.db.query_row(&sql, [], |r| r.get(0))?)
    }
    pub fn review_transfer(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.reconcile", auth)?;
        let input: Review = decode(input)?;
        if !(15..=500).contains(&input.reason.len()) || input.reason.chars().any(char::is_control) {
            return Err(invalid("Review requires a 15–500 character reason"));
        }
        let tx = crate::service::transaction(&self.db)?;
        let old = Transfer::get(self, id)?;
        let next = TransferWrite {
            disposition: if input.exclude {
                Disposition::Excluded
            } else {
                Disposition::Open
            }
            .into(),
            resolution_note: Some(input.reason).into(),
            review_reason: None.into(),
            ..Default::default()
        }
        .update(self, id)?;
        next.audit(self, "payments.review", auth, Some(&old))?;
        let response = self.transfer_view(&next)?;
        crate::service::commit(tx)?;
        Ok(response)
    }
    pub(crate) fn reconcile_pending_transfers(&self, auth: &AuthContext) -> Result<()> {
        self.cap_command("payments", "payments.reconcile", auth)?;
        // Retained unmatched credits may become matchable after the subscription
        // renewal worker creates the next request. No receipt is consumed twice.
        let sql = format!(
            "SELECT t.* FROM {} t WHERE t.direction='in' AND t.status='settled' AND t.disposition='open' AND t.review_reason IS NULL AND EXISTS (SELECT 1 FROM {} p WHERE p.status='pending' AND p.bank_reference=t.reference AND p.bank_account=t.account AND p.currency=t.currency AND p.amount_minor=t.amount_minor) AND NOT EXISTS (SELECT 1 FROM {} a WHERE a.transaction_id=t.id) ORDER BY t.id LIMIT 100",
            quote(Transfer::TABLE),
            quote(Payment::TABLE),
            quote(Allocation::TABLE)
        );
        for row in Transfer::query(self, &sql, &[])? {
            if self.allocated(row.id)? == 0 {
                self.reconcile_transfer(row.id, auth)?;
            }
        }
        Ok(())
    }
    pub(super) fn transfer_destination(&self) -> Result<Destination> {
        if self.cap_setting("payments", "feed") == Some("simulated") {
            return Ok(Destination {
                account: "simulated".into(),
                name: "SIMULATED BANK - DO NOT TRANSFER MONEY".into(),
                sort_code: "000000".into(),
                account_number: "00000000".into(),
            });
        }
        let required = |name| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| invalid(format!("Missing {name}")))
        };
        let d = Destination {
            account: required("BANK_ACCOUNT_ID")?,
            name: required("BANK_ACCOUNT_NAME")?,
            sort_code: required("BANK_SORT_CODE")?,
            account_number: required("BANK_ACCOUNT_NUMBER")?,
        };
        if d.sort_code.len() != 6
            || d.account_number.len() != 8
            || !d
                .sort_code
                .bytes()
                .chain(d.account_number.bytes())
                .all(|b| b.is_ascii_digit())
            || d.name.chars().any(char::is_control)
        {
            return Err(invalid("Invalid receiving bank details"));
        }
        Ok(d)
    }
    pub fn import_simulated_transfer(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.import", auth)?;
        if self.cap_setting("payments", "feed") != Some("simulated")
            || !crate::identity::fake_allowed()
        {
            return Err(ServiceError::new(
                "forbidden",
                "Simulated transfers are disabled",
            ));
        }
        let fact: BankFact = decode(input)?;
        let id = self.record_bank_fact(fact, Source::Simulated, auth, Utc::now())?;
        self.reconcile_transfer(id, &AuthContext::system())
    }
    pub(crate) fn record_bank_fact(
        &self,
        fact: BankFact,
        source: Source,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<i64> {
        if fact.transaction_id.is_empty()
            || fact.transaction_id.len() > 210
            || fact.transaction_id.chars().any(char::is_control)
        {
            return Err(invalid(
                "Bank transaction ID must contain 1–210 printable characters",
            ));
        }
        if fact.account != self.transfer_destination()?.account
            || !(1..=1_000_000_000_000).contains(&fact.amount_minor)
            || fact.currency != "GBP"
            || fact.reference.len() > 500
            || fact.reference.chars().any(char::is_control)
            || fact.bank_source.len() > 100
            || fact.bank_source.chars().any(char::is_control)
            || fact.source_reference.len() > 210
            || fact.source_reference.chars().any(char::is_control)
            || fact.narrative.len() > 2000
            || fact.narrative.chars().any(char::is_control)
        {
            return Err(invalid("Invalid bank fact or receiving account"));
        }
        let direction = Direction::try_from(fact.direction.as_str())
            .map_err(|_| invalid("Invalid transfer direction"))?;
        let status = TransferStatus::try_from(fact.status.as_str())
            .map_err(|_| invalid("Invalid transfer status"))?;
        let updated = instant(&fact.updated_at)?;
        if updated > now + chrono::Duration::minutes(5) {
            return Err(invalid("Bank update is in the future"));
        }
        if let Some(settled) = &fact.settled_at {
            if instant(settled)? > updated {
                return Err(invalid("Settlement cannot follow its bank update"));
            }
        } else if status == TransferStatus::Settled {
            return Err(invalid("Settled transfers require settlement time"));
        }
        let reference = fact.reference.trim().to_ascii_uppercase();
        let tx = crate::service::transaction(&self.db)?;
        let old = Transfer::first(
            self,
            "WHERE source=? AND account=? AND transaction_id=?",
            &[&source, &fact.account, &fact.transaction_id],
        )?;
        if let Some(old) = &old {
            if old.amount_minor != fact.amount_minor
                || old.currency != fact.currency
                || old.direction != direction
            {
                return Err(ServiceError::new(
                    "conflict",
                    "Bank transaction identity is bound to different monetary facts",
                ));
            }
            if instant(&old.updated_at)? > updated {
                return Ok(old.id);
            }
            if instant(&old.updated_at)? == updated {
                if old.status != status
                    || old.reference != reference
                    || old.settled_at != fact.settled_at
                    || old.source_reference != fact.source_reference
                    || old.narrative != fact.narrative
                {
                    return Err(ServiceError::new(
                        "conflict",
                        "Conflicting bank facts at the same revision",
                    ));
                }
                return Ok(old.id);
            }
        }
        let write = TransferWrite {
            source: source.into(),
            account: fact.account.into(),
            transaction_id: fact.transaction_id.into(),
            amount_minor: fact.amount_minor.into(),
            currency: fact.currency.into(),
            direction: direction.into(),
            status: status.clone().into(),
            reference: reference.into(),
            settled_at: fact.settled_at.into(),
            updated_at: fact.updated_at.into(),
            bank_source: fact.bank_source.into(),
            source_reference: fact.source_reference.into(),
            narrative: fact.narrative.into(),
            ..Default::default()
        };
        let row = if let Some(old) = &old {
            write.update(self, old.id)?
        } else {
            TransferWrite {
                received_at: now.to_rfc3339().into(),
                ..write
            }
            .insert(self)?
        };
        row.audit(self, "payments.bank_fact", auth, old.as_ref())?;
        if status == TransferStatus::Reversed {
            TransferWrite {
                review_reason: Some(
                    "Bank reversal: review allocations, access and accounting".to_owned(),
                )
                .into(),
                ..Default::default()
            }
            .update(self, row.id)?;
        } else if old.as_ref().is_some_and(|old| old.status != status)
            && self.allocated(row.id)? > 0
        {
            TransferWrite{review_reason:Some("Bank changed the status of an allocated transfer; review its accounting and access".to_owned()).into(),..Default::default()}.update(self,row.id)?;
        }
        crate::service::commit(tx)?;
        Ok(row.id)
    }
    fn allocated(&self, id: i64) -> Result<i64> {
        Allocation::select(self, "WHERE transaction_id=?", &[&id])?
            .iter()
            .try_fold(0i64, |n, a| {
                n.checked_add(a.amount_minor)
                    .ok_or_else(|| invalid("Allocation overflow"))
            })
    }
    fn paid_amount(&self, id: i64, kind: Kind) -> Result<i64> {
        let sql = format!(
            "SELECT a.* FROM {} a JOIN {} t ON t.id=a.transaction_id WHERE a.payment_id=? AND a.kind=? AND t.status='settled'",
            quote(Allocation::TABLE),
            quote(Transfer::TABLE)
        );
        Allocation::query(self, &sql, &[&id, &kind])?
            .iter()
            .try_fold(0i64, |n, a| {
                n.checked_add(a.amount_minor)
                    .ok_or_else(|| invalid("Allocation overflow"))
            })
    }
    pub fn bank_transfers(&self, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.reconcile", auth)?;
        let items = Transfer::select(self, "ORDER BY id DESC LIMIT 100", &[])?
            .iter()
            .map(|r| self.transfer_view(r))
            .collect::<Result<Vec<_>>>()?;
        let sync = PaymentsSync::first(self, "ORDER BY id LIMIT 1", &[])?;
        Ok(json!({"items":items,"sync":sync,"unresolved_count":self.unresolved_transfer_count()?}))
    }
    fn transfer_view(&self, row: &Transfer) -> Result<Value> {
        let mut value = row.wire(self)?;
        value["unallocated_minor"] = json!(row.amount_minor - self.allocated(row.id)?);
        value["allocations"] = json!(Allocation::select(
            self,
            "WHERE transaction_id=? ORDER BY id",
            &[&row.id]
        )?);
        Ok(value)
    }
    pub fn reconcile_transfer(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.reconcile", auth)?;
        let row = Transfer::get(self, id)?;
        if row.disposition == Disposition::Excluded
            || row.direction != Direction::In
            || row.status == TransferStatus::Pending
            || (row.status == TransferStatus::Settled
                && (row.reference.is_empty() || self.allocated(id)? > 0))
        {
            return self.transfer_view(&row);
        }
        // Bank facts survive rejected business reactions. Every downstream effect
        // rolls back together; operators can inspect and retry the retained receipt.
        let result = (|| -> Result<()> {
            let tx = crate::service::transaction(&self.db)?;
            let mut before = row.clone();
            before.status = TransferStatus::Pending;
            self.cap_publish(
                Event::BankTransaction {
                    before,
                    after: row.clone(),
                },
                auth,
                instant(if row.status == TransferStatus::Reversed {
                    &row.updated_at
                } else {
                    row.settled_at
                        .as_deref()
                        .ok_or_else(|| invalid("Missing settlement time"))?
                })?,
            )?;
            TransferWrite {
                review_reason: None.into(),
                ..Default::default()
            }
            .update(self, id)?;
            crate::service::commit(tx)?;
            Ok(())
        })();
        if let Err(error) = result {
            TransferWrite {
                review_reason: Some(format!("{}: {}", error.code, error.message)).into(),
                ..Default::default()
            }
            .update(self, id)?;
        }
        self.transfer_view(&Transfer::get(self, id)?)
    }
    pub fn allocate_transfer(&self, id: i64, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.reconcile", auth)?;
        let input: Allocate = decode(input)?;
        let tx = crate::service::transaction(&self.db)?;
        let row = Transfer::get(self, id)?;
        let now = instant(
            row.settled_at
                .as_deref()
                .ok_or_else(|| invalid("Missing settlement time"))?,
        )?;
        self.cap_allocate_transfer(id, input.payment_id, input.amount_minor, auth, now)?;
        TransferWrite {
            review_reason: None.into(),
            ..Default::default()
        }
        .update(self, id)?;
        let response = self.transfer_view(&Transfer::get(self, id)?)?;
        crate::service::commit(tx)?;
        Ok(response)
    }
    pub(crate) fn cap_allocate_transfer(
        &self,
        transaction: i64,
        payment: i64,
        amount: i64,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let bank = Transfer::get(self, transaction)?;
        let old = Payment::get(self, payment)?;
        let kind = if bank.direction == Direction::In {
            Kind::Receipt
        } else {
            Kind::Refund
        };
        if let Some(previous) = Allocation::first(
            self,
            "WHERE transaction_id=? AND payment_id=?",
            &[&transaction, &payment],
        )? {
            if previous.amount_minor == amount && previous.kind == kind {
                return Ok(());
            }
            return Err(ServiceError::new(
                "conflict",
                "Allocation already exists with another amount",
            ));
        }
        if bank.disposition == Disposition::Excluded
            || bank.status != TransferStatus::Settled
            || bank.currency != old.currency
            || bank.account != old.bank_account
            || amount < 1
            || amount > bank.amount_minor - self.allocated(transaction)?
        {
            return Err(invalid(
                "Allocation requires confirmed, available funds in the request's account and currency",
            ));
        }
        let paid = self.paid_amount(payment, kind.clone())?;
        let status = match kind {
            Kind::Receipt => {
                if old.status != Status::Pending || amount > old.amount_minor - paid {
                    return Err(state("Request cannot accept this receipt"));
                }
                Status::Succeeded
            }
            Kind::Refund => {
                if old.status != Status::Succeeded || amount != old.amount_minor || paid != 0 {
                    return Err(state(
                        "Refund requires a separate confirmed full outgoing transfer",
                    ));
                }
                Status::Refunded
            }
        };
        let allocation = AllocationWrite {
            transaction_id: transaction.into(),
            payment_id: payment.into(),
            amount_minor: amount.into(),
            kind: kind.into(),
            created_at: now.to_rfc3339().into(),
            ..Default::default()
        }
        .insert(self)?;
        allocation.audit(self, "payments.allocate", auth, None)?;
        self.cap_publish(Event::PaymentAllocation { after: allocation }, auth, now)?;
        if paid + amount == old.amount_minor {
            // Backfilled receipts may be allocated out of order. Fulfilment is
            // funded at the latest receipt, never at an earlier allocation's date.
            let now = if status == Status::Succeeded {
                Allocation::select(self, "WHERE payment_id=? AND kind='receipt'", &[&payment])?
                    .iter()
                    .try_fold(now, |latest, row| -> Result<_> {
                        Ok(latest.max(instant(&row.created_at)?))
                    })?
            } else {
                now
            };
            let next = PaymentWrite {
                status: status.into(),
                ..Default::default()
            }
            .update(self, payment)?;
            next.audit(self, "payments.settle", auth, Some(&old))?;
            self.cap_publish(
                Event::Payment {
                    before: old,
                    after: next,
                },
                auth,
                now,
            )?;
        }
        Ok(())
    }
    pub(crate) fn cap_reverse_transfer(
        &self,
        id: i64,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let bank = Transfer::get(self, id)?;
        if bank.status != TransferStatus::Reversed || bank.direction != Direction::In {
            return Err(state("A confirmed incoming-bank reversal is required"));
        }
        for allocation in
            Allocation::select(self, "WHERE transaction_id=? AND kind='receipt'", &[&id])?
        {
            let old = Payment::get(self, allocation.payment_id)?;
            if old.status == Status::Refunded
                || old.status == Status::Pending
                || old.status == Status::Cancelled
            {
                continue;
            }
            if allocation.amount_minor != old.amount_minor {
                return Err(state(
                    "A reversed partial receipt requires accounting and access review",
                ));
            }
            let next = PaymentWrite {
                status: Status::Refunded.into(),
                ..Default::default()
            }
            .update(self, old.id)?;
            next.audit(self, "payments.bank_reversal", auth, Some(&old))?;
            self.cap_publish(
                Event::Payment {
                    before: old,
                    after: next,
                },
                auth,
                now,
            )?;
        }
        Ok(())
    }
}
