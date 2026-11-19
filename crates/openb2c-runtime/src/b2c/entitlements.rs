use super::*;
use crate::records::{
    EntitlementsEntitlement as Entitlement, EntitlementsEntitlementStatus as Status,
    EntitlementsEntitlementWrite as EntitlementWrite, Record,
};
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
    ) -> Result<Entitlement> {
        if let Some(old) = Entitlement::first(self, "WHERE source_key=?", &[&source])? {
            if old.user_id != user || old.resource != resource {
                return Err(state(
                    "Entitlement source is bound to a different customer or resource",
                ));
            }
            let next = EntitlementWrite {
                status: Status::Active.into(),
                ends_at: end.map(str::to_owned).into(),
                ..Default::default()
            }
            .update(self, old.id)?;
            next.audit(self, "entitlements.extend", auth, Some(&old))?;
            Ok(next)
        } else {
            let next = EntitlementWrite {
                user_id: user.into(),
                resource: resource.to_owned().into(),
                source_key: source.to_owned().into(),
                status: Status::Active.into(),
                starts_at: now.to_rfc3339().into(),
                ends_at: end.map(str::to_owned).into(),
                ..Default::default()
            }
            .insert(self)?;
            next.audit(self, "entitlements.grant", auth, None)?;
            Ok(next)
        }
    }
    pub(crate) fn cap_revoke_source(&self, source: &str, auth: &AuthContext) -> Result<()> {
        if let Some(old) = Entitlement::first(self, "WHERE source_key=?", &[&source])?
            && old.status != Status::Revoked
        {
            let next = EntitlementWrite {
                status: Status::Revoked.into(),
                ..Default::default()
            }
            .update(self, old.id)?;
            next.audit(self, "entitlements.revoke", auth, Some(&old))?;
        }
        Ok(())
    }
    pub fn b2c_entitlements(&self, auth: &AuthContext) -> Result<Value> {
        self.b2c_entitlements_at(auth, Utc::now())
    }
    pub fn b2c_entitlements_at(&self, auth: &AuthContext, now: DateTime<Utc>) -> Result<Value> {
        self.cap_command("entitlements", "entitlements.read", auth)?;
        let user = self.cap_customer(auth)?;
        let time = now.to_rfc3339();
        let items=Entitlement::select(self,"WHERE user_id=? AND status='active' AND julianday(starts_at)<=julianday(?) AND (ends_at IS NULL OR julianday(ends_at)>julianday(?)) ORDER BY id",&[&user,&time,&time])?.iter().map(|r|r.wire(self)).collect::<Result<Vec<_>>>()?;
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
        if let Some(old) = Entitlement::first(self, "WHERE source_key=?", &[&source])? {
            if old.user_id != input.user_id || old.resource != input.resource || old.ends_at != end
            {
                return Err(ServiceError::new(
                    "conflict",
                    "Grant idempotency key already used with different data",
                ));
            }
            return old.wire(self);
        }
        let grant = self.cap_grant(
            input.user_id,
            &input.resource,
            &source,
            end.as_deref(),
            auth,
            now,
        )?;
        let result = grant.wire(self)?;
        crate::service::commit(tx)?;
        Ok(result)
    }
    pub fn b2c_revoke_entitlement(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("entitlements", "entitlements.revoke", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let old = Entitlement::get(self, id)?;
        self.cap_revoke_source(&old.source_key, auth)?;
        let row = Entitlement::get(self, id)?.wire(self)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
}
