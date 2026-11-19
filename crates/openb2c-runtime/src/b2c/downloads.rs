//! Immutable files and typed access queries against current entitlements.
use super::*;
use crate::records::{
    DownloadsFile as File, DownloadsFileStatus as Status, DownloadsFileWrite as FileWrite,
    EntitlementsEntitlement, Record,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
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
#[derive(Serialize)]
struct Metadata {
    id: i64,
    name: String,
    media_type: String,
    sha256: String,
    size_bytes: i64,
    resources: Vec<String>,
    status: Status,
}
impl Metadata {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let resources: String = row.get("resources_json")?;
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            media_type: row.get("media_type")?,
            sha256: row.get("sha256")?,
            size_bytes: row.get("size_bytes")?,
            status: row.get("status")?,
            resources: serde_json::from_str(&resources).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        })
    }
    fn from_file(row: File) -> Result<Self> {
        Ok(Self {
            id: row.id,
            name: row.name,
            media_type: row.media_type,
            sha256: row.sha256,
            size_bytes: row.size_bytes,
            resources: serde_json::from_str(&row.resources_json)
                .map_err(|_| invalid("Invalid stored download resources"))?,
            status: row.status,
        })
    }
    fn wire(self) -> Result<Value> {
        serde_json::to_value(self).map_err(|e| invalid(e.to_string()))
    }
}
const METADATA: &str = "id,name,media_type,sha256,size_bytes,resources_json,status";
impl Service {
    pub fn b2c_managed_downloads(&self, auth: &AuthContext) -> Result<Value> {
        self.cap_command("downloads", "downloads.publish", auth)?;
        let mut statement = self.db.prepare(&format!(
            "SELECT {METADATA} FROM {} ORDER BY id",
            quote(File::TABLE)
        ))?;
        let items = statement
            .query_map([], Metadata::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"items":items}))
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
        let resources = serde_json::to_string(&i.resources).map_err(|e| invalid(e.to_string()))?;
        let tx = crate::service::transaction(&self.db)?;
        if let Some(old) = File::first(self, "WHERE source_key=?", &[&i.idempotency_key])? {
            if old.sha256 != hash
                || old.name != i.name
                || old.media_type != i.media_type
                || old.resources_json != resources
            {
                return Err(ServiceError::new(
                    "conflict",
                    "Publication key already used with different content or access requirements",
                ));
            }
            return Metadata::from_file(old)?.wire();
        }
        let row = FileWrite {
            source_key: i.idempotency_key.into(),
            name: i.name.into(),
            media_type: i.media_type.into(),
            content_base64: STANDARD.encode(&bytes).into(),
            sha256: hash.into(),
            size_bytes: (bytes.len() as i64).into(),
            resources_json: resources.into(),
            status: Status::Published.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "downloads.publish", auth, None)?;
        crate::service::commit(tx)?;
        Metadata::from_file(row)?.wire()
    }
    pub fn b2c_withdraw_download(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.cap_command("downloads", "downloads.publish", auth)?;
        let tx = crate::service::transaction(&self.db)?;
        let old = File::get(self, id)?;
        let row = if old.status == Status::Withdrawn {
            old
        } else {
            let row = FileWrite {
                status: Status::Withdrawn.into(),
                ..Default::default()
            }
            .update(self, id)?;
            row.audit(self, "downloads.withdraw", auth, Some(&old))?;
            row
        };
        crate::service::commit(tx)?;
        Metadata::from_file(row)?.wire()
    }
    fn download_access_sql(&self, columns: &str) -> String {
        format!(
            "SELECT {columns} FROM {} f WHERE f.status='published' AND (? IS NULL OR f.id=?) AND EXISTS (SELECT 1 FROM {} e JOIN json_each(f.resources_json) r ON r.value=e.resource WHERE e.user_id=? AND e.status='active' AND julianday(e.starts_at)<=julianday(?) AND (e.ends_at IS NULL OR julianday(e.ends_at)>julianday(?))) ORDER BY f.id",
            quote(File::TABLE),
            quote(EntitlementsEntitlement::TABLE)
        )
    }
    pub fn b2c_downloads(&self, auth: &AuthContext) -> Result<Value> {
        self.b2c_downloads_at(auth, Utc::now())
    }
    pub fn b2c_downloads_at(&self, auth: &AuthContext, now: DateTime<Utc>) -> Result<Value> {
        self.cap_command("downloads", "downloads.read", auth)?;
        let user = self.cap_customer(auth)?;
        let columns = METADATA
            .split(',')
            .map(|c| format!("f.{c}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = self.db.prepare(&self.download_access_sql(&columns))?;
        let time = now.to_rfc3339();
        let items = statement
            .query_map(
                rusqlite::params![Option::<i64>::None, Option::<i64>::None, user, time, time],
                Metadata::read,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"items":items}))
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
        self.cap_command("downloads", "downloads.read", auth)?;
        let user = self.cap_customer(auth)?;
        let time = now.to_rfc3339();
        let row = File::query(
            self,
            &self.download_access_sql("f.*"),
            &[&id, &id, &user, &time, &time],
        )?
        .into_iter()
        .next()
        .ok_or_else(|| ServiceError::new("not_found", "Download is unavailable"))?;
        let bytes = STANDARD
            .decode(&row.content_base64)
            .map_err(|_| ServiceError::new("internal_error", "Stored file is corrupt"))?;
        let hash = hex::encode(Sha256::digest(&bytes));
        if row.sha256 != hash || row.size_bytes != bytes.len() as i64 {
            return Err(ServiceError::new(
                "internal_error",
                "Stored file failed integrity verification",
            ));
        }
        Ok(Download {
            bytes,
            name: row.name,
            media_type: row.media_type,
            sha256: hash,
        })
    }
}
