//! Immutable files; explicit resource alternatives consume current entitlements.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    idempotency_key: String,
    name: String,
    media_type: String,
    content_base64: String,
    resources: Vec<String>,
}

pub struct Download {
    pub bytes: Vec<u8>,
    pub name: String,
    pub media_type: String,
    pub sha256: String,
}

fn metadata(mut row: Value) -> Value {
    let fields = row.as_object_mut().expect("Database row is an object");
    fields.remove("content_base64");
    fields.remove("source_key");
    if let Some(resources) = fields.remove("resources_json") {
        fields.insert(
            "resources".into(),
            serde_json::from_str(resources.as_str().unwrap_or("[]")).unwrap_or(json!([])),
        );
    }
    row
}

impl Service {
    pub fn b2c_managed_downloads(&self, auth: &AuthContext) -> Result<Value> {
        self.cap_command("downloads", "downloads.publish", auth)?;
        let table = quote(self.cap_table("downloads", "file")?);
        let rows = self.query(&format!("SELECT id,name,media_type,sha256,size_bytes,resources_json,status FROM {table} ORDER BY id"), &[])?;
        Ok(json!({"items":rows.into_iter().map(metadata).collect::<Vec<_>>()}))
    }
    pub fn b2c_publish_download(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("downloads", "downloads.publish", auth)?;
        let mut i: Publication = decode(input)?;
        key(&i.idempotency_key)?;
        if i.name.is_empty()
            || i.name.len() > 128
            || i.name.starts_with('.')
            || !i
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._- ".contains(&b))
        {
            return Err(invalid(
                "Use a filename of 1–128 ASCII letters, digits, spaces, dots, underscores or hyphens, without a leading dot",
            ));
        }
        if i.media_type.len() > 100
            || i.media_type.split('/').count() != 2
            || !i.media_type.split('/').all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
            })
        {
            return Err(invalid("A media type without parameters is required"));
        }
        if i.resources.is_empty() || i.resources.len() > 20 {
            return Err(invalid("Declare 1–20 alternative entitlement resources"));
        }
        for resource in &i.resources {
            key(resource)?;
        }
        i.resources.sort();
        i.resources.dedup();
        if i.content_base64.len() > MAX_BYTES.div_ceil(3) * 4 {
            return Err(invalid("Downloads may contain at most 8 MiB"));
        }
        let bytes = STANDARD
            .decode(&i.content_base64)
            .map_err(|_| invalid("Invalid base64 file"))?;
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(invalid("Files must contain 1 byte to 8 MiB"));
        }
        let hash = hex::encode(Sha256::digest(&bytes));
        let resources =
            serde_json::to_string(&i.resources).map_err(|_| invalid("Invalid resources"))?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("downloads", "file")?;
        if let Some(old) = self.cap_find(
            table,
            "source_key",
            SqlValue::Text(i.idempotency_key.clone()),
        )? {
            if old["sha256"] != hash
                || old["name"] != i.name
                || old["media_type"] != i.media_type
                || old["resources_json"] != resources
            {
                return Err(ServiceError::new(
                    "conflict",
                    "Publication key already used with different content or access requirements",
                ));
            }
            return Ok(metadata(old));
        }
        let row = self.cap_insert(table, json!({"source_key":i.idempotency_key,"name":i.name,"media_type":i.media_type,"content_base64":STANDARD.encode(&bytes),"sha256":hash,"size_bytes":bytes.len(),"resources_json":resources,"status":"published"}))?;
        self.cap_audit(table, "downloads.publish", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(metadata(row))
    }

    pub fn b2c_withdraw_download(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("downloads", "downloads.publish", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let table = self.cap_table("downloads", "file")?;
        let old = self.raw(table, &json!(id))?;
        let row = if old["status"] == "withdrawn" {
            old
        } else {
            let row = self.cap_update(table, id, json!({"status":"withdrawn"}))?;
            self.cap_audit(table, "downloads.withdraw", auth, Some(&old), &row)?;
            row
        };
        crate::service::commit(tx)?;
        Ok(metadata(row))
    }

    // Single SQL snapshot checks file status and current access. No file bytes are
    // selected by the library listing, and staff scopes never bypass membership.
    fn download_rows(
        &self,
        id: Option<i64>,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Vec<Value>> {
        self.cap_command("downloads", "downloads.read", auth)?;
        let user = self.cap_customer(auth)?;
        let file = quote(self.cap_table("downloads", "file")?);
        let grants = quote(self.cap_table("entitlements", "entitlement")?);
        let columns = if id.is_some() {
            "f.*"
        } else {
            "f.id,f.name,f.media_type,f.sha256,f.size_bytes,f.resources_json,f.status"
        };
        self.query(&format!("SELECT {columns} FROM {file} f WHERE f.status='published' AND (? IS NULL OR f.id=?) AND EXISTS (SELECT 1 FROM {grants} e JOIN json_each(f.resources_json) r ON r.value=e.resource WHERE e.user_id=? AND e.status='active' AND julianday(e.starts_at)<=julianday(?) AND (e.ends_at IS NULL OR julianday(e.ends_at)>julianday(?))) ORDER BY f.id"), &[id.map(SqlValue::Integer).unwrap_or(SqlValue::Null),id.map(SqlValue::Integer).unwrap_or(SqlValue::Null),SqlValue::Integer(user),SqlValue::Text(now.to_rfc3339()),SqlValue::Text(now.to_rfc3339())])
    }

    pub fn b2c_downloads(&self, auth: &AuthContext) -> Result<Value> {
        self.b2c_downloads_at(auth, Utc::now())
    }
    pub fn b2c_downloads_at(&self, auth: &AuthContext, now: DateTime<Utc>) -> Result<Value> {
        Ok(
            json!({"items":self.download_rows(None, auth, now)?.into_iter().map(metadata).collect::<Vec<_>>()}),
        )
    }
    pub fn b2c_download(&self, id: i64, auth: &AuthContext) -> Result<Download> {
        self.b2c_download_at(id, auth, Utc::now())
    }
    pub fn b2c_download_at(
        &self,
        id: i64,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Download> {
        let row = self
            .download_rows(Some(id), auth, now)?
            .into_iter()
            .next()
            .ok_or_else(|| ServiceError::new("not_found", "Download is unavailable"))?;
        let bytes = STANDARD
            .decode(text(&row, "content_base64")?)
            .map_err(|_| ServiceError::new("internal_error", "Stored file is corrupt"))?;
        let hash = hex::encode(Sha256::digest(&bytes));
        if row["sha256"] != hash || row["size_bytes"].as_u64() != Some(bytes.len() as u64) {
            return Err(ServiceError::new(
                "internal_error",
                "Stored file failed integrity verification",
            ));
        }
        Ok(Download {
            bytes,
            name: text(&row, "name")?.into(),
            media_type: text(&row, "media_type")?.into(),
            sha256: hash,
        })
    }
}
