use crate::service::{AuthContext, Result, Service, ServiceError};
use chrono::{Duration, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn secret(prefix: &str) -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| ServiceError::new("internal_error", "Random source unavailable"))?;
    Ok(format!("{prefix}{}", hex::encode(bytes)))
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn after(seconds: i64) -> String {
    (Utc::now() + Duration::seconds(seconds)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn production() -> bool {
    std::env::var("NODE_ENV").is_ok_and(|s| s == "production")
}
pub fn fake_allowed() -> bool {
    !production() || std::env::var("ALLOW_FAKE_PROVIDERS").is_ok_and(|s| s == "true")
}

/// Issue a staff credential from trusted, local operator execution.
pub fn provision_api_key(
    schema_json: &str,
    root: &std::path::Path,
    email: &str,
    scopes: &str,
) -> anyhow::Result<()> {
    let schema = serde_json::from_str(schema_json)?;
    let service = crate::open_from_environment(schema, root)?;
    let user = service.ensure_user(email)?;
    let result = service.create_api_key(
        &json!({"user_id":user,"name":"Operator key","scopes":scopes}),
        &AuthContext::system(),
    )?;
    println!("{result}");
    Ok(())
}
pub fn verify_signature(public: &str, message: &[u8], signature: &str) -> bool {
    let Ok(bytes) = hex::decode(public) else {
        return false;
    };
    let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice()) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&bytes) else {
        return false;
    };
    let Ok(signature) = hex::decode(signature).and_then(|v| {
        Signature::from_slice(&v).map_err(|_| hex::FromHexError::InvalidStringLength)
    }) else {
        return false;
    };
    key.verify_strict(message, &signature).is_ok()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    pub email: String,
    pub public_key: String,
    pub issued_at: String,
    pub expires_at: String,
    pub signature: String,
}
impl Certificate {
    fn payload(&self) -> String {
        // Preserve the established certificate wire representation and field order.
        format!(
            "{{\"email\":{},\"publicKey\":{},\"issuedAt\":{},\"expiresAt\":{}}}",
            json!(self.email),
            json!(self.public_key),
            json!(self.issued_at),
            json!(self.expires_at)
        )
    }
}

pub struct Registry {
    signing: Option<SigningKey>,
    pub public_key: String,
    pub local: bool,
}
impl Registry {
    pub fn ephemeral() -> Result<Self> {
        let seed = hex::decode(secret("")?)
            .map_err(|_| ServiceError::new("internal_error", "Invalid random seed"))?;
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| ServiceError::new("internal_error", "Invalid random seed"))?;
        let signing = SigningKey::from_bytes(&seed);
        Ok(Self {
            public_key: hex::encode(signing.verifying_key().to_bytes()),
            signing: Some(signing),
            local: true,
        })
    }
    pub fn from_environment() -> Result<Self> {
        if let Some(private) = std::env::var("REGISTRY_PRIVATE_KEY")
            .ok()
            .filter(|s| !s.is_empty())
        {
            let bytes = hex::decode(private)
                .map_err(|_| ServiceError::new("invalid", "Invalid registry private key"))?;
            if bytes.len() != 32 && bytes.len() != 64 {
                return Err(ServiceError::new(
                    "invalid",
                    "Registry private key must be a 32-byte seed or 64-byte keypair",
                ));
            }
            let seed: [u8; 32] = bytes[..32]
                .try_into()
                .map_err(|_| ServiceError::new("invalid", "Invalid registry seed"))?;
            let signing = SigningKey::from_bytes(&seed);
            if bytes.len() == 64 && signing.verifying_key().as_bytes() != &bytes[32..] {
                return Err(ServiceError::new(
                    "invalid",
                    "Registry keypair does not match",
                ));
            }
            Ok(Self {
                public_key: hex::encode(signing.verifying_key().to_bytes()),
                signing: Some(signing),
                local: true,
            })
        } else if let Some(public_key) = std::env::var("REGISTRY_PUBLIC_KEY")
            .ok()
            .filter(|s| !s.is_empty())
        {
            let bytes = hex::decode(&public_key)
                .map_err(|_| ServiceError::new("invalid", "Invalid registry public key"))?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                ServiceError::new("invalid", "Registry public key must contain 32 bytes")
            })?;
            VerifyingKey::from_bytes(&bytes)
                .map_err(|_| ServiceError::new("invalid", "Invalid registry public key"))?;
            Ok(Self {
                public_key,
                signing: None,
                local: false,
            })
        } else {
            if production()
                && std::env::var("ALLOW_EPHEMERAL_REGISTRY_KEYS").as_deref() != Ok("true")
            {
                return Err(ServiceError::new(
                    "invalid",
                    "A persistent registry key is required in production",
                ));
            }
            Self::ephemeral()
        }
    }
    pub fn issue(&self, email: &str, public_key: &str, lifetime: i64) -> Result<Certificate> {
        let signing = self.signing.as_ref().ok_or_else(|| {
            ServiceError::new("forbidden", "External registry cannot issue certificates")
        })?;
        let mut cert = Certificate {
            email: email.into(),
            public_key: public_key.into(),
            issued_at: now(),
            expires_at: after(lifetime),
            signature: String::new(),
        };
        cert.signature = hex::encode(signing.sign(cert.payload().as_bytes()).to_bytes());
        Ok(cert)
    }
    pub fn verify(&self, cert: &Certificate) -> bool {
        chrono::DateTime::parse_from_rfc3339(&cert.expires_at).is_ok_and(|t| t > Utc::now())
            && verify_signature(&self.public_key, cert.payload().as_bytes(), &cert.signature)
    }
}

impl Service {
    #[cfg(feature = "reference")]
    pub fn self_scopes(&self) -> Vec<String> {
        self.schema
            .operations
            .iter()
            .flat_map(|(table, ops)| {
                ops.iter()
                    .filter(|(_, op)| op.enabled && !op.public && !op.relationships.is_empty())
                    .map(move |(name, op)| {
                        op.scope
                            .clone()
                            .unwrap_or_else(|| format!("{table}.{name}"))
                    })
            })
            .chain(openb2c_schema::capabilities::customer_scopes(&self.schema))
            .chain(
                self.schema
                    .capabilities
                    .contains_key("subscriptions")
                    .then_some("subscriptions.retry".to_owned()),
            )
            .collect()
    }
    pub fn ensure_user(&self, email: &str) -> Result<i64> {
        let normalized;
        let email = if self.cap_enabled("accounts") {
            normalized = crate::passwords::email(email)?;
            normalized.as_str()
        } else {
            email
        };
        let existing = self
            .db
            .query_row("SELECT id FROM user WHERE email = ?", [email], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        self.db.execute(
            "INSERT INTO user (name, email) VALUES (?, ?)",
            [email, email],
        )?;
        Ok(self.db.last_insert_rowid())
    }
    pub fn challenge(&self, email: &str, public: &str, ip: &str) -> Result<(i64, String)> {
        if !email.contains('@') || email.len() > 254 || public.len() > 256 {
            return Err(ServiceError::new(
                "invalid",
                "Valid email and public key are required",
            ));
        }
        for (field, value, limit) in [
            ("email", email, 3),
            ("public_key", public, 3),
            ("ip_address", ip, 10),
        ] {
            let count: i64 = self.db.query_row(&format!("SELECT count(*) FROM identity_challenge WHERE {field} = ? AND julianday(created_at) > julianday('now', '-10 minutes')"), [value], |r| r.get(0))?;
            if count >= limit {
                return Err(ServiceError::new(
                    "rate_limited",
                    format!("Too many identity challenges for {field}"),
                ));
            }
        }
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes)
            .map_err(|_| ServiceError::new("internal_error", "Random source unavailable"))?;
        let code = format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000);
        let hash = bcrypt::hash(&code, 10)
            .map_err(|_| ServiceError::new("internal_error", "Could not hash OTP"))?;
        self.db.execute("INSERT INTO identity_challenge (email, code_hash, public_key, ip_address, expires_at) VALUES (?, ?, ?, ?, ?)", params![email.to_lowercase(),hash,public,ip,after(600)])?;
        Ok((self.db.last_insert_rowid(), code))
    }
    pub fn verify_challenge(
        &self,
        registry: &Registry,
        id: i64,
        code: &str,
        signature: &str,
    ) -> Result<Value> {
        let challenge = self.db.query_row("SELECT email, code_hash, public_key, expires_at FROM identity_challenge WHERE id = ? AND used = 0", [id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?.ok_or_else(|| ServiceError::new("invalid", "Invalid or used challenge"))?;
        for (field, value, limit) in [
            ("challenge_id", id.to_string(), 5),
            ("email", challenge.0.clone(), 10),
        ] {
            let count: i64 = self.db.query_row(&format!("SELECT count(*) FROM identity_verification_attempt WHERE {field} = ? AND julianday(created_at) > julianday('now', '-10 minutes')"), [value], |r| r.get(0))?;
            if count >= limit {
                return Err(ServiceError::new(
                    "rate_limited",
                    "Too many identity verification attempts",
                ));
            }
        }
        self.db.execute(
            "INSERT INTO identity_verification_attempt (challenge_id,email) VALUES (?,?)",
            params![id, challenge.0],
        )?;
        if chrono::DateTime::parse_from_rfc3339(&challenge.3).map_or(true, |t| t < Utc::now())
            || !bcrypt::verify(code, &challenge.1).unwrap_or(false)
        {
            return Err(ServiceError::new(
                "invalid",
                "Incorrect or expired verification code",
            ));
        }
        if !verify_signature(&challenge.2, code.as_bytes(), signature) {
            return Err(ServiceError::new(
                "invalid",
                "Invalid key ownership signature",
            ));
        }
        let tx = crate::service::transaction(&self.db)?;
        let changed = self.db.execute(
            "UPDATE identity_challenge SET used = 1 WHERE id = ? AND used = 0",
            [id],
        )?;
        if changed != 1 {
            return Err(ServiceError::new("invalid", "Challenge already consumed"));
        }
        self.db.execute("INSERT INTO identity_registry (email, public_key) VALUES (?,?) ON CONFLICT(email) DO UPDATE SET public_key = excluded.public_key, revoked = 0, verified_at = CURRENT_TIMESTAMP", [&challenge.0,&challenge.2])?;
        let user = self.ensure_user(&challenge.0)?;
        let (token, expires) = self.issue_session(user)?;
        let certificate = json!(registry.issue(&challenge.0, &challenge.2, 365 * 86400)?);
        crate::service::commit(tx)?;
        Ok(
            json!({"certificate":certificate,"sessionToken":token,"sessionExpiresAt":expires,"auth":{"userId":user,"scopes":self.self_scopes()}}),
        )
    }
    pub fn issue_session(&self, user: i64) -> Result<(String, String)> {
        let token = secret("sess_")?;
        let hash = bcrypt::hash(&token, 10)
            .map_err(|_| ServiceError::new("internal_error", "Could not hash session"))?;
        let expires = after(30 * 86400);
        self.db.execute("INSERT INTO identity_session (user_id,token_hash,token_prefix,expires_at) VALUES (?,?,?,?)", params![user,hash,&token[..16],expires])?;
        Ok((token, expires))
    }
    pub fn revoke_session(&self, token: &str) -> Result<()> {
        let auth = self.authenticate(token)?;
        if !token.starts_with("sess_") {
            return Err(ServiceError::new("invalid", "Identity session required"));
        }
        self.db.execute(
            "UPDATE identity_session SET revoked = 1 WHERE user_id = ? AND token_prefix = ?",
            params![auth.user_id, token.chars().take(16).collect::<String>()],
        )?;
        Ok(())
    }
    pub fn verify_request(
        &self,
        registry: &Registry,
        cert: &Certificate,
        method: &str,
        path: &str,
        timestamp: &str,
        signature: &str,
    ) -> Result<AuthContext> {
        let invalid =
            || ServiceError::new("unauthorized", "Invalid certificate or request signature");
        if !registry.verify(cert) {
            return Err(invalid());
        }
        let state = self
            .db
            .query_row(
                "SELECT revoked FROM identity_registry WHERE email = ? AND public_key = ?",
                [&cert.email, &cert.public_key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        if state == Some(1) || (registry.local && state != Some(0)) {
            return Err(invalid());
        }
        let ts = timestamp.parse::<i64>().map_err(|_| invalid())?;
        if Utc::now().timestamp_millis().abs_diff(ts) > 300_000
            || !verify_signature(
                &cert.public_key,
                format!("{method} {path} {timestamp}").as_bytes(),
                signature,
            )
        {
            return Err(invalid());
        }
        self.db.execute("DELETE FROM identity_request_signature WHERE julianday(created_at) < julianday('now','-5 minutes')", [])?;
        if self.db.execute(
            "INSERT OR IGNORE INTO identity_request_signature (signature) VALUES (?)",
            [signature],
        )? != 1
        {
            return Err(invalid());
        }
        Ok(AuthContext {
            user_id: Some(self.ensure_user(&cert.email)?),
            scopes: self.self_scopes(),
        })
    }
    pub fn create_api_key(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        let tx = crate::service::transaction(&self.db)?;
        let user = input["user_id"]
            .as_i64()
            .or(auth.user_id)
            .ok_or_else(|| ServiceError::new("unauthorized", "A user is required"))?;
        self.authorize("api_key", "create", auth, Some(&json!({"user_id":user})))?;
        let default_scopes = auth.scopes.join(",");
        let scopes = input["scopes"]
            .as_str()
            .unwrap_or(&default_scopes)
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if scopes.iter().any(|s| !auth.has_scope(s)) {
            return Err(ServiceError::new(
                "forbidden",
                "Cannot grant scopes you do not hold",
            ));
        }
        let name = input["name"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 120)
            .ok_or_else(|| ServiceError::new("invalid", "A key name is required"))?;
        let key = secret("do_")?;
        let hash = bcrypt::hash(&key, 10)
            .map_err(|_| ServiceError::new("internal_error", "Could not hash key"))?;
        self.db.execute("INSERT INTO api_key (user_id,name,scopes,key_hash,key_prefix,expires_at) VALUES (?,?,?,?,?,?)", params![user,name,scopes.join(","),hash,&key[..11],input["expires_at"].as_str()])?;
        let row = self.raw("api_key", &json!(self.db.last_insert_rowid()))?;
        let mut row = row;
        let operation = self.policy("api_key", "create")?;
        self.record(
            "api_key",
            &row["id"],
            "create",
            auth,
            (None, Some(&row)),
            &operation,
        )?;
        crate::service::commit(tx)?;
        self.redact("api_key", &mut row);
        row["key"] = json!(key);
        Ok(row)
    }
}
