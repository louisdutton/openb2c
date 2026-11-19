use super::*;
use crate::records::{DocumentsDocument as DocumentRow, DocumentsDocumentWrite, Record};
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
        if let Some(row) = DocumentRow::first(self, "WHERE source_key=?", &[&i.idempotency_key])? {
            if row.sha256 != hash || row.name != i.name || row.media_type != i.media_type {
                return Err(state("Document key already used with different content"));
            }
            return serde_json::to_value(row).map_err(|e| invalid(e.to_string()));
        }
        let row = DocumentsDocumentWrite {
            source_key: i.idempotency_key.into(),
            name: i.name.into(),
            media_type: i.media_type.into(),
            content_base64: STANDARD.encode(bytes).into(),
            sha256: hash.into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "documents.store", auth, None)?;
        crate::service::commit(tx)?;
        // This explicitly authorized response includes the immutable evidence bytes.
        serde_json::to_value(row).map_err(|e| invalid(e.to_string()))
    }
}
