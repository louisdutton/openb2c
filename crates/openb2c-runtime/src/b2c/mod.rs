//! Capability services and synchronous, transactional integration ports.
mod catalog;
mod entitlements;
mod integrations;
mod orders;
mod payments;
pub(crate) mod rest;
mod subscriptions;

use crate::{
    AuthContext, Service,
    service::{Result, ServiceError, sql_value},
};
use chrono::{DateTime, Utc};
use openb2c_schema::{Operation, capabilities, sql::quote};
use rusqlite::{params_from_iter, types::Value as SqlValue};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub use payments::{FakePaymentProvider, PaymentProvider};

pub(super) fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::new("invalid", message)
}
pub(super) fn state(message: impl Into<String>) -> ServiceError {
    ServiceError::new("bad_state", message)
}
pub(super) fn decode<T: DeserializeOwned>(input: &Value) -> Result<T> {
    serde_json::from_value(input.clone()).map_err(|e| invalid(e.to_string()))
}
pub(super) fn key(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 100 || value.chars().any(char::is_control) {
        Err(invalid(
            "Idempotency/event keys must contain 1–100 printable characters",
        ))
    } else {
        Ok(())
    }
}
pub(super) fn integer(row: &Value, field: &str) -> Result<i64> {
    row[field]
        .as_i64()
        .ok_or_else(|| invalid(format!("Invalid integer {field}")))
}
pub(super) fn text<'a>(row: &'a Value, field: &str) -> Result<&'a str> {
    row[field]
        .as_str()
        .ok_or_else(|| invalid(format!("Invalid text {field}")))
}
pub(super) fn instant(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| invalid("Invalid RFC3339 timestamp"))
}

pub(super) enum Event {
    Payment { before: Value, after: Value },
    Order { before: Value, after: Value },
    Subscription { before: Value, after: Value },
}
pub(super) trait Consumer: Sync {
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
    pub(super) fn cap_response(&self, table: &str, mut row: Value) -> Result<Value> {
        self.derived(table, &mut row)?;
        self.redact(table, &mut row);
        Ok(row)
    }
    pub(super) fn cap_table(&self, capability: &str, role: &str) -> Result<&str> {
        capabilities::entity(&self.schema, capability, role).ok_or_else(|| {
            ServiceError::new("not_found", format!("{capability}.{role} is not enabled"))
        })
    }
    pub(super) fn cap_command(
        &self,
        capability: &str,
        scope: &str,
        auth: &AuthContext,
    ) -> Result<()> {
        if !self.schema.capabilities.contains_key(capability) {
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
    pub(super) fn cap_customer(&self, auth: &AuthContext) -> Result<i64> {
        auth.user_id
            .filter(|id| *id > 0)
            .ok_or_else(|| ServiceError::new("unauthorized", "A user-bound credential is required"))
    }
    pub(super) fn cap_owned(
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
    pub(super) fn cap_insert(&self, table: &str, data: Value) -> Result<Value> {
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
    pub(super) fn cap_update(&self, table: &str, id: i64, data: Value) -> Result<Value> {
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
    pub(super) fn cap_find(
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
    pub(super) fn cap_audit(
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
    pub(super) fn cap_publish(
        &self,
        event: Event,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let unchanged = match &event {
            Event::Payment { before, after }
            | Event::Order { before, after }
            | Event::Subscription { before, after } => before == after,
        };
        if unchanged {
            return Ok(());
        }
        let consumers: [&dyn Consumer; 4] = [
            &integrations::OrderPayments,
            &subscriptions::Subscriptions,
            &integrations::OrderEntitlements,
            &integrations::SubscriptionEntitlements,
        ];
        for consumer in consumers {
            if self.schema.capabilities.contains_key(consumer.capability()) {
                consumer.handle(self, &event, auth, now)?;
            }
        }
        Ok(())
    }
}
