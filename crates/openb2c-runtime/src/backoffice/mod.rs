//! Internal business operations. Capabilities own their state and consume upstream events.
#[cfg(feature = "reference")]
#[path = "reference/accounting.rs"]
mod accounting;
#[cfg(all(not(feature = "reference"), feature = "cap_accounting"))]
mod accounting;
#[cfg(feature = "reference")]
#[path = "reference/banking.rs"]
mod banking;
#[cfg(all(not(feature = "reference"), feature = "cap_banking"))]
mod banking;
#[cfg(feature = "reference")]
#[path = "reference/billing.rs"]
mod billing;
#[cfg(all(not(feature = "reference"), feature = "cap_billing"))]
mod billing;
#[cfg(feature = "reference")]
#[path = "reference/documents.rs"]
mod documents;
#[cfg(all(not(feature = "reference"), feature = "cap_documents"))]
mod documents;
#[cfg(feature = "reference")]
#[path = "reference/purchasing.rs"]
mod purchasing;
#[cfg(all(not(feature = "reference"), feature = "cap_purchasing"))]
mod purchasing;
#[cfg(any(feature = "reference", feature = "cap_accounting"))]
mod reports;
#[cfg(all(not(feature = "reference"), feature = "cap_billing"))]
pub(crate) use sales::SaleFacts;
#[cfg(feature = "reference")]
pub(crate) mod rest;
#[cfg(feature = "reference")]
#[path = "reference/sales.rs"]
pub(crate) mod sales;
#[cfg(all(not(feature = "reference"), feature = "cap_billing"))]
pub(crate) mod sales;

#[cfg(all(not(feature = "reference"), feature = "cap_billing"))]
mod history;

// Shared implementation follows.
use crate::b2c::{decode, invalid, key, state};
#[cfg(feature = "reference")]
use crate::b2c::{integer, text};
#[cfg(feature = "reference")]
use crate::service::sql_value;
use crate::{
    AuthContext, Service,
    service::{Result, ServiceError},
};
use chrono::{NaiveDate, Utc};
use openb2c_schema::sql::quote;
#[cfg(feature = "reference")]
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_MONEY: i64 = 9_000_000_000_000;
fn date(value: &str) -> Result<NaiveDate> {
    let d = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| invalid("Use a valid YYYY-MM-DD date"))?;
    if d.to_string() != value {
        return Err(invalid("Use a valid YYYY-MM-DD date"));
    }
    Ok(d)
}
fn description(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > 500 || value.chars().any(char::is_control)
    {
        return Err(invalid("Text must contain 1–500 printable characters"));
    }
    Ok(())
}
fn money(value: i64) -> Result<()> {
    if !(0..=MAX_MONEY).contains(&value) {
        return Err(invalid("Amount is outside the supported minor-unit range"));
    }
    Ok(())
}
fn tax(net: i64, code: &str) -> Result<i64> {
    money(net)?;
    let rate = match code {
        "standard" => 20,
        "reduced" => 5,
        "zero" | "exempt" | "outside" | "not_registered" | "unassessed" => 0,
        _ => return Err(invalid("Unknown UK VAT treatment")),
    };
    Ok(((i128::from(net) * rate + 50) / 100) as i64)
}
#[cfg(feature = "reference")]
fn total(row: &Value) -> Result<i64> {
    integer(row, "net_minor")?
        .checked_add(integer(row, "tax_minor")?)
        .ok_or_else(|| invalid("Amount overflow"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dated {
    date: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Through {
    through_date: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reason {
    date: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Range {
    from: String,
    through: String,
}
impl Range {
    fn check(&self) -> Result<()> {
        if date(&self.from)? > date(&self.through)? {
            return Err(invalid("Report start must precede its end"));
        }
        Ok(())
    }
}
impl Service {
    fn office_table(&self, cap: &str, role: &str) -> Result<&str> {
        self.cap_table(cap, role)
    }
    pub fn backoffice_list(&self, cap: &str, role: &str, auth: &AuthContext) -> Result<Value> {
        self.cap_command(cap, &format!("{cap}.read"), auth)?;
        let table = self.office_table(cap, role)?;
        // Documents are fetched individually with an explicit permission; never bulk-export secrets.
        if cap == "documents" {
            return Err(invalid("Fetch documents individually"));
        }
        #[cfg(feature = "reference")]
        let items = self.query(&format!("SELECT * FROM {} ORDER BY id", quote(table)), &[])?;
        #[cfg(not(feature = "reference"))]
        let items = self.export_rows(table)?;
        Ok(json!({"items":items}))
    }
    pub fn backoffice_record(
        &self,
        cap: &str,
        role: &str,
        id: i64,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command(cap, &format!("{cap}.read"), auth)?;
        self.raw(self.office_table(cap, role)?, &json!(id))
    }
    #[cfg(feature = "reference")]
    fn replay(&self, table: &str, source: &str, request: &Value) -> Result<Option<Value>> {
        if let Some(row) = self.cap_find(table, "source_key", SqlValue::Text(source.into()))? {
            let fingerprint = request.to_string();
            if row["request_json"].as_str() != Some(fingerprint.as_str()) {
                return Err(ServiceError::new(
                    "conflict",
                    "Idempotency key already used with different details",
                ));
            }
            return Ok(Some(row));
        }
        Ok(None)
    }
    fn posted_date(&self, value: &str) -> Result<()> {
        let day = date(value)?;
        if day > Utc::now().date_naive() {
            return Err(invalid("Cannot post future-dated financial entries"));
        }
        let table = self.office_table("accounting", "period")?;
        let closed: Option<String> = self.db.query_row(
            &format!("SELECT max(through_date) FROM {}", quote(table)),
            [],
            |r| r.get(0),
        )?;
        if closed.as_deref().is_some_and(|end| value <= end) {
            return Err(state("The accounting period is closed"));
        }
        Ok(())
    }
}
