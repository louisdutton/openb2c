//! Account identity, customer membership and staff authority have separate lifecycles.
use crate::{
    AuthContext, Service,
    service::{Result, ServiceError},
};
use openb2c_schema::sql::quote;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeSet;
impl Service {
    pub fn account_scopes(&self, user: i64) -> Result<Vec<String>> {
        if !self.cap_enabled("accounts") {
            return Ok(self.self_scopes());
        }
        let mut scopes = BTreeSet::new();
        if let Some(table) = self.entity("customers", "customer") {
            let member: bool = self.db.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE user_id=?)",
                    quote(table)
                ),
                [user],
                |r| r.get(0),
            )?;
            if member {
                scopes.extend(self.self_scopes());
            }
        }
        let table = self
            .entity("accounts", "staff")
            .ok_or_else(|| ServiceError::new("not_found", "Staff identity is not enabled"))?;
        let staff: Option<String> = self
            .db
            .query_row(
                &format!(
                    "SELECT scopes FROM {} WHERE user_id=? AND active=1",
                    quote(table)
                ),
                [user],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(staff) = staff {
            scopes.extend(
                staff
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        }
        Ok(scopes.into_iter().collect())
    }
    pub fn ensure_customer(&self, email: &str, auth: &AuthContext) -> Result<i64> {
        self.cap_command("customers", "customers.manage", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let user = self.ensure_user(email)?;
        let table = self.cap_table("customers", "customer")?;
        self.db.execute(
            &format!(
                "INSERT INTO {} (user_id) VALUES (?) ON CONFLICT(user_id) DO NOTHING",
                quote(table)
            ),
            [user],
        )?;
        crate::service::commit(tx)?;
        Ok(user)
    }
    pub fn provision_staff(
        &self,
        email: &str,
        password: &str,
        scopes: &str,
        auth: &AuthContext,
    ) -> Result<Value> {
        if !auth.has_scope("*") {
            return Err(ServiceError::new(
                "forbidden",
                "Local owner authority is required",
            ));
        }
        if !email.contains('@') || email.len() > 254 {
            return Err(ServiceError::new(
                "invalid",
                "A valid account email is required",
            ));
        }
        let scopes = scopes
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<BTreeSet<_>>();
        if scopes.is_empty()
            || scopes
                .iter()
                .any(|s| s.len() > 100 || s.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err(ServiceError::new(
                "invalid",
                "Explicit staff scopes are required",
            ));
        }
        let outer = crate::service::transaction(&self.db)?;
        let normalized = email.trim().to_lowercase();
        let user = self.ensure_user(&normalized)?;
        self.recover_password(&normalized, password, auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("accounts", "staff")?;
        let scopes = scopes.into_iter().collect::<Vec<_>>().join(",");
        self.db.execute(&format!("INSERT INTO {} (user_id,scopes,active) VALUES (?,?,1) ON CONFLICT(user_id) DO UPDATE SET scopes=excluded.scopes,active=1",quote(table)),params![user,scopes])?;
        #[cfg(feature = "reference")]
        let row = self
            .cap_find(table, "user_id", rusqlite::types::Value::Integer(user))?
            .unwrap();
        #[cfg(feature = "reference")]
        self.cap_audit(table, "accounts.provision_staff", auth, None, &row)?;
        #[cfg(not(feature = "reference"))]
        let row = self.provisioned_staff(user, auth)?;
        crate::service::commit(tx)?;
        crate::service::commit(outer)?;
        Ok(json!({"user_id":user,"staff":row,"credentials_replaced":true}))
    }
    pub fn disable_staff(&self, email: &str, auth: &AuthContext) -> Result<Value> {
        if !auth.has_scope("*") {
            return Err(ServiceError::new(
                "forbidden",
                "Local owner authority is required",
            ));
        }
        let tx = crate::service::transaction(&self.db)?;
        let user: i64 = self
            .db
            .query_row(
                "SELECT id FROM user WHERE lower(email)=?",
                [email.trim().to_lowercase()],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| ServiceError::new("not_found", "Account not found"))?;
        let table = self.cap_table("accounts", "staff")?;
        self.db.execute(
            &format!("UPDATE {} SET active=0 WHERE user_id=?", quote(table)),
            [user],
        )?;
        self.db
            .execute("UPDATE api_key SET active=0 WHERE user_id=?", [user])?;
        let sessions = self.cap_table("password_auth", "session")?;
        self.db.execute(
            &format!("UPDATE {} SET revoked=1 WHERE user_id=?", quote(sessions)),
            [user],
        )?;
        self.cap_audit(
            table,
            "accounts.disable_staff",
            auth,
            None,
            &json!({"id":user,"user_id":user,"active":0}),
        )?;
        crate::service::commit(tx)?;
        Ok(json!({"user_id":user,"disabled":true}))
    }
}
