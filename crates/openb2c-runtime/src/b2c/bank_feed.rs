//! Shared read-only feed lifecycle. The generator binds one concrete adapter.
use super::transfers::BankFact;
use super::*;
use crate::records::{PaymentsSync, PaymentsSyncWrite, PaymentsTransactionSource, Record};
use serde::Deserialize;

/// Protocol adapters supply facts, never business decisions or payment initiation.
/// Each fact is committed before its compiled business reaction is attempted.
pub(super) trait BankFeed: Sized {
    const SOURCE: PaymentsTransactionSource;
    fn configured(account: String) -> Result<Self>;
    fn read(&self, since: &str, accept: impl FnMut(BankFact) -> Result<()>) -> Result<()>;
    /// None means replay the configured booking window, not an update cursor.
    fn checkpoint(started: DateTime<Utc>) -> Option<String>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sync {
    changes_since: String,
}

impl Service {
    pub(crate) fn poll_bank_feed(&self, now: DateTime<Utc>) -> Result<()> {
        let row = if let Some(row) = PaymentsSync::first(self, "ORDER BY id LIMIT 1", &[])? {
            row
        } else {
            let from = std::env::var("BANK_SYNC_FROM")
                .map_err(|_| invalid("Missing BANK_SYNC_FROM initial reconciliation timestamp"))?;
            instant(&from)?;
            PaymentsSyncWrite {
                changes_since: from.into(),
                next_poll_at: now.to_rfc3339().into(),
                requested: 0.into(),
                ..Default::default()
            }
            .insert(self)?
        };
        if instant(&row.next_poll_at)? > now
            && (row.requested != 1
                || instant(&row.next_poll_at)? - chrono::Duration::seconds(25) > now)
        {
            return Ok(());
        }
        PaymentsSyncWrite {
            next_poll_at: (now + chrono::Duration::seconds(30)).to_rfc3339().into(),
            requested: 0.into(),
            ..Default::default()
        }
        .update(self, row.id)?;
        match self.sync_bank_feed(
            &json!({"changes_since":row.changes_since}),
            &AuthContext::system(),
        ) {
            Ok(_) => {
                PaymentsSyncWrite {
                    changes_since: SelectedBankFeed::checkpoint(now)
                        .unwrap_or(row.changes_since)
                        .into(),
                    last_error: None.into(),
                    ..Default::default()
                }
                .update(self, row.id)?;
            }
            Err(error) => {
                PaymentsSyncWrite {
                    last_error: Some(error.to_string()).into(),
                    ..Default::default()
                }
                .update(self, row.id)?;
            }
        }
        Ok(())
    }

    pub fn sync_bank_feed(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.import", auth)?;
        let input: Sync = decode(input)?;
        let since = instant(&input.changes_since)?;
        if since > Utc::now() {
            return Err(invalid("Bank scan cannot start in the future"));
        }
        let feed = SelectedBankFeed::configured(self.transfer_destination()?.account)?;
        self.ingest_bank_feed(&feed, &input.changes_since, auth)
    }

    fn ingest_bank_feed<F: BankFeed>(
        &self,
        feed: &F,
        since: &str,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command("payments", "payments.import", auth)?;
        let mut imported = Vec::new();
        feed.read(since, |fact| {
            let id = self.record_bank_fact(fact, F::SOURCE, auth, Utc::now())?;
            imported.push(self.reconcile_transfer(id, &AuthContext::system())?);
            Ok(())
        })?;
        Ok(json!({"items":imported,"changes_since":since}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct InterruptedFeed(bool);
    impl BankFeed for InterruptedFeed {
        const SOURCE: PaymentsTransactionSource = SelectedBankFeed::SOURCE;
        fn configured(_: String) -> Result<Self> {
            Ok(Self(false))
        }
        fn checkpoint(_: DateTime<Utc>) -> Option<String> {
            None
        }
        fn read(&self, _: &str, mut accept: impl FnMut(BankFact) -> Result<()>) -> Result<()> {
            accept(decode(
                &json!({"transaction_id":"retained-bank-id", "account":"account",
                "amount_minor":1500,"currency":"GBP","direction":"in","status":"settled",
                "reference":"unmatched", "source_reference":"bank-fpid", "narrative":"Sender reference",
                "settled_at":"2025-01-01T12:00:00Z", "updated_at":"2025-01-01T12:00:00Z"}),
            )?)?;
            if self.0 {
                return Err(ServiceError::new("unavailable", "Later page failed"));
            }
            Ok(())
        }
    }
    #[test]
    fn facts_survive_interrupted_reads_and_replay_without_duplicate_money() {
        let schema = serde_json::from_str(include_str!("../../../../schema.json")).unwrap();
        let service = Service::open(schema, ":memory:", false).unwrap();
        let auth = AuthContext::system();
        let since = "2025-01-01T00:00:00Z";
        let rejected = service
            .sync_bank_feed(&json!({"changes_since":since}), &AuthContext::default())
            .unwrap_err();
        assert!(["forbidden", "unauthorized"].contains(&rejected.code));
        assert_eq!(service.unresolved_transfer_count().unwrap(), 0);
        assert!(
            service
                .ingest_bank_feed(&InterruptedFeed(true), since, &auth)
                .is_err()
        );
        assert_eq!(service.unresolved_transfer_count().unwrap(), 1);
        service
            .ingest_bank_feed(&InterruptedFeed(false), since, &auth)
            .unwrap();
        let transfers = service.bank_transfers(&auth).unwrap();
        assert_eq!(transfers["items"].as_array().unwrap().len(), 1);
        assert_eq!(transfers["items"][0]["source_reference"], "bank-fpid");
        assert_eq!(transfers["items"][0]["narrative"], "Sender reference");
        assert_eq!(transfers["items"][0]["unallocated_minor"], 1500);
    }
}

pub(super) fn read_response(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    if !response.status().is_success() {
        return Err(ServiceError::new(
            "unavailable",
            format!("Bank feed returned {}", response.status().as_u16()),
        ));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .take(4_000_001)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("Cannot read bank feed"))?;
    if bytes.len() > 4_000_000 {
        return Err(invalid(
            "Bank feed exceeds response limit; manual reconciliation is required",
        ));
    }
    Ok(bytes)
}
