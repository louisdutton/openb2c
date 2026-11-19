use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    idempotency_key: String,
    user_id: i64,
    resource: String,
    #[serde(default)]
    ends_at: Option<String>,
}

impl Service {
    pub(crate) fn cap_grant(
        &self,
        user: i64,
        resource: &str,
        source: &str,
        end: Option<&str>,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        let table = self.cap_table("entitlements", "entitlement")?;
        let row = if let Some(old) =
            self.cap_find(table, "source_key", SqlValue::Text(source.into()))?
        {
            if old["user_id"] != user || old["resource"] != resource {
                return Err(state(
                    "Entitlement source is bound to a different customer or resource",
                ));
            }
            let next = self.cap_update(
                table,
                integer(&old, "id")?,
                json!({"status":"active","ends_at":end}),
            )?;
            self.cap_audit(table, "entitlements.extend", auth, Some(&old), &next)?;
            next
        } else {
            let next=self.cap_insert(table,json!({"user_id":user,"resource":resource,"source_key":source,"status":"active","starts_at":now.to_rfc3339(),"ends_at":end}))?;
            self.cap_audit(table, "entitlements.grant", auth, None, &next)?;
            next
        };
        Ok(row)
    }
    pub(crate) fn cap_revoke_source(&self, source: &str, auth: &AuthContext) -> Result<()> {
        let table = self.cap_table("entitlements", "entitlement")?;
        if let Some(old) = self.cap_find(table, "source_key", SqlValue::Text(source.into()))?
            && old["status"] != "revoked"
        {
            let next = self.cap_update(table, integer(&old, "id")?, json!({"status":"revoked"}))?;
            self.cap_audit(table, "entitlements.revoke", auth, Some(&old), &next)?;
        }
        Ok(())
    }
    pub fn b2c_entitlements(&self, auth: &AuthContext) -> Result<Value> {
        self.b2c_entitlements_at(auth, Utc::now())
    }
    pub fn b2c_entitlements_at(&self, auth: &AuthContext, now: DateTime<Utc>) -> Result<Value> {
        self.cap_command("entitlements", "entitlements.read", auth)?;
        let user = self.cap_customer(auth)?;
        let items=self.query(&format!("SELECT * FROM {} WHERE user_id=? AND status='active' AND julianday(starts_at)<=julianday(?) AND (ends_at IS NULL OR julianday(ends_at)>julianday(?)) ORDER BY id",quote(self.cap_table("entitlements","entitlement")?)),&[SqlValue::Integer(user),SqlValue::Text(now.to_rfc3339()),SqlValue::Text(now.to_rfc3339())])?;
        let items = items
            .into_iter()
            .map(|row| self.cap_response(self.cap_table("entitlements", "entitlement")?, row))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"items":items}))
    }
    pub fn b2c_grant_entitlement(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("entitlements", "entitlements.grant", auth)?;
        let input: Grant = decode(input)?;
        key(&input.idempotency_key)?;
        if input.resource.is_empty() || input.resource.len() > 100 || input.user_id < 1 {
            return Err(invalid("A customer and resource are required"));
        }
        let now = Utc::now();
        let end = input
            .ends_at
            .as_deref()
            .map(instant)
            .transpose()?
            .map(|t| t.to_rfc3339());
        if end
            .as_deref()
            .map(instant)
            .transpose()?
            .is_some_and(|t| t <= now)
        {
            return Err(invalid("Grant expiry must be in the future"));
        }
        let source = format!(
            "manual:{}:{}",
            auth.user_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "system".into()),
            input.idempotency_key
        );
        let tx = crate::service::transaction(&self.db)?;
        if let Some(old) = self.cap_find(
            self.cap_table("entitlements", "entitlement")?,
            "source_key",
            SqlValue::Text(source.clone()),
        )? {
            if old["user_id"] != input.user_id
                || old["resource"] != input.resource
                || old["ends_at"] != json!(end)
            {
                return Err(ServiceError::new(
                    "conflict",
                    "Grant idempotency key already used with different data",
                ));
            }
            return self.cap_response(self.cap_table("entitlements", "entitlement")?, old);
        }
        let grant = self.cap_grant(
            input.user_id,
            &input.resource,
            &source,
            end.as_deref(),
            auth,
            now,
        )?;
        let result = self.cap_response(self.cap_table("entitlements", "entitlement")?, grant)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub fn b2c_revoke_entitlement(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("entitlements", "entitlements.revoke", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("entitlements", "entitlement")?;
        let old = self.raw(table, &json!(id))?;
        self.cap_revoke_source(text(&old, "source_key")?, auth)?;
        let row = self.cap_response(table, self.raw(table, &json!(id))?)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
}
