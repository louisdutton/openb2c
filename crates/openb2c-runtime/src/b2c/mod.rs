//! Capability services and synchronous, transactional integration ports.
#[cfg(all(
    not(feature = "reference"),
    any(feature = "cap_starling", feature = "cap_open_banking")
))]
mod bank_feed;
#[cfg(all(not(feature = "reference"), feature = "cap_open_banking"))]
mod open_banking;
#[cfg(all(not(feature = "reference"), feature = "cap_open_banking"))]
use open_banking::OpenBanking as SelectedBankFeed;
#[cfg(all(not(feature = "reference"), feature = "cap_starling"))]
use starling::Starling as SelectedBankFeed;
#[cfg(all(not(feature = "reference"), feature = "cap_open_banking"))]
const OPEN_BANKING_REFERENCE_FIELD: &str = "none";
#[cfg(any(feature = "reference", feature = "cap_subscriptions"))]
mod calendar;
#[cfg(feature = "reference")]
#[path = "reference/catalog.rs"]
mod catalog;
#[cfg(all(not(feature = "reference"), feature = "cap_catalog"))]
mod catalog;
#[cfg(feature = "reference")]
#[path = "reference/downloads.rs"]
mod downloads;
#[cfg(all(not(feature = "reference"), feature = "cap_downloads"))]
mod downloads;
#[cfg(feature = "reference")]
#[path = "reference/entitlements.rs"]
mod entitlements;
#[cfg(all(not(feature = "reference"), feature = "cap_entitlements"))]
mod entitlements;
#[cfg(feature = "reference")]
#[path = "reference/integrations.rs"]
mod integrations;
#[cfg(all(not(feature = "reference"), feature = "cap_order_payments"))]
mod integrations;
#[cfg(feature = "reference")]
#[path = "reference/orders.rs"]
mod orders;
#[cfg(all(not(feature = "reference"), feature = "cap_orders"))]
mod orders;
#[cfg(feature = "reference")]
#[path = "reference/payments.rs"]
mod payments;
#[cfg(all(not(feature = "reference"), feature = "cap_payments"))]
mod payments;
#[cfg(feature = "reference")]
pub(crate) mod rest;
#[cfg(all(not(feature = "reference"), feature = "cap_starling"))]
mod starling;
#[cfg(feature = "reference")]
#[path = "reference/subscriptions.rs"]
mod subscriptions;
#[cfg(all(not(feature = "reference"), feature = "cap_subscriptions"))]
mod subscriptions;
#[cfg(all(not(feature = "reference"), feature = "cap_payments"))]
mod transfers;

// Shared implementation follows.
#[cfg(feature = "reference")]
use crate::service::sql_value;
use crate::{
    AuthContext, Service,
    service::{Result, ServiceError},
};
use chrono::{DateTime, Utc};
use openb2c_schema::{Operation, sql::quote};
#[cfg(feature = "reference")]
use rusqlite::{params_from_iter, types::Value as SqlValue};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[cfg(any(feature = "reference", feature = "cap_downloads"))]
pub use downloads::Download;
#[cfg(feature = "reference")]
pub use payments::{FakePaymentProvider, PaymentProvider};
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}
#[cfg(all(not(feature = "reference"), feature = "cap_subscriptions"))]
pub(crate) use calendar::billing_period;

pub(crate) fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::new("invalid", message)
}
pub(crate) fn state(message: impl Into<String>) -> ServiceError {
    ServiceError::new("bad_state", message)
}
pub(crate) fn decode<T: DeserializeOwned>(input: &Value) -> Result<T> {
    serde_json::from_value(input.clone()).map_err(|e| invalid(e.to_string()))
}
pub(crate) fn key(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 100 || value.chars().any(char::is_control) {
        Err(invalid(
            "Idempotency/event keys must contain 1–100 printable characters",
        ))
    } else {
        Ok(())
    }
}
#[cfg(feature = "reference")]
pub(crate) fn integer(row: &Value, field: &str) -> Result<i64> {
    row[field]
        .as_i64()
        .ok_or_else(|| invalid(format!("Invalid integer {field}")))
}
#[cfg(feature = "reference")]
pub(crate) fn text<'a>(row: &'a Value, field: &str) -> Result<&'a str> {
    row[field]
        .as_str()
        .ok_or_else(|| invalid(format!("Invalid text {field}")))
}
pub(crate) fn instant(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| invalid("Invalid RFC3339 timestamp"))
}

#[cfg(not(feature = "reference"))]
pub(crate) use crate::records::Event;
#[cfg(feature = "reference")]
pub(crate) enum Event {
    Payment { before: Value, after: Value },
    Order { before: Value, after: Value },
    Subscription { before: Value, after: Value },
}
#[cfg(feature = "reference")]
pub(crate) trait Consumer: Sync {
    fn enabled(&self, service: &Service) -> bool {
        service.schema.capabilities.contains_key(self.capability())
    }
    fn capability(&self) -> &'static str;
    fn handle(
        &self,
        service: &Service,
        event: &Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()>;
}

impl Service {
    #[cfg(feature = "reference")]
    pub(crate) fn cap_response(&self, table: &str, mut row: Value) -> Result<Value> {
        self.derived(table, &mut row)?;
        self.redact(table, &mut row);
        Ok(row)
    }
    #[inline]
    pub(crate) fn cap_table(&self, capability: &str, role: &str) -> Result<&str> {
        self.entity(capability, role).ok_or_else(|| {
            ServiceError::new("not_found", format!("{capability}.{role} is not enabled"))
        })
    }
    #[inline]
    pub(crate) fn entity(&self, capability: &str, role: &str) -> Option<&str> {
        #[cfg(feature = "reference")]
        {
            openb2c_schema::capabilities::entity(&self.schema, capability, role)
        }
        #[cfg(not(feature = "reference"))]
        {
            crate::composition::entity(capability, role)
        }
    }
    #[inline]
    pub(crate) fn cap_enabled(&self, capability: &str) -> bool {
        #[cfg(feature = "reference")]
        {
            self.schema.capabilities.contains_key(capability)
        }
        #[cfg(not(feature = "reference"))]
        {
            crate::composition::enabled(capability)
        }
    }
    #[inline]
    pub(crate) fn cap_setting(&self, capability: &str, key: &str) -> Option<&str> {
        #[cfg(feature = "reference")]
        {
            self.schema
                .capabilities
                .get(capability)?
                .settings
                .get(key)?
                .as_str()
        }
        #[cfg(not(feature = "reference"))]
        {
            crate::composition::setting(capability, key)
        }
    }
    #[inline]
    pub(crate) fn model_has_table(&self, table: &str) -> bool {
        #[cfg(feature = "reference")]
        {
            self.schema.tables.contains_key(table)
        }
        #[cfg(not(feature = "reference"))]
        {
            crate::composition::has_table(table)
        }
    }
    pub(crate) fn api_table(&self, path: &str) -> Option<&str> {
        #[cfg(feature = "reference")]
        {
            self.schema
                .tables
                .keys()
                .find(|t| format!("{t}s") == path)
                .map(String::as_str)
        }
        #[cfg(not(feature = "reference"))]
        {
            crate::composition::api_table(path)
        }
    }
    pub(crate) fn cap_command(
        &self,
        capability: &str,
        scope: &str,
        auth: &AuthContext,
    ) -> Result<()> {
        if !self.cap_enabled(capability) {
            return Err(ServiceError::new("not_found", "Capability is not enabled"));
        }
        if auth.has_scope(scope) {
            Ok(())
        } else {
            Err(ServiceError::new(
                if auth.user_id.is_some() {
                    "forbidden"
                } else {
                    "unauthorized"
                },
                format!("{scope} is required"),
            ))
        }
    }
    pub(crate) fn cap_customer(&self, auth: &AuthContext) -> Result<i64> {
        let user = auth.user_id.filter(|id| *id > 0).ok_or_else(|| {
            ServiceError::new("unauthorized", "A user-bound credential is required")
        })?;
        let table = self.cap_table("customers", "customer")?;
        let member: bool = self.db.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE user_id=?)",
                quote(table)
            ),
            [user],
            |row| row.get(0),
        )?;
        if !member {
            return Err(ServiceError::new(
                "forbidden",
                "A customer membership is required",
            ));
        }
        Ok(user)
    }
    #[cfg(feature = "reference")]
    pub(crate) fn cap_owned(
        &self,
        capability: &str,
        role: &str,
        id: i64,
        scope: &str,
        auth: &AuthContext,
    ) -> Result<Value> {
        self.cap_command(capability, scope, auth)?;
        let row = self.raw(self.cap_table(capability, role)?, &json!(id))?;
        if !auth.has_scope("*") && row["user_id"].as_i64() != auth.user_id {
            return Err(ServiceError::new(
                "forbidden",
                "Record belongs to another customer",
            ));
        }
        Ok(row)
    }
    #[cfg(feature = "reference")]
    pub(crate) fn cap_insert(&self, table: &str, data: Value) -> Result<Value> {
        let data = data
            .as_object()
            .ok_or_else(|| invalid("Expected an object"))?;
        self.validate_input(table, data, true)?;
        let fields = data.keys().map(|f| quote(f)).collect::<Vec<_>>();
        let values = data.values().map(sql_value).collect::<Result<Vec<_>>>()?;
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) RETURNING *",
            quote(table),
            fields.join(","),
            vec!["?"; fields.len()].join(",")
        );
        let row = self
            .query(&sql, &values)?
            .into_iter()
            .next()
            .ok_or_else(|| invalid("Insert returned no row"))?;
        self.application_constraints(table, &row)?;
        Ok(row)
    }
    #[cfg(feature = "reference")]
    pub(crate) fn cap_update(&self, table: &str, id: i64, data: Value) -> Result<Value> {
        let data = data
            .as_object()
            .ok_or_else(|| invalid("Expected an object"))?;
        self.validate_input(table, data, false)?;
        let fields = data
            .keys()
            .map(|f| format!("{}=?", quote(f)))
            .collect::<Vec<_>>();
        let mut values = data.values().map(sql_value).collect::<Result<Vec<_>>>()?;
        values.push(SqlValue::Integer(id));
        self.db.execute(
            &format!(
                "UPDATE {} SET {} WHERE id=?",
                quote(table),
                fields.join(",")
            ),
            params_from_iter(values),
        )?;
        let row = self.raw(table, &json!(id))?;
        self.application_constraints(table, &row)?;
        Ok(row)
    }
    #[cfg(feature = "reference")]
    pub(crate) fn cap_find(
        &self,
        table: &str,
        field: &str,
        value: SqlValue,
    ) -> Result<Option<Value>> {
        Ok(self
            .query(
                &format!("SELECT * FROM {} WHERE {}=?", quote(table), quote(field)),
                &[value],
            )?
            .into_iter()
            .next())
    }
    pub(crate) fn cap_audit(
        &self,
        table: &str,
        action: &str,
        auth: &AuthContext,
        before: Option<&Value>,
        after: &Value,
    ) -> Result<()> {
        self.record(
            table,
            &after["id"],
            action,
            auth,
            (before, Some(after)),
            &Operation {
                audit: json!({"required":true,"category":"workflow","summary":action}),
                ..Default::default()
            },
        )
    }
    pub(crate) fn cap_publish(
        &self,
        event: Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        #[cfg(not(feature = "reference"))]
        {
            crate::composition::publish(self, event, auth, now)
        }
        #[cfg(feature = "reference")]
        {
            let unchanged = match &event {
                Event::Payment { before, after }
                | Event::Order { before, after }
                | Event::Subscription { before, after } => before == after,
            };
            if unchanged {
                return Ok(());
            }
            let consumers: [&dyn Consumer; 6] = [
                &integrations::OrderPayments,
                &subscriptions::Subscriptions,
                &integrations::OrderEntitlements,
                &integrations::SubscriptionEntitlements,
                &crate::backoffice::sales::OrderBilling,
                &crate::backoffice::sales::SubscriptionBilling,
            ];
            for consumer in consumers {
                if consumer.enabled(self) {
                    consumer.handle(self, &event, auth, now)?;
                }
            }
            Ok(())
        }
    }
}
