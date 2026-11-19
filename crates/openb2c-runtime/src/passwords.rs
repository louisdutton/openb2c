//! Password authentication is a platform policy for independent accounts and staff memberships.
use crate::{
    AuthContext, Service, identity,
    service::{Result, ServiceError},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use openb2c_schema::sql::quote;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

const SESSION_SECONDS: i64 = 3600;
static EMAIL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap());
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("internal-dummy-password-no-account").expect("Argon2 parameters are valid")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Signup {
    email: String,
    password: String,
    name: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    email: String,
    password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    current_password: String,
    password: String,
}

fn invalid(message: &str) -> ServiceError {
    ServiceError::new("invalid", message)
}
fn unauthorized() -> ServiceError {
    ServiceError::new("unauthorized", "Invalid credentials or expired session")
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn decode<T: serde::de::DeserializeOwned>(input: &Value) -> Result<T> {
    serde_json::from_value(input.clone()).map_err(|e| invalid(&e.to_string()))
}
pub(crate) fn email(value: &str) -> Result<String> {
    let value = value.trim().to_lowercase();
    if value.len() > 254 || !EMAIL.is_match(&value) {
        return Err(invalid("A valid email address is required"));
    }
    Ok(value)
}
fn new_password(value: &str) -> Result<()> {
    if !(15..=128).contains(&value.chars().count()) {
        return Err(invalid("Passwords must contain 15 to 128 characters"));
    }
    if [
        "passwordpassword",
        "123456789012345",
        "qwertyuiopasdfgh",
        "letmeinletmeinletmein",
        "correct horse battery staple",
    ]
    .contains(&value.to_lowercase().as_str())
    {
        return Err(invalid("Choose a less common password"));
    }
    Ok(())
}
fn hash_password(value: &str) -> Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| ServiceError::new("internal_error", "Random source unavailable"))?;
    let salt = SaltString::encode_b64(&random)
        .map_err(|_| ServiceError::new("internal_error", "Could not prepare password salt"))?;
    Argon2::default()
        .hash_password(value.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ServiceError::new("internal_error", "Could not hash password"))
}
fn matches(value: &str, hash: &str) -> bool {
    if value.chars().count() > 128 {
        return false;
    }
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(value.as_bytes(), &hash)
            .is_ok()
    })
}

impl Service {
    fn password_table(&self, role: &str) -> Result<&str> {
        self.entity("password_auth", role)
            .ok_or_else(|| ServiceError::new("not_found", "Password authentication is not enabled"))
    }
    // Persist reservations before checking secrets, including failed requests.
    fn password_attempt(&self, account: &str, peer: &str) -> Result<()> {
        let table = quote(self.password_table("attempt")?);
        let account = digest(account);
        let peer = digest(peer);
        let tx = crate::service::transaction(&self.db)?;
        self.db.execute(
            &format!(
                "DELETE FROM {table} WHERE julianday(created_at) <= julianday('now','-15 minutes')"
            ),
            [],
        )?;
        let accounts: i64 = self.db.query_row(
            &format!("SELECT count(*) FROM {table} WHERE account_key=?"),
            [&account],
            |r| r.get(0),
        )?;
        let peers: i64 = self.db.query_row(
            &format!("SELECT count(*) FROM {table} WHERE peer_key=?"),
            [&peer],
            |r| r.get(0),
        )?;
        if accounts >= 10 || peers >= 100 {
            return Err(ServiceError::new(
                "rate_limited",
                "Too many authentication attempts; try again later",
            ));
        }
        self.db.execute(
            &format!("INSERT INTO {table} (account_key,peer_key) VALUES (?,?)"),
            params![account, peer],
        )?;
        crate::service::commit(tx)
    }
    fn password_session(&self, user: i64) -> Result<Value> {
        if self.account_scopes(user)?.is_empty() {
            return Err(unauthorized());
        }
        let token = identity::secret("pwd_")?;
        let expires = identity::after(SESSION_SECONDS);
        self.db.execute(
            &format!(
                "INSERT INTO {} (user_id,token_hash,expires_at) VALUES (?,?,?)",
                quote(self.password_table("session")?)
            ),
            params![user, digest(&token), expires],
        )?;
        let customer = self
            .entity("accounts", "account")
            .ok_or_else(unauthorized)?;
        let (email, name): (String, String) = self.db.query_row(
            &format!("SELECT email,name FROM {} WHERE id=?", quote(customer)),
            [user],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(
            json!({"access_token":token,"token_type":"Bearer","expires_in":SESSION_SECONDS,"expires_at":expires,"user":{"id":user,"email":email,"name":name}}),
        )
    }
    pub fn password_signup(&self, input: &Value, peer: &str) -> Result<Value> {
        let membership = self
            .entity("customers", "customer")
            .ok_or_else(|| ServiceError::new("not_found", "Customer signup is not enabled"))?;
        self.password_table("credential")?;
        let input: Signup = decode(input)?;
        let email = email(&input.email)?;
        self.password_attempt(&email, peer)?;
        new_password(&input.password)?;
        let name = input
            .name
            .unwrap_or_else(|| email.split('@').next().unwrap().chars().take(120).collect());
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(invalid("Name must contain 1 to 120 characters"));
        }
        let customer = self
            .entity("accounts", "account")
            .ok_or_else(unauthorized)?;
        #[cfg(feature = "reference")]
        let data = json!({"email":email,"name":name});
        #[cfg(feature = "reference")]
        self.validate_input(customer, data.as_object().unwrap(), true)?;
        let hash = hash_password(&input.password)?;
        let tx = crate::service::transaction(&self.db)?;
        let existing: bool = self.db.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE lower(email)=?)",
                quote(customer)
            ),
            [&email],
            |r| r.get(0),
        )?;
        // Existing operator-provisioned customers cannot be claimed by public signup.
        if existing {
            return Err(ServiceError::new(
                "conflict",
                "Registration is unavailable for these details",
            ));
        }
        #[cfg(feature = "reference")]
        self.db.execute(
            &format!("INSERT INTO {} (email,name) VALUES (?,?)", quote(customer)),
            params![email, name],
        )?;
        #[cfg(feature = "reference")]
        let user = self.db.last_insert_rowid();
        #[cfg(not(feature = "reference"))]
        let user = self.insert_signup_account(&email, name)?;
        self.db.execute(
            &format!("INSERT INTO {} (user_id) VALUES (?)", quote(membership)),
            [user],
        )?;
        self.db.execute(
            &format!(
                "INSERT INTO {} (user_id,password_hash) VALUES (?,?)",
                quote(self.password_table("credential")?)
            ),
            params![user, hash],
        )?;
        let response = self.password_session(user)?;
        crate::service::commit(tx)?;
        Ok(response)
    }
    pub fn recover_password(
        &self,
        account_email: &str,
        password: &str,
        auth: &AuthContext,
    ) -> Result<Value> {
        if !auth.has_scope("*") {
            return Err(ServiceError::new(
                "forbidden",
                "Local owner authority is required",
            ));
        }
        let email = email(account_email)?;
        new_password(password)?;
        let hash = hash_password(password)?;
        let tx = crate::service::transaction(&self.db)?;
        let user = self
            .db
            .query_row("SELECT id FROM user WHERE lower(email)=?", [&email], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .ok_or_else(unauthorized)?;
        let credential = self.password_table("credential")?;
        self.db.execute(&format!("INSERT INTO {} (user_id,password_hash) VALUES (?,?) ON CONFLICT(user_id) DO UPDATE SET password_hash=excluded.password_hash",quote(credential)),params![user,hash])?;
        self.db.execute(
            &format!(
                "UPDATE {} SET revoked=1 WHERE user_id=?",
                quote(self.password_table("session")?)
            ),
            [user],
        )?;
        self.db
            .execute("UPDATE api_key SET active=0 WHERE user_id=?", [user])?;
        self.db.execute(
            &format!(
                "DELETE FROM {} WHERE account_key=?",
                quote(self.password_table("attempt")?)
            ),
            [digest(&email)],
        )?;
        self.cap_audit(
            credential,
            "accounts.recover_password",
            auth,
            None,
            &json!({"id":user,"user_id":user}),
        )?;
        crate::service::commit(tx)?;
        Ok(json!({"user_id":user,"sessions_revoked":true,"api_keys_revoked":true}))
    }
    pub fn password_login(&self, input: &Value, peer: &str) -> Result<Value> {
        let credentials = quote(self.password_table("credential")?);
        let input: Login = decode(input)?;
        let email = email(&input.email)?;
        self.password_attempt(&email, peer)?;
        let customer = quote(
            self.entity("accounts", "account")
                .ok_or_else(unauthorized)?,
        );
        let account: Option<(i64,String)> = self.db.query_row(&format!("SELECT u.id,p.password_hash FROM {customer} u JOIN {credentials} p ON p.user_id=u.id WHERE lower(u.email)=?"), [&email], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let verified = matches(
            &input.password,
            account
                .as_ref()
                .map(|(_, hash)| hash.as_str())
                .unwrap_or(&DUMMY_HASH),
        );
        let Some((user, hash)) = account.filter(|_| verified) else {
            return Err(unauthorized());
        };
        let tx = crate::service::transaction(&self.db)?;
        // A concurrent password change invalidates credentials verified before this transaction.
        let unchanged: bool = self.db.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {credentials} WHERE user_id=? AND password_hash=?)"
            ),
            params![user, hash],
            |r| r.get(0),
        )?;
        if !unchanged {
            return Err(unauthorized());
        }
        let response = self.password_session(user)?;
        crate::service::commit(tx)?;
        Ok(response)
    }
    pub(crate) fn authenticate_password_session(&self, token: &str) -> Result<AuthContext> {
        if token.len() != 68 || !token.starts_with("pwd_") {
            return Err(unauthorized());
        }
        let table = self
            .entity("password_auth", "session")
            .ok_or_else(unauthorized)?;
        let user: Option<i64> = self.db.query_row(&format!("SELECT user_id FROM {} WHERE token_hash=? AND revoked=0 AND julianday(expires_at)>julianday('now')", quote(table)), [digest(token)], |r| r.get(0)).optional()?;
        let user = user.ok_or_else(unauthorized)?;
        let mut scopes = self.account_scopes(user)?;
        if scopes.is_empty() {
            return Err(unauthorized());
        }
        scopes.push("auth.session".into());
        Ok(AuthContext {
            user_id: Some(user),
            scopes,
        })
    }
    pub fn password_logout(&self, token: &str) -> Result<Value> {
        self.authenticate_password_session(token)?;
        self.db.execute(
            &format!(
                "UPDATE {} SET revoked=1 WHERE token_hash=?",
                quote(self.password_table("session")?)
            ),
            [digest(token)],
        )?;
        Ok(json!({"revoked":true}))
    }
    pub fn password_change(&self, token: &str, input: &Value, peer: &str) -> Result<Value> {
        let auth = self.authenticate_password_session(token)?;
        let user = auth.user_id.ok_or_else(unauthorized)?;
        let input: Change = decode(input)?;
        let customer = self
            .entity("accounts", "account")
            .ok_or_else(unauthorized)?;
        let email: String = self.db.query_row(
            &format!("SELECT email FROM {} WHERE id=?", quote(customer)),
            [user],
            |row| row.get(0),
        )?;
        self.password_attempt(&email, peer)?;
        let table = quote(self.password_table("credential")?);
        let old: String = self.db.query_row(
            &format!("SELECT password_hash FROM {table} WHERE user_id=?"),
            [user],
            |r| r.get(0),
        )?;
        if !matches(&input.current_password, &old) {
            return Err(unauthorized());
        }
        new_password(&input.password)?;
        let hash = hash_password(&input.password)?;
        let tx = crate::service::transaction(&self.db)?;
        self.authenticate_password_session(token)?;
        if self.db.execute(
            &format!("UPDATE {table} SET password_hash=? WHERE user_id=? AND password_hash=?"),
            params![hash, user, old],
        )? != 1
        {
            return Err(unauthorized());
        }
        self.db.execute(
            &format!(
                "UPDATE {} SET revoked=1 WHERE user_id=?",
                quote(self.password_table("session")?)
            ),
            [user],
        )?;
        let response = self.password_session(user)?;
        crate::service::commit(tx)?;
        Ok(response)
    }
}
