//! Starling's account-holder API. Only GET requests are implemented.
use super::bank_feed::{BankFeed, read_response};
use super::transfers::BankFact;
use super::*;
use crate::records::{PaymentsSync, PaymentsSyncWrite, PaymentsTransactionSource, Record};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Amount {
    currency: String,
    minor_units: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedItem {
    feed_item_uid: String,
    amount: Amount,
    direction: String,
    status: String,
    source: String,
    #[serde(default)]
    reference: String,
    settlement_time: Option<String>,
    updated_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feed {
    feed_items: Vec<FeedItem>,
}
impl FeedItem {
    fn fact(self, account: &str) -> Result<Option<BankFact>> {
        if !matches!(
            self.source.as_str(),
            "FASTER_PAYMENTS_IN"
                | "FASTER_PAYMENTS_OUT"
                | "FASTER_PAYMENTS_REVERSAL"
                | "FASTER_PAYMENTS_REFUND"
        ) {
            return Ok(None);
        }
        if self.status == "ACCOUNT_CHECK" {
            return Ok(None);
        }
        Ok(Some(BankFact {
            transaction_id: self.feed_item_uid,
            account: account.into(),
            amount_minor: self.amount.minor_units,
            currency: self.amount.currency,
            direction: match self.direction.as_str() {
                "IN" => "in",
                "OUT" => "out",
                _ => return Err(invalid("Unknown bank transfer direction")),
            }
            .into(),
            status: match self.status.as_str() {
                "SETTLED" => "settled",
                "UPCOMING" | "PENDING" | "RETRYING" => "pending",
                "REVERSED" | "DECLINED" | "REFUNDED" | "UPCOMING_CANCELLED" => "reversed",
                _ => return Err(invalid("Unknown bank transfer status")),
            }
            .into(),
            source_reference: self.reference.clone(),
            narrative: String::new(),
            reference: self.reference,
            settled_at: self.settlement_time,
            updated_at: self.updated_at,
            bank_source: self.source,
        }))
    }
}
pub(super) struct Starling {
    account: String,
    category: String,
    token: String,
    base: &'static str,
    client: reqwest::blocking::Client,
}
impl BankFeed for Starling {
    const SOURCE: PaymentsTransactionSource = PaymentsTransactionSource::Starling;
    fn configured(account: String) -> Result<Self> {
        let category = std::env::var("STARLING_CATEGORY_UID")
            .map_err(|_| invalid("Missing STARLING_CATEGORY_UID"))?;
        let uuid = |v: &str| {
            v.len() == 36
                && v.bytes().enumerate().all(|(i, c)| {
                    if [8, 13, 18, 23].contains(&i) {
                        c == b'-'
                    } else {
                        c.is_ascii_hexdigit()
                    }
                })
        };
        if !uuid(&account) || !uuid(&category) {
            return Err(invalid("Starling account and category must be UUIDs"));
        }
        let token = std::env::var("STARLING_ACCESS_TOKEN")
            .map_err(|_| invalid("Missing STARLING_ACCESS_TOKEN"))?;
        let base = match std::env::var("STARLING_ENVIRONMENT").as_deref() {
            Ok("production") => "https://api.starlingbank.com",
            Ok("sandbox") | Err(_) => "https://api-sandbox.starlingbank.com",
            _ => {
                return Err(invalid(
                    "STARLING_ENVIRONMENT must be sandbox or production",
                ));
            }
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| invalid("Cannot build bank client"))?;
        Ok(Self {
            account,
            category,
            token,
            base,
            client,
        })
    }
    fn read(&self, since: &str, mut accept: impl FnMut(BankFact) -> Result<()>) -> Result<()> {
        let feed = fetch_feed(
            &self.client,
            &format!(
                "{}/api/v2/feed/account/{}/category/{}",
                self.base, self.account, self.category
            ),
            &self.token,
            since,
        )?;
        for item in feed.feed_items {
            if let Some(fact) = item.fact(&self.account)? {
                accept(fact)?;
            }
        }
        Ok(())
    }
    fn checkpoint(started: DateTime<Utc>) -> Option<String> {
        Some((started - chrono::Duration::minutes(2)).to_rfc3339())
    }
}
impl Service {
    /// Webhooks are authenticated wake-up hints. Facts are fetched from the bank,
    /// so payload-controlled account identifiers can never redirect reconciliation.
    pub fn starling_webhook(&self, bytes: &[u8], signature: &str) -> Result<Value> {
        if self.cap_setting("payments", "feed") != Some("starling") {
            return Err(ServiceError::new(
                "not_found",
                "Bank webhook is not enabled",
            ));
        }
        let public = std::env::var("STARLING_WEBHOOK_PUBLIC_KEY")
            .map_err(|_| invalid("Missing STARLING_WEBHOOK_PUBLIC_KEY"))?;
        verify_bank_signature(&public, bytes, signature)?;
        // Persist the hint quickly; the regular worker polls independently as well.
        if let Some(row) = PaymentsSync::first(self, "ORDER BY id LIMIT 1", &[])? {
            PaymentsSyncWrite {
                requested: 1.into(),
                ..Default::default()
            }
            .update(self, row.id)?;
        }
        Ok(json!({"accepted":true}))
    }
}

fn fetch_feed(
    client: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    since: &str,
) -> Result<Feed> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .header("user-agent", "business-software/0.1")
        .query(&[("changesSince", since)])
        .send()
        .map_err(|_| ServiceError::new("unavailable", "Bank feed request failed"))?;
    serde_json::from_slice(&read_response(response)?)
        .map_err(|_| invalid("Invalid bank feed response"))
}

fn verify_bank_signature(public: &str, bytes: &[u8], signature: &str) -> Result<()> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rsa::{
        pkcs1::DecodeRsaPublicKey, pkcs8::DecodePublicKey, signature::Verifier,
        traits::PublicKeyParts,
    };
    if public.len() > 16_384 {
        return Err(invalid("Invalid bank webhook public key"));
    }
    let der = STANDARD
        .decode(public.split_whitespace().collect::<String>())
        .ok();
    let public = rsa::RsaPublicKey::from_public_key_pem(public)
        .ok()
        .or_else(|| rsa::RsaPublicKey::from_pkcs1_pem(public).ok())
        .or_else(|| rsa::RsaPublicKey::from_public_key_der(der.as_deref().unwrap_or(&[])).ok())
        .ok_or_else(|| invalid("Invalid bank webhook public key"))?;
    if !(2048..=8192).contains(&public.n().bits()) {
        return Err(invalid("Bank webhook key must be 2048–8192 bits"));
    }
    let key = rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(public);
    let signature = STANDARD
        .decode(signature)
        .ok()
        .and_then(|v| rsa::pkcs1v15::Signature::try_from(v.as_slice()).ok())
        .ok_or_else(|| ServiceError::new("unauthorized", "Invalid bank signature"))?;
    key.verify(bytes, &signature)
        .map_err(|_| ServiceError::new("unauthorized", "Invalid bank signature"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feed_transport_only_reads_and_does_not_follow_redirects() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for status in [200, 302] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                let mut b = [0u8; 1];
                while !bytes.ends_with(b"\r\n\r\n") {
                    assert!(bytes.len() < 8192);
                    stream.read_exact(&mut b).unwrap();
                    bytes.push(b[0]);
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with(
                    "GET /api/v2/feed/account/account/category/category?changesSince="
                ));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-read-only-token\r\n")
                );
                let body = r#"{"feedItems":[]}"#;
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1:1/never\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let url = format!("http://{address}/api/v2/feed/account/account/category/category");
        assert!(
            fetch_feed(
                &client,
                &url,
                "test-read-only-token",
                "2025-01-01T12:00:00Z"
            )
            .unwrap()
            .feed_items
            .is_empty()
        );
        assert_eq!(
            fetch_feed(
                &client,
                &url,
                "test-read-only-token",
                "2025-01-01T12:00:00Z"
            )
            .err()
            .unwrap()
            .code,
            "unavailable"
        );
        server.join().unwrap();
    }
    #[test]
    fn normalizes_only_known_bank_transfer_facts() {
        let item = |source: &str, status: &str| {
            serde_json::from_value::<FeedItem>(json!({"feedItemUid":"bank-id","amount":{"currency":"GBP","minorUnits":1500},"direction":"IN","status":status,"source":source,"reference":"P123","settlementTime":"2025-01-01T12:00:00Z","updatedAt":"2025-01-01T12:00:00Z"})).unwrap()
        };
        let fact = item("FASTER_PAYMENTS_IN", "SETTLED")
            .fact("account-id")
            .unwrap()
            .unwrap();
        assert_eq!(fact.account, "account-id");
        assert_eq!(fact.direction, "in");
        assert_eq!(fact.amount_minor, 1500);
        assert!(
            item("MASTER_CARD", "SETTLED")
                .fact("account-id")
                .unwrap()
                .is_none()
        );
        assert!(
            item("FASTER_PAYMENTS_IN", "NEW_STATUS")
                .fact("account-id")
                .is_err()
        );
        assert_eq!(
            item("FASTER_PAYMENTS_IN", "REVERSED")
                .fact("account-id")
                .unwrap()
                .unwrap()
                .status,
            "reversed"
        );
    }
    #[test]
    fn signatures_cover_the_original_bytes() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use rsa::{
            pkcs8::EncodePublicKey,
            signature::{SignatureEncoding, Signer},
        };
        let private = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let public = rsa::RsaPublicKey::from(&private);
        let pem = public.to_public_key_pem(Default::default()).unwrap();
        let body = br#"{"webhookEventUid":"example","content":{"amount":1500}}"#;
        let key = rsa::pkcs1v15::SigningKey::<sha2::Sha512>::new(private);
        let signature = STANDARD.encode(key.sign(body).to_bytes());
        verify_bank_signature(&pem, body, &signature).unwrap();
        let der = STANDARD.encode(public.to_public_key_der().unwrap().as_bytes());
        verify_bank_signature(&der, body, &signature).unwrap();
        assert!(verify_bank_signature(&pem, b"{}", &signature).is_err());
        assert!(verify_bank_signature(&pem, body, "invalid").is_err());
    }
}
