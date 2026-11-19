//! UK Open Banking AIS transaction reader (v4 and v3.1 wire status codes).
//! Consent and token acquisition belong to the deployment's authorised AIS client.
use super::bank_feed::{BankFeed, read_response};
use super::transfers::BankFact;
use super::*;
use crate::records::PaymentsTransactionSource;
use reqwest::{Url, blocking::Client};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

pub(super) struct OpenBanking {
    client: Client,
    endpoint: Url,
    account: String,
    token: String,
}

impl BankFeed for OpenBanking {
    const SOURCE: PaymentsTransactionSource = PaymentsTransactionSource::OpenBanking;
    fn configured(account: String) -> Result<Self> {
        let required = |name| {
            std::env::var(name)
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid(format!("Missing {name}")))
        };
        let endpoint = transaction_endpoint(&required("OPEN_BANKING_BASE_URL")?, &account)?;
        // Read on every scan so an external consent client can rotate credentials.
        let token = std::fs::read_to_string(required("OPEN_BANKING_ACCESS_TOKEN_FILE")?)
            .map_err(|_| invalid("Cannot read Open Banking access token file"))?
            .trim()
            .to_owned();
        if token.is_empty() || token.chars().any(char::is_control) {
            return Err(invalid("Invalid Open Banking access token"));
        }
        let pem = std::fs::read(required("OPEN_BANKING_CLIENT_IDENTITY_FILE")?)
            .map_err(|_| invalid("Cannot read Open Banking client identity"))?;
        let identity = reqwest::Identity::from_pem(&pem)
            .map_err(|_| invalid("Invalid Open Banking client identity PEM"))?;
        let mut builder = Client::builder()
            .identity(identity)
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15));
        if let Some(path) = std::env::var_os("OPEN_BANKING_CA_CERT_FILE") {
            let pem = std::fs::read(path)
                .map_err(|_| invalid("Cannot read Open Banking CA certificate"))?;
            let certificate = reqwest::Certificate::from_pem(&pem)
                .map_err(|_| invalid("Invalid Open Banking CA certificate PEM"))?;
            builder = builder.add_root_certificate(certificate);
        }
        let client = builder
            .build()
            .map_err(|_| invalid("Cannot build Open Banking client"))?;
        Ok(Self {
            client,
            endpoint,
            account,
            token,
        })
    }
    fn read(&self, since: &str, accept: impl FnMut(BankFact) -> Result<()>) -> Result<()> {
        fetch_transactions(
            &self.client,
            &self.endpoint,
            &self.token,
            &self.account,
            since,
            accept,
        )
    }
    fn checkpoint(_: DateTime<Utc>) -> Option<String> {
        // BookingDateTime is not a revision timestamp. Replay the fixed window;
        // advancing a changes-since cursor would silently miss later postings.
        None
    }
}

fn transaction_endpoint(base: &str, account: &str) -> Result<Url> {
    let mut url = Url::parse(base).map_err(|_| invalid("Invalid OPEN_BANKING_BASE_URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || account.is_empty()
        || account.len() > 40
        || account.chars().any(char::is_control)
        || account == "."
        || account == ".."
    {
        return Err(invalid(
            "Open Banking requires an HTTPS API base without credentials, query or fragment",
        ));
    }
    url.path_segments_mut()
        .map_err(|_| invalid("Invalid Open Banking API base"))?
        .pop_if_empty()
        .push("accounts")
        .push(account)
        .push("transactions");
    Ok(url)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Page {
    data: Data,
    links: Links,
    meta: Meta,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Data {
    #[serde(default)]
    transaction: Vec<Transaction>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Links {
    next: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Meta {
    total_pages: Option<usize>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Amount {
    amount: String,
    currency: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Transaction {
    account_id: String,
    transaction_id: Option<String>,
    transaction_reference: Option<String>,
    transaction_information: Option<String>,
    credit_debit_indicator: String,
    status: String,
    transaction_mutability: Option<String>,
    booking_date_time: String,
    amount: Amount,
}

impl Transaction {
    fn fact(self, account: &str) -> Result<Option<BankFact>> {
        if self.account_id != account {
            return Err(invalid("Open Banking returned another account"));
        }
        match self.status.as_str() {
            // Pending amounts and identifiers can change. They are not cash evidence.
            "PDNG" | "Pending" => return Ok(None),
            "BOOK" | "Booked" => {}
            _ => return Err(invalid("Unsupported Open Banking transaction status")),
        }
        if self
            .transaction_mutability
            .as_deref()
            .is_some_and(|v| v != "Immutable")
        {
            return Err(invalid(
                "Mutable booked Open Banking transaction requires review",
            ));
        }
        let transaction_id = self
            .transaction_id
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                invalid("Open Banking booked transaction needs a stable TransactionId")
            })?;
        let booked = instant(&self.booking_date_time)?.to_rfc3339();
        // The generator supplies this choice from Nix, never a runtime mapping.
        let reference = match super::OPEN_BANKING_REFERENCE_FIELD {
            "transaction_information" => self.transaction_information.clone(),
            "transaction_reference" => self.transaction_reference.clone(),
            _ => None,
        }
        .unwrap_or_default();
        Ok(Some(BankFact {
            transaction_id,
            account: account.into(),
            amount_minor: gbp_minor(&self.amount)?,
            currency: self.amount.currency,
            direction: match self.credit_debit_indicator.as_str() {
                "Credit" => "in",
                "Debit" => "out",
                _ => return Err(invalid("Unknown Open Banking direction")),
            }
            .into(),
            status: "settled".into(),
            reference,
            settled_at: Some(booked.clone()),
            updated_at: booked,
            bank_source: "UK_OB_AIS".into(),
            source_reference: self.transaction_reference.unwrap_or_default(),
            narrative: self.transaction_information.unwrap_or_default(),
        }))
    }
}

fn gbp_minor(amount: &Amount) -> Result<i64> {
    if amount.currency != "GBP" {
        return Err(invalid("Only GBP bank facts are supported"));
    }
    let (whole, fraction) = amount
        .amount
        .split_once('.')
        .unwrap_or((&amount.amount, ""));
    if whole.is_empty()
        || amount.amount.ends_with('.')
        || whole.len() > 13
        || fraction.len() > 5
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
        || fraction.bytes().skip(2).any(|b| b != b'0')
    {
        return Err(invalid("Bank amount is not an exact GBP penny amount"));
    }
    let pennies = fraction
        .bytes()
        .take(2)
        .chain(std::iter::repeat(b'0'))
        .take(2)
        .fold(0i64, |n, b| n * 10 + i64::from(b - b'0'));
    let minor = whole
        .parse::<i64>()
        .ok()
        .and_then(|v| v.checked_mul(100))
        .and_then(|v| v.checked_add(pennies))
        .filter(|n| (1..=1_000_000_000_000).contains(n))
        .ok_or_else(|| invalid("Bank amount is out of range"))?;
    Ok(minor)
}

fn next_page(endpoint: &Url, current: &Url, link: &str) -> Result<Url> {
    let next = current
        .join(link)
        .map_err(|_| invalid("Invalid bank pagination link"))?;
    if next.origin() != endpoint.origin()
        || !next.username().is_empty()
        || next.password().is_some()
        || next.fragment().is_some()
        || next.path().trim_end_matches('/') != endpoint.path().trim_end_matches('/')
    {
        return Err(invalid(
            "Bank pagination must stay on the configured account endpoint",
        ));
    }
    Ok(next)
}

fn fetch_transactions(
    client: &Client,
    endpoint: &Url,
    token: &str,
    account: &str,
    since: &str,
    mut accept: impl FnMut(BankFact) -> Result<()>,
) -> Result<()> {
    instant(since)?;
    let mut url = endpoint.clone();
    url.query_pairs_mut()
        .append_pair("fromBookingDateTime", since);
    let mut visited = BTreeSet::new();
    let started = Instant::now();
    let mut facts = 0usize;
    for page_number in 1..=100 {
        if started.elapsed() > Duration::from_secs(20) || !visited.insert(url.to_string()) {
            return Err(invalid(
                "Bank pagination time limit or repeated page; scan incomplete",
            ));
        }
        let response = client
            .get(url.clone())
            .bearer_auth(token)
            .header("accept", "application/json")
            .header("user-agent", "business-software/0.1")
            .send()
            .map_err(|_| ServiceError::new("unavailable", "Open Banking feed request failed"))?;
        let page: Page = serde_json::from_slice(&read_response(response)?)
            .map_err(|_| invalid("Invalid Open Banking transaction response"))?;
        for transaction in page.data.transaction {
            facts += 1;
            if facts > 50_000 {
                return Err(invalid("Bank scan transaction limit exceeded"));
            }
            if let Some(fact) = transaction.fact(account)? {
                accept(fact)?;
            }
        }
        if let Some(link) = page.links.next {
            url = next_page(endpoint, &url, &link)?;
        } else if page
            .meta
            .total_pages
            .is_some_and(|total| total > page_number)
        {
            return Err(invalid(
                "Bank omitted pagination link before the final page",
            ));
        } else {
            return Ok(());
        }
    }
    Err(invalid("Bank pagination limit exceeded; scan incomplete"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transaction(status: &str) -> Transaction {
        serde_json::from_value(json!({"AccountId":"account", "TransactionId":"bank-id",
            "TransactionReference":"bank-reference", "TransactionInformation":"P123",
            "CreditDebitIndicator":"Credit", "Status":status,
            "BookingDateTime":"2025-01-01T12:00:00Z", "Amount":{"Amount":"15.00", "Currency":"GBP"}})).unwrap()
    }
    #[test]
    fn only_stable_booked_facts_can_be_money_evidence() {
        for status in ["BOOK", "Booked"] {
            let fact = transaction(status).fact("account").unwrap().unwrap();
            assert_eq!(fact.amount_minor, 1500);
            assert_eq!(fact.status, "settled");
            assert_eq!(fact.direction, "in");
            assert_eq!(fact.source_reference, "bank-reference");
            assert_eq!(fact.narrative, "P123");
            if super::super::OPEN_BANKING_REFERENCE_FIELD == "none" {
                assert!(fact.reference.is_empty());
            }
            assert_eq!(fact.updated_at, fact.settled_at.unwrap());
        }
        assert!(transaction("PDNG").fact("account").unwrap().is_none());
        assert!(transaction("UNKNOWN").fact("account").is_err());
        assert!(transaction("BOOK").fact("other-account").is_err());
        let mut t = transaction("BOOK");
        t.transaction_id = None;
        assert!(t.fact("account").is_err());
        let mut t = transaction("BOOK");
        t.transaction_mutability = Some("Mutable".into());
        assert!(t.fact("account").is_err());
    }
    #[test]
    fn amounts_are_exact_and_never_use_floating_point() {
        for (text, expected) in [
            ("15", 1500),
            ("15.1", 1510),
            ("15.01000", 1501),
            ("0.01", 1),
        ] {
            assert_eq!(
                gbp_minor(&Amount {
                    amount: text.into(),
                    currency: "GBP".into()
                })
                .unwrap(),
                expected
            );
        }
        for text in ["0", "-1", "1e3", "1.001", ".01", "9999999999999", "NaN"] {
            assert!(
                gbp_minor(&Amount {
                    amount: text.into(),
                    currency: "GBP".into()
                })
                .is_err(),
                "{text}"
            );
        }
    }
    #[test]
    fn pagination_cannot_expose_credentials_or_read_another_account() {
        let base =
            transaction_endpoint("https://bank.example/open-banking/v4.0/aisp", "account").unwrap();
        assert_eq!(
            base.path(),
            "/open-banking/v4.0/aisp/accounts/account/transactions"
        );
        assert!(next_page(&base, &base, "?page=2").is_ok());
        for link in [
            "https://attacker.example/steal",
            "http://bank.example/steal",
            "/accounts/other/transactions",
            "https://user@bank.example/open-banking/v4.0/aisp/accounts/account/transactions",
            "?page=2#fragment",
        ] {
            assert!(next_page(&base, &base, link).is_err(), "{link}");
        }
        for base in [
            "http://bank.example/aisp",
            "https://user:secret@bank.example/aisp",
            "https://bank.example/aisp?token=secret",
        ] {
            assert!(transaction_endpoint(base, "account").is_err());
        }
    }
    #[test]
    fn paginated_transport_replays_the_booking_window_and_keeps_earlier_pages_on_failure() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for step in 0..4 {
                let page = step % 2 + 1;
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                let mut b = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    assert!(bytes.len() < 8192);
                    stream.read_exact(&mut b).unwrap();
                    bytes.push(b[0]);
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with("GET /accounts/account/transactions?"));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-token\r\n")
                );
                if page == 1 {
                    assert!(request.contains("fromBookingDateTime="));
                } else {
                    assert!(request.contains("page=2"));
                }
                let body = if page == 1 {
                    json!({"Data":{"Transaction":[{
                    "AccountId":"account","TransactionId":"bank-id", "CreditDebitIndicator":"Credit",
                    "Status":"BOOK","BookingDateTime":"2025-01-01T12:00:00Z", "Amount":{"Amount":"15.00","Currency":"GBP"}
                }]},"Links":{"Next":"?page=2"},"Meta":{"TotalPages":2}}).to_string()
                } else {
                    json!({"Data":{"Transaction":[]},"Links":{},"Meta":{"TotalPages":2}})
                        .to_string()
                };
                let status = if step == 3 { 401 } else { 200 };
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let endpoint =
            Url::parse(&format!("http://{address}/accounts/account/transactions")).unwrap();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let mut facts = Vec::new();
        fetch_transactions(
            &client,
            &endpoint,
            "test-token",
            "account",
            "2025-01-01T00:00:00Z",
            |fact| {
                facts.push(fact);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(facts.len(), 1);
        facts.clear();
        let error = fetch_transactions(
            &client,
            &endpoint,
            "test-token",
            "account",
            "2025-01-01T00:00:00Z",
            |fact| {
                facts.push(fact);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "unavailable");
        assert_eq!(facts.len(), 1);
        assert!(OpenBanking::checkpoint(Utc::now()).is_none());
        server.join().unwrap();
    }
}
