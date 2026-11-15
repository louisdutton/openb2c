use crate::{
    Service,
    identity::{fake_allowed, now, production},
    service::{Result, ServiceError},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{env, time::Duration};

pub fn hmac(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}
pub fn verify_hmac(secret: &str, body: &[u8], signature: &str) -> bool {
    let Ok(signature) = hex::decode(signature.strip_prefix("sha256=").unwrap_or(signature)) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}
pub fn verify_webhook(
    secret: &str,
    body: &[u8],
    timestamp: &str,
    signature: &str,
    tolerance: i64,
) -> bool {
    let Ok(ts) = timestamp.parse::<i64>() else {
        return false;
    };
    if Utc::now().timestamp().abs_diff(ts) > tolerance.max(1) as u64 {
        return false;
    }
    let mut payload = format!("{timestamp}.").into_bytes();
    payload.extend_from_slice(body);
    !secret.is_empty() && verify_hmac(secret, &payload, signature)
}
fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| ServiceError::new("internal_error", "Could not initialize delivery client"))
}

impl Service {
    pub(crate) fn effect_tables(&self) -> Result<()> {
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_effect_attempt (
            id INTEGER PRIMARY KEY AUTOINCREMENT, idempotency_key TEXT NOT NULL UNIQUE,
            effect_type TEXT NOT NULL, payload_json TEXT NOT NULL, context_json TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0,
            max_attempts INTEGER NOT NULL DEFAULT 3, next_attempt_at TEXT, last_error TEXT,
            result_json TEXT, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
            CREATE INDEX IF NOT EXISTS openb2c_effect_attempt_status ON openb2c_effect_attempt(status,next_attempt_at);
            CREATE TABLE IF NOT EXISTS openb2c_fake_email (id INTEGER PRIMARY KEY, payload_json TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
        Ok(())
    }
    pub fn queue_effect(
        &self,
        key: &str,
        kind: &str,
        payload: &Value,
        context: &Value,
    ) -> Result<()> {
        self.db.execute("INSERT OR IGNORE INTO openb2c_effect_attempt (idempotency_key,effect_type,payload_json,context_json,max_attempts) VALUES (?,?,?,?,?)",params![key,kind,payload.to_string(),context.to_string(),env::var("EFFECT_MAX_ATTEMPTS").ok().and_then(|s|s.parse::<i64>().ok()).unwrap_or(3).max(1)])?;
        Ok(())
    }
    pub fn effects(&self, status: Option<&str>) -> Result<Vec<Value>> {
        self.query("SELECT * FROM openb2c_effect_attempt WHERE (? IS NULL OR status=?) ORDER BY id DESC LIMIT 1000",&[status.map(|s|rusqlite::types::Value::Text(s.into())).unwrap_or(rusqlite::types::Value::Null),status.map(|s|rusqlite::types::Value::Text(s.into())).unwrap_or(rusqlite::types::Value::Null)])
    }
    pub fn fake_emails(&self) -> Result<Vec<Value>> {
        let mut stmt = self
            .db
            .prepare("SELECT payload_json FROM openb2c_fake_email ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|s| {
                serde_json::from_str(&s)
                    .map_err(|_| ServiceError::new("internal_error", "Corrupt email capture"))
            })
            .collect()
    }
    pub fn deliver_identity_email(&self, email: &str, code: &str, challenge: i64) -> Result<()> {
        let name = if self.schema.organization.name.is_empty() {
            "OpenB2C"
        } else {
            &self.schema.organization.name
        };
        let message = json!({"provider":env::var("EMAIL_PROVIDER").unwrap_or_else(|_|if production(){"resend"}else{"fake"}.into()),"to":email,"challengeId":challenge,"subject":format!("{name} sign-in code"),"text":format!("Your sign-in code for {name} is: {code}"),"html":format!("<p>Your sign-in code for {} is: <strong>{code}</strong></p>", crate::text::escape(name))});
        if message["provider"] == "fake" {
            if !fake_allowed() {
                return Err(ServiceError::new(
                    "invalid",
                    "Fake email is disabled in production",
                ));
            }
            self.db.execute(
                "INSERT INTO openb2c_fake_email(payload_json) VALUES (?)",
                [message.to_string()],
            )?;
            return Ok(());
        }
        if message["provider"] != "resend" {
            return Err(ServiceError::new("invalid", "Unsupported EMAIL_PROVIDER"));
        }
        let key = env::var("RESEND_API_KEY")
            .map_err(|_| ServiceError::new("invalid", "RESEND_API_KEY is required"))?;
        let from = env::var("EMAIL_FROM")
            .map_err(|_| ServiceError::new("invalid", "EMAIL_FROM is required"))?;
        let payload = json!({"from":from,"to":[email],"subject":message["subject"],"text":message["text"],"html":message["html"]});
        let response = client()?
            .post(
                env::var("RESEND_EMAILS_URL")
                    .unwrap_or_else(|_| "https://api.resend.com/emails".into()),
            )
            .bearer_auth(key)
            .header("idempotency-key", format!("identity-challenge-{challenge}"))
            .json(&payload)
            .send()
            .map_err(|_| ServiceError::new("internal_error", "Identity email delivery failed"))?;
        if !response.status().is_success() {
            return Err(ServiceError::new(
                "internal_error",
                format!("Identity email delivery failed ({})", response.status()),
            ));
        }
        Ok(())
    }
    fn deliver_effect(&self, kind: &str, payload: &Value, key: &str) -> Result<Value> {
        if kind == "emit" {
            eprintln!(
                "{}",
                json!({"level":"info","message":"effect emitted","payload":payload,"idempotencyKey":key})
            );
            return Ok(json!({"emitted":true}));
        }
        let email = kind == "notify" || (kind == "call" && payload["service"] == "email");
        let url = env::var(if email {
            "EMAIL_WEBHOOK_URL"
        } else {
            "WEBHOOK_URL"
        })
        .ok()
        .filter(|s| !s.is_empty());
        let Some(url) = url else {
            if fake_allowed() {
                if email {
                    self.db.execute(
                        "INSERT INTO openb2c_fake_email(payload_json) VALUES (?)",
                        [
                            json!({"provider":"fake","message":payload,"idempotencyKey":key})
                                .to_string(),
                        ],
                    )?;
                }
                return Ok(json!({"provider":"fake","idempotencyKey":key}));
            }
            return Err(ServiceError::new(
                "invalid",
                if email {
                    "EMAIL_WEBHOOK_URL is required"
                } else {
                    "WEBHOOK_URL is required"
                },
            ));
        };
        let body = payload.to_string();
        let mut request = client()?
            .post(url)
            .header("content-type", "application/json")
            .header("idempotency-key", key)
            .body(body.clone());
        let signing = &self
            .schema
            .metadata
            .get("integrations")
            .unwrap_or(&Value::Null)["webhookEffects"]["signing"];
        if !email && signing["enabled"] == true {
            let secret = env::var("WEBHOOK_SIGNING_SECRET").map_err(|_| {
                ServiceError::new(
                    "invalid",
                    "WEBHOOK_SIGNING_SECRET is required for signed deliveries",
                )
            })?;
            let ts = Utc::now().timestamp().to_string();
            let signature = hmac(&secret, format!("{ts}.{body}").as_bytes());
            request = request
                .header(
                    signing["timestampHeader"]
                        .as_str()
                        .unwrap_or("X-OpenB2C-Timestamp"),
                    &ts,
                )
                .header(
                    signing["signatureHeader"]
                        .as_str()
                        .unwrap_or("X-OpenB2C-Signature"),
                    format!("sha256={signature}"),
                );
        }
        let response = request
            .send()
            .map_err(|_| ServiceError::new("internal_error", "Effect delivery failed"))?;
        if !response.status().is_success() {
            return Err(ServiceError::new(
                "internal_error",
                format!("Effect delivery failed ({})", response.status()),
            ));
        }
        Ok(json!({"status":response.status().as_u16()}))
    }
    pub fn dispatch_effects(&self, force: bool) -> Result<Value> {
        // Import the transactional operation outbox into the compatibility ledger.
        let rows = self.query(
            "SELECT * FROM openb2c_rust_outbox WHERE status='pending' ORDER BY id",
            &[],
        )?;
        for row in rows {
            let effect: Value = serde_json::from_str(row["effect_json"].as_str().unwrap_or("null"))
                .map_err(|_| ServiceError::new("invalid", "Invalid pending effect"))?;
            let data: Value = serde_json::from_str(row["data_json"].as_str().unwrap_or("null"))
                .unwrap_or(Value::Null);
            let (kind, mut payload) = if let Some(kind) = effect["type"].as_str() {
                (kind.to_string(), effect["payload"].clone())
            } else if !effect["emit"].is_null() {
                ("emit".into(), effect["emit"].clone())
            } else if !effect["notify"].is_null() {
                (
                    "notify".into(),
                    json!({"template":effect["notify"]["template"]["template"],"channel":effect["notify"]["template"]["channel"],"to":effect["notify"]["to"]["recipient"]}),
                )
            } else {
                ("call".into(), effect["call"].clone())
            };
            if let Some(object) = payload.as_object_mut() {
                object.insert("data".into(), data);
            }
            let key = format!("operation-outbox:{}", row["id"]);
            self.queue_effect(&key,&kind,&payload,&json!({"entity":row["entity"],"operation":row["operation"],"recordId":row["record_id"]}))?;
            self.db.execute(
                "UPDATE openb2c_rust_outbox SET status='queued' WHERE id=?",
                [row["id"].as_i64()],
            )?;
        }
        let rows=self.query("SELECT * FROM openb2c_effect_attempt WHERE status IN ('pending','failed') AND (? OR next_attempt_at IS NULL OR julianday(next_attempt_at)<=julianday('now')) ORDER BY id LIMIT 100",&[rusqlite::types::Value::Integer(i64::from(force))])?;
        self.db.execute("UPDATE openb2c_effect_attempt SET status=CASE WHEN attempts>=max_attempts THEN 'dead_letter' ELSE 'failed' END WHERE status='processing' AND julianday(updated_at)<julianday('now','-1 minute')",[])?;
        let mut attempted = 0;
        let mut succeeded = 0;
        let mut failed = 0;
        let mut dead = 0;
        for row in &rows {
            crate::service::check_deadline()?;
            if self.db.execute("UPDATE openb2c_effect_attempt SET status='processing',updated_at=? WHERE id=? AND status IN ('pending','failed')",rusqlite::params![now(),row["id"].as_i64()])?!=1{continue;}
            attempted += 1;
            let payload: Value =
                serde_json::from_str(row["payload_json"].as_str().unwrap_or("null"))
                    .unwrap_or(Value::Null);
            let attempts = row["attempts"].as_i64().unwrap_or(0) + 1;
            let result = self.deliver_effect(
                row["effect_type"].as_str().unwrap_or(""),
                &payload,
                row["idempotency_key"].as_str().unwrap_or(""),
            );
            let (status, error, result) = match result {
                Ok(result) => {
                    succeeded += 1;
                    ("succeeded", None, Some(result.to_string()))
                }
                Err(error) => {
                    failed += 1;
                    if attempts >= row["max_attempts"].as_i64().unwrap_or(3) {
                        dead += 1;
                        ("dead_letter", Some(error.message), None)
                    } else {
                        ("failed", Some(error.message), None)
                    }
                }
            };
            let delay = env::var("EFFECT_RETRY_DELAY_MS")
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(1000)
                .max(1)
                .saturating_mul(2i64.saturating_pow((attempts - 1).min(20) as u32))
                .min(300000);
            self.db.execute("UPDATE openb2c_effect_attempt SET status=?,attempts=?,last_error=?,result_json=?,next_attempt_at=?,updated_at=? WHERE id=?",params![status,attempts,error,result,(Utc::now()+chrono::Duration::milliseconds(delay)).to_rfc3339(),now(),row["id"].as_i64()])?;
        }
        Ok(
            json!({"attempted":attempted,"succeeded":succeeded,"failed":failed,"deadLetter":dead,"skipped":0}),
        )
    }
    pub fn retry_effect(&self, id: i64) -> Result<()> {
        let status: Option<String> = self
            .db
            .query_row(
                "SELECT status FROM openb2c_effect_attempt WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if status.is_none() {
            return Err(ServiceError::new("not_found", "Effect not found"));
        }
        self.db.execute("UPDATE openb2c_effect_attempt SET status='pending',attempts=0,next_attempt_at=NULL WHERE id=? AND status!='succeeded'",[id])?;
        Ok(())
    }
}

/// A separate SQLite connection keeps slow providers out of the API connection.
pub struct Worker {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start(schema: openb2c_schema::Schema, path: &str) -> anyhow::Result<Self> {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if path == ":memory:" {
            return Ok(Self { stop, thread: None });
        }
        let db = rusqlite::Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON;")?;
        let service = Service { schema, db };
        let signal = stop.clone();
        let thread = std::thread::spawn(move || {
            while !signal.load(std::sync::atomic::Ordering::Relaxed) {
                let _deadline = crate::service::Deadline::enter(Duration::from_secs(20));
                if service.schema.capabilities.contains_key("subscriptions")
                    && let Err(error) = service.b2c_renew_due(&crate::AuthContext::system())
                {
                    eprintln!(
                        "{}",
                        json!({"level":"error","message":"Subscription billing worker failed","code":error.code})
                    );
                }
                if let Err(error) = service.dispatch_effects(false) {
                    eprintln!(
                        "{}",
                        json!({"level":"error","message":"Effect worker failed","code":error.code})
                    );
                }
                for _ in 0..10 {
                    if signal.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
