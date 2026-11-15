mod common;
use common::service;
use ed25519_dalek::{Signer, SigningKey};
use openb2c_runtime::{
    AuthContext,
    identity::{Registry, after, now},
    service::ListOptions,
};
use serde_json::json;

#[test]
fn otp_hashes_key_proof_single_use_and_bearer_revocation() {
    let service = service();
    let registry = Registry::ephemeral().unwrap();
    let key = SigningKey::from_bytes(&[7; 32]);
    let public = hex::encode(key.verifying_key().to_bytes());
    let (id, code) = service
        .challenge("sign-in@example.test", &public, "127.0.0.1")
        .unwrap();
    let hash: String = service
        .db
        .query_row(
            "SELECT code_hash FROM identity_challenge WHERE id=?",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(hash, code);
    assert!(bcrypt::verify(&code, &hash).unwrap());
    assert_eq!(
        service
            .verify_challenge(&registry, id, &code, "00")
            .unwrap_err()
            .code,
        "invalid"
    );
    let signature = hex::encode(key.sign(code.as_bytes()).to_bytes());
    let result = service
        .verify_challenge(&registry, id, &code, &signature)
        .unwrap();
    let token = result["sessionToken"].as_str().unwrap();
    assert!(token.starts_with("sess_"));
    let cert: openb2c_runtime::identity::Certificate =
        serde_json::from_value(result["certificate"].clone()).unwrap();
    assert!(registry.verify(&cert));
    let auth = service.authenticate(token).unwrap();
    assert!(auth.user_id.is_some());
    assert!(auth.scopes.contains(&"booking.create".into()));
    assert!(
        service
            .verify_challenge(&registry, id, &code, &signature)
            .is_err()
    );
    service.revoke_session(token).unwrap();
    assert_eq!(
        service.authenticate(token).unwrap_err().code,
        "unauthorized"
    );
}
#[test]
fn challenge_rate_limits_email_key_and_ip() {
    let service = service();
    for i in 0..3 {
        service
            .challenge("rate@example.test", &format!("key-{i}"), &format!("ip-{i}"))
            .unwrap();
    }
    assert_eq!(
        service
            .challenge("rate@example.test", "fresh-key", "fresh-ip")
            .unwrap_err()
            .code,
        "rate_limited"
    );
    for i in 0..3 {
        service
            .challenge(
                &format!("keyrate{i}@example.test"),
                "shared-key",
                &format!("key-ip-{i}"),
            )
            .unwrap();
    }
    assert_eq!(
        service
            .challenge("keyrate4@example.test", "shared-key", "key-ip-4")
            .unwrap_err()
            .code,
        "rate_limited"
    );
    for i in 0..10 {
        service
            .challenge(
                &format!("iprate{i}@example.test"),
                &format!("ip-key-{i}"),
                "shared-ip",
            )
            .unwrap();
    }
    assert_eq!(
        service
            .challenge("iprate11@example.test", "ip-key-11", "shared-ip")
            .unwrap_err()
            .code,
        "rate_limited"
    );
}
#[test]
fn verification_attempts_are_limited_and_expired_challenges_fail() {
    let service = service();
    let registry = Registry::ephemeral().unwrap();
    let (id, _) = service
        .challenge("guess@example.test", "public-key", "ip")
        .unwrap();
    for _ in 0..5 {
        assert_eq!(
            service
                .verify_challenge(&registry, id, "wrong", "00")
                .unwrap_err()
                .code,
            "invalid"
        );
    }
    assert_eq!(
        service
            .verify_challenge(&registry, id, "wrong", "00")
            .unwrap_err()
            .code,
        "rate_limited"
    );
    let (id, code) = service
        .challenge("expired@example.test", "different-key", "ip2")
        .unwrap();
    service
        .db
        .execute(
            "UPDATE identity_challenge SET expires_at=? WHERE id=?",
            rusqlite::params![after(-60), id],
        )
        .unwrap();
    assert!(
        service
            .verify_challenge(&registry, id, &code, "00")
            .is_err()
    );
}
#[test]
fn certificates_reject_replay_expiry_revocation_rotation_and_wrong_signers() {
    let service = service();
    let registry = Registry::ephemeral().unwrap();
    let key = SigningKey::from_bytes(&[9; 32]);
    let public = hex::encode(key.verifying_key().to_bytes());
    let cert = registry.issue("cert@example.test", &public, 3600).unwrap();
    service
        .db
        .execute(
            "INSERT INTO identity_registry(email,public_key) VALUES (?,?)",
            [&cert.email, &public],
        )
        .unwrap();
    let ts = chrono::Utc::now().timestamp_millis().to_string();
    let message = format!("GET /auth/context {ts}");
    let signature = hex::encode(key.sign(message.as_bytes()).to_bytes());
    assert!(
        service
            .verify_request(&registry, &cert, "GET", "/auth/context", &ts, &signature)
            .is_ok()
    );
    assert!(
        service
            .verify_request(&registry, &cert, "GET", "/auth/context", &ts, &signature)
            .is_err()
    );
    let expired = registry.issue(&cert.email, &public, -1).unwrap();
    assert!(!registry.verify(&expired));
    let wrong = SigningKey::from_bytes(&[10; 32]);
    let ts = (chrono::Utc::now().timestamp_millis() + 1).to_string();
    let message = format!("GET /auth/context {ts}");
    let wrong_signature = hex::encode(wrong.sign(message.as_bytes()).to_bytes());
    assert!(
        service
            .verify_request(
                &registry,
                &cert,
                "GET",
                "/auth/context",
                &ts,
                &wrong_signature
            )
            .is_err()
    );
    service
        .db
        .execute(
            "UPDATE identity_registry SET revoked=1 WHERE email=?",
            [&cert.email],
        )
        .unwrap();
    let signature = hex::encode(key.sign(message.as_bytes()).to_bytes());
    assert!(
        service
            .verify_request(&registry, &cert, "GET", "/auth/context", &ts, &signature)
            .is_err()
    );
    service
        .db
        .execute(
            "UPDATE identity_registry SET revoked=0,public_key=? WHERE email=?",
            [
                hex::encode(wrong.verifying_key().to_bytes()),
                cert.email.clone(),
            ],
        )
        .unwrap();
    assert!(
        service
            .verify_request(&registry, &cert, "GET", "/auth/context", &ts, &signature)
            .is_err()
    );
    assert!(!now().is_empty());
}
#[test]
fn api_keys_cannot_escalate_scopes_and_secrets_are_only_shown_once() {
    let service = service();
    let customer = AuthContext {
        user_id: Some(1),
        scopes: service.self_scopes(),
    };
    assert_eq!(
        service
            .create_api_key(&json!({"name":"Escalation","scopes":"*"}), &customer)
            .unwrap_err()
            .code,
        "forbidden"
    );
    let created = service
        .create_api_key(
            &json!({"name":"Account reader","scopes":"ticket.read"}),
            &customer,
        )
        .unwrap();
    let key = created["key"].as_str().unwrap();
    assert!(!created.as_object().unwrap().contains_key("key_hash"));
    assert_eq!(
        service.authenticate(key).unwrap().scopes,
        vec!["ticket.read"]
    );
    assert!(
        service
            .mutate(
                "api_key",
                &created["id"],
                "update",
                &json!({"scopes":"*"}),
                &customer,
                None
            )
            .is_err()
    );
    let listed = service
        .list("api_key", &ListOptions::default(), &customer)
        .unwrap();
    assert!(!listed.to_string().contains(key));
    service
        .mutate(
            "api_key",
            &created["id"],
            "revoke",
            &json!({}),
            &customer,
            None,
        )
        .unwrap();
    assert!(service.authenticate(key).is_err());
}
