use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    idempotency_key: String,
    name: String,
    media_type: String,
    content_base64: String,
}

impl Service {
    pub fn backoffice_document(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("documents", "documents.write", auth)?;
        let i: Document = decode(input)?;
        key(&i.idempotency_key)?;
        description(&i.name)?;
        description(&i.media_type)?;
        if i.content_base64.len() > 500_000 {
            return Err(invalid(
                "Documents may contain at most 375000 decoded bytes",
            ));
        }
        let bytes = STANDARD
            .decode(&i.content_base64)
            .map_err(|_| invalid("Invalid base64 document"))?;
        if bytes.is_empty() {
            return Err(invalid("A document cannot be empty"));
        }
        let hash = hex::encode(Sha256::digest(&bytes));
        let tx = crate::service::transaction(&self.db)?;
        let t = self.office_table("documents", "document")?;
        if let Some(row) =
            self.cap_find(t, "source_key", SqlValue::Text(i.idempotency_key.clone()))?
        {
            if row["sha256"] != hash || row["name"] != i.name || row["media_type"] != i.media_type {
                return Err(state("Document key already used with different content"));
            }
            return Ok(row);
        }
        let row=self.cap_insert(t,json!({"source_key":i.idempotency_key,"name":i.name,"media_type":i.media_type,"content_base64":STANDARD.encode(bytes),"sha256":hash}))?;
        self.cap_audit(t, "documents.store", auth, None, &row)?;
        crate::service::commit(tx)?;
        Ok(row)
    }
}
