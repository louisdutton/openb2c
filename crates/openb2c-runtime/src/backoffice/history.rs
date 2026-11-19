//! Reviewed upgrade repair and reconstruction from typed durable source history.
use super::sales::{BillingTask, SaleFacts};
use super::*;
use crate::records::{
    AccountingJournal, BillingCredit, BillingInvoice, BillingRefund, BillingRefundWrite, Record,
};
impl Service {
    pub(crate) fn preserve_legacy_refunds(&self, old: &openb2c_schema::Schema) -> Result<()> {
        if old
            .capabilities
            .get("billing")
            .is_none_or(|b| b.entities.contains_key("refund"))
            || !self.cap_enabled("billing")
        {
            return Ok(());
        }
        let mut statement=self.db.prepare(&format!("SELECT i.id AS invoice_id,j.id AS journal_id,j.date FROM {} i JOIN {} c ON c.invoice_id=i.id JOIN {} j ON j.source_key='refund:'||i.id",quote(BillingInvoice::TABLE),quote(BillingCredit::TABLE),quote(AccountingJournal::TABLE)))?;
        let historical = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (invoice, journal, date) in historical {
            if BillingRefund::first(self, "WHERE invoice_id=?", &[&invoice])?.is_some() {
                continue;
            }
            let saved = BillingRefundWrite {
                invoice_id: invoice.into(),
                journal_id: journal.into(),
                date: date.into(),
                reason: "Preserved refund already recorded by billing version 1"
                    .to_owned()
                    .into(),
                ..Default::default()
            }
            .insert(self)?;
            saved.audit(self, "billing.migrate_refund", &AuthContext::system(), None)?;
        }
        Ok(())
    }
    #[cfg(not(feature = "cap_payments"))]
    pub(crate) fn backfill_billing(&self, _old: &openb2c_schema::Schema) -> anyhow::Result<()> {
        Ok(())
    }
    #[cfg(feature = "cap_payments")]
    pub(crate) fn backfill_billing(&self, old: &openb2c_schema::Schema) -> anyhow::Result<()> {
        use crate::records::{
            PaymentsEvent, PaymentsEventOutcome, PaymentsPayment, PaymentsPaymentStatus,
        };
        use chrono::Months;
        if self.cap_enabled("billing_payments")
            && !old.capabilities.contains_key("billing_payments")
        {
            self.backfill_allocated_cash()?;
        }
        let activate_orders =
            self.cap_enabled("billing_orders") && !old.capabilities.contains_key("billing_orders");
        let activate_subscriptions = self.cap_enabled("billing_subscriptions")
            && !old.capabilities.contains_key("billing_subscriptions");
        if !activate_orders && !activate_subscriptions {
            return Ok(());
        }
        for payment in PaymentsPayment::select(
            self,
            "WHERE status IN ('succeeded','refunded') ORDER BY id",
            &[],
        )? {
            let mut target: Option<(String, SaleFacts, String, Option<(String, String)>)> = None;
            #[cfg(feature = "cap_order_payments")]
            if activate_orders {
                use crate::records::{OrderPaymentsLink, OrdersOrder};
                if let Some(link) =
                    OrderPaymentsLink::first(self, "WHERE payment_id=?", &[&payment.id])?
                {
                    let order = OrdersOrder::get(self, link.order_id)?;
                    target = Some((
                        format!("order:{}", order.id),
                        SaleFacts {
                            user_id: order.user_id,
                            amount_minor: order.amount_minor,
                            currency: order.currency,
                        },
                        format!("Historical order {}", link.order_id),
                        None,
                    ));
                }
            }
            #[cfg(feature = "cap_subscriptions")]
            if activate_subscriptions {
                use crate::records::{SubscriptionsAttempt, SubscriptionsSubscription};
                if let Some(link) =
                    SubscriptionsAttempt::first(self, "WHERE payment_id=?", &[&payment.id])?
                {
                    let sub = SubscriptionsSubscription::get(self, link.subscription_id)?;
                    let anchor = crate::b2c::instant(
                        sub.started_at
                            .as_deref()
                            .ok_or_else(|| invalid("Missing subscription anchor"))?,
                    )?;
                    let cycle = u32::try_from(link.cycle)?;
                    let factor = if sub.billing_interval.as_str() == "year" {
                        12u32
                    } else {
                        1
                    };
                    let start = anchor
                        .checked_add_months(Months::new(
                            cycle
                                .checked_mul(factor)
                                .ok_or_else(|| invalid("Billing cycle overflow"))?,
                        ))
                        .ok_or_else(|| invalid("Billing date overflow"))?;
                    let end = anchor
                        .checked_add_months(Months::new(
                            cycle
                                .checked_add(1)
                                .and_then(|n| n.checked_mul(factor))
                                .ok_or_else(|| invalid("Billing cycle overflow"))?,
                        ))
                        .ok_or_else(|| invalid("Billing date overflow"))?;
                    target = Some((
                        format!("subscription-payment:{}", payment.id),
                        SaleFacts {
                            user_id: payment.user_id,
                            amount_minor: payment.amount_minor,
                            currency: payment.currency.clone(),
                        },
                        format!(
                            "Historical subscription {} cycle {cycle}",
                            link.subscription_id
                        ),
                        Some((start.date_naive().to_string(), end.date_naive().to_string())),
                    ));
                }
            }
            let Some((source, sale, summary, period)) = target else {
                continue;
            };
            let events = PaymentsEvent::select(
                self,
                "WHERE payment_id=? AND outcome IN ('succeeded','refunded') ORDER BY id",
                &[&payment.id],
            )?;
            let (settled, refunded) = if payment.provider.as_str() == "bank_transfer" {
                use crate::records::{
                    PaymentsAllocation, PaymentsAllocationKind as Kind, PaymentsTransaction,
                    PaymentsTransactionDirection as Direction, PaymentsTransactionStatus as Status,
                };
                let allocations = PaymentsAllocation::select(
                    self,
                    "WHERE payment_id=? ORDER BY created_at,id",
                    &[&payment.id],
                )?;
                let mut total = 0i64;
                let mut settled = None;
                let mut refunded = None;
                for allocation in allocations {
                    let bank = PaymentsTransaction::get(self, allocation.transaction_id)?;
                    if bank.account != payment.bank_account
                        || bank.currency != payment.currency
                        || bank.status == Status::Pending
                        || allocation.amount_minor > bank.amount_minor
                    {
                        return Err(
                            invalid("Historical allocation has no matching bank evidence").into(),
                        );
                    }
                    if allocation.kind == Kind::Receipt {
                        if bank.status == Status::Reversed
                            && allocation.amount_minor < payment.amount_minor
                        {
                            continue;
                        }
                        if bank.direction != Direction::In {
                            return Err(invalid("Receipt has no incoming bank evidence").into());
                        }
                        total = total
                            .checked_add(allocation.amount_minor)
                            .ok_or_else(|| invalid("Historical receipt overflow"))?;
                        if total == payment.amount_minor {
                            settled = Some(allocation.created_at);
                        }
                        if bank.status == Status::Reversed
                            && allocation.amount_minor == payment.amount_minor
                        {
                            refunded = Some(bank.updated_at);
                        }
                    } else {
                        if bank.direction != Direction::Out
                            || bank.status != Status::Settled
                            || allocation.amount_minor != payment.amount_minor
                        {
                            return Err(invalid("Refund has no full outgoing bank evidence").into());
                        }
                        refunded = Some(allocation.created_at);
                    }
                }
                if total != payment.amount_minor {
                    return Err(
                        invalid("Historical receipts do not equal the payment request").into(),
                    );
                }
                (
                    settled
                        .ok_or_else(|| invalid("Historical payment has no settlement evidence"))?,
                    refunded,
                )
            } else {
                let settled=events.iter().find(|e|e.outcome==PaymentsEventOutcome::Succeeded).ok_or_else(||invalid(format!("Historical payment {} has no settlement evidence; supply reviewed source history before enabling billing",payment.id)))?;
                (
                    settled.created_at.clone(),
                    events
                        .iter()
                        .find(|e| e.outcome == PaymentsEventOutcome::Refunded)
                        .map(|e| e.created_at.clone()),
                )
            };
            let when = crate::b2c::instant(&settled)?;
            let mut payload = self.sale_task(&source, &sale, &summary, period, when)?;
            if let BillingTask::Sale {
                invoice,
                inclusive_vat,
                historical_backfill,
                cash_account,
                ..
            } = &mut payload
            {
                invoice.tax_code = "unassessed".into();
                invoice.net_minor = sale.amount_minor;
                *inclusive_vat = 0;
                *historical_backfill = true;
                if self.cap_enabled("billing_payments")
                    && payment.provider.as_str() == "bank_transfer"
                {
                    *cash_account = Some("customer_deposits".into());
                }
            }
            self.billing_queue(payload, &AuthContext::system(), when)?;
            if payment.status == PaymentsPaymentStatus::Refunded {
                let refund =
                    refunded.ok_or_else(|| invalid("Historical refund has no source evidence"))?;
                self.billing_queue(
                    BillingTask::Refund {
                        source,
                        cash_account: (self.cap_enabled("billing_payments")
                            && payment.provider.as_str() == "bank_transfer")
                            .then(|| "customer_deposits".into()),
                        historical_backfill: true,
                    },
                    &AuthContext::system(),
                    crate::b2c::instant(&refund)?,
                )?;
            }
        }
        Ok(())
    }
    /// Reconstruct allocated cash only during the reviewed consumer activation.
    /// Existing invoice cash journals remain authoritative and are never reposted.
    #[cfg(feature = "cap_payments")]
    fn backfill_allocated_cash(&self) -> Result<()> {
        use crate::records::{
            AccountingAccount, AccountingLine, BillingReceipt, PaymentsAllocation,
            PaymentsAllocationKind as Kind, PaymentsPayment, PaymentsTransaction,
            PaymentsTransactionStatus as Status,
        };
        let auth = AuthContext::system();
        for allocation in PaymentsAllocation::select(self, "ORDER BY created_at,id", &[])? {
            let payment = PaymentsPayment::get(self, allocation.payment_id)?;
            let bank = PaymentsTransaction::get(self, allocation.transaction_id)?;
            if bank.account != payment.bank_account
                || bank.currency != payment.currency
                || bank.status == Status::Pending
                || allocation.amount_minor <= 0
                || allocation.amount_minor > bank.amount_minor
                || (allocation.kind == Kind::Receipt) != (bank.direction.as_str() == "in")
            {
                return Err(invalid(
                    "Historical allocation has no matching bank evidence",
                ));
            }
            let mut source: Option<String> = None;
            #[cfg(feature = "cap_order_payments")]
            if let Some(link) = crate::records::OrderPaymentsLink::first(
                self,
                "WHERE payment_id=?",
                &[&payment.id],
            )? {
                source = Some(format!("order:{}", link.order_id));
            }
            #[cfg(feature = "cap_subscriptions")]
            if crate::records::SubscriptionsAttempt::first(
                self,
                "WHERE payment_id=?",
                &[&payment.id],
            )?
            .is_some()
            {
                source = Some(format!("subscription-payment:{}", payment.id));
            }
            if let Some(source) = &source {
                use crate::records::{BillingEvent, BillingEventStatus, BillingEventWrite};
                for kind in ["sale", "refund"] {
                    if let Some(event) = BillingEvent::first(
                        self,
                        "WHERE source_key=? AND status='pending'",
                        &[&format!("{kind}:{source}")],
                    )? {
                        let mut task: BillingTask = serde_json::from_str(&event.payload_json)
                            .map_err(|_| invalid("Invalid saved billing event"))?;
                        if let BillingTask::Sale { cash_account, .. }
                        | BillingTask::Refund { cash_account, .. } = &mut task
                        {
                            *cash_account = Some("customer_deposits".into());
                        }
                        let next = BillingEventWrite {
                            payload_json: serde_json::to_string(&task)
                                .map_err(|e| invalid(e.to_string()))?
                                .into(),
                            status: BillingEventStatus::Pending.into(),
                            ..Default::default()
                        }
                        .update(self, event.id)?;
                        next.audit(self, "billing.migrate_cash", &auth, Some(&event))?;
                    }
                }
            }
            let invoice = source
                .as_ref()
                .map(|source| BillingInvoice::first(self, "WHERE source_key=?", &[source]))
                .transpose()?
                .flatten();
            let mut legacy_receipt = false;
            let mut legacy_refund = false;
            if let Some(invoice) = invoice {
                if let Some(receipt) =
                    BillingReceipt::first(self, "WHERE invoice_id=?", &[&invoice.id])?
                {
                    for line in AccountingLine::select(
                        self,
                        "WHERE journal_id=? AND debit>0",
                        &[&receipt.journal_id],
                    )? {
                        let account = AccountingAccount::get(self, line.account_id)?;
                        legacy_receipt |= account.code == "bank" || account.code == "clearing";
                    }
                }
                legacy_refund =
                    BillingRefund::first(self, "WHERE invoice_id=?", &[&invoice.id])?.is_some();
            }
            let day = bank
                .settled_at
                .as_deref()
                .ok_or_else(|| invalid("Historical allocation lacks settlement evidence"))?;
            let when = crate::b2c::instant(day)?;
            let receipt = allocation.kind == Kind::Receipt;
            if !(if receipt {
                legacy_receipt
            } else {
                legacy_refund
            }) {
                self.capture_cash(
                    &format!("allocation:{}", allocation.id),
                    allocation.amount_minor,
                    &payment.currency,
                    if receipt { "bank" } else { "customer_deposits" },
                    if receipt { "customer_deposits" } else { "bank" },
                    &auth,
                    when,
                )?;
            }
            if receipt && bank.status == Status::Reversed && !legacy_refund {
                self.capture_cash(
                    &format!("allocation-reversal:{}", allocation.id),
                    allocation.amount_minor,
                    &payment.currency,
                    "customer_deposits",
                    "bank",
                    &auth,
                    crate::b2c::instant(&bank.updated_at)?,
                )?;
            }
        }
        Ok(())
    }
}
