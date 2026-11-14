//! Supabase asymmetric signing keys (JWKS): ES256 tokens verified by `kid`,
//! with algorithm-confusion and unknown-key cases rejected.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axom_api::auth::{session_from_bearer, AuthConfig, AuthError};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use uuid::Uuid;

// Test-only P-256 key pair.
const PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgghI7bimEzzejL8T2
ETMm7GyK5JZ24kvzMnjgWh/GdHuhRANCAASNo0Ogz6DPO6/ywNLYLFn4mos0xsZX
nBn6b/ThDYfEEQofNMnJVlQDQnICyABDeLDrchvMyKRo9lcu8shMVmTv
-----END PRIVATE KEY-----
";
const X: &str = "jaNDoM-gzzuv8sDS2CxZ-JqLNMbGV5wZ-m_04Q2HxBE";
const Y: &str = "Ch80yclWVANCcgLIAEN4sOtyG8zIpGj2Vy7yyExWZO8";

async fn jwks_server() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = axum::Router::new().route(
        "/auth/v1/.well-known/jwks.json",
        axum::routing::get(move || {
            h.fetch_add(1, Ordering::SeqCst);
            async move { axum::Json(json!({ "keys": [{ "kty": "EC", "crv": "P-256", "x": X, "y": Y, "kid": "key-1", "alg": "ES256", "use": "sig" }] })) }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/auth/v1/.well-known/jwks.json", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, hits)
}

fn claims(aal: &str, exp_in: Duration) -> serde_json::Value {
    json!({ "sub": Uuid::new_v4(), "email": "owner@example.com", "aal": aal, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + exp_in).timestamp() })
}

fn es256(kid: &str, c: &serde_json::Value) -> String {
    let mut h = Header::new(jsonwebtoken::Algorithm::ES256);
    h.kid = Some(kid.into());
    encode(&h, c, &EncodingKey::from_ec_pem(PEM.as_bytes()).unwrap()).unwrap()
}

#[tokio::test]
async fn es256_tokens_verify_by_kid_and_bad_ones_are_rejected() {
    let (url, hits) = jwks_server().await;
    let cfg = AuthConfig::jwks(&url);
    let bearer = |t: String| format!("Bearer {t}");

    let s = session_from_bearer(&cfg, Some(&bearer(es256("key-1", &claims("aal2", Duration::hours(1)))))).await.unwrap();
    assert!(s.mfa_verified);
    let s = session_from_bearer(&cfg, Some(&bearer(es256("key-1", &claims("aal1", Duration::hours(1)))))).await.unwrap();
    assert!(!s.mfa_verified, "aal1 is not MFA");

    // Expired, unknown key, and missing token.
    assert_eq!(session_from_bearer(&cfg, Some(&bearer(es256("key-1", &claims("aal2", Duration::hours(-1)))))).await.err(), Some(AuthError::Invalid));
    assert_eq!(session_from_bearer(&cfg, Some(&bearer(es256("rotated-away", &claims("aal2", Duration::hours(1)))))).await.err(), Some(AuthError::Invalid));
    assert_eq!(session_from_bearer(&cfg, None).await.err(), Some(AuthError::Missing));

    // Algorithm confusion: an HS256 token "signed" with the public key is
    // rejected when no shared secret is configured.
    let forged = encode(&Header::default(), &claims("aal2", Duration::hours(1)), &EncodingKey::from_secret(X.as_bytes())).unwrap();
    assert_eq!(session_from_bearer(&cfg, Some(&bearer(forged))).await.err(), Some(AuthError::Invalid));

    // Keys are cached; unknown kids refresh at most once a minute.
    let fetches = hits.load(Ordering::SeqCst);
    assert!(fetches <= 2, "fetched {fetches} times");
    for _ in 0..5 {
        let _ = session_from_bearer(&cfg, Some(&bearer(es256("unknown", &claims("aal2", Duration::hours(1)))))).await;
    }
    assert_eq!(hits.load(Ordering::SeqCst), fetches);
}

#[tokio::test]
async fn jwks_and_legacy_secret_work_together_during_migration() {
    let (url, _) = jwks_server().await;
    let cfg = AuthConfig::jwks(&url).with_hs256(b"legacy-secret-legacy-secret-legacy!!");
    let legacy = encode(&Header::default(), &claims("aal2", Duration::hours(1)), &EncodingKey::from_secret(b"legacy-secret-legacy-secret-legacy!!")).unwrap();
    assert!(session_from_bearer(&cfg, Some(&format!("Bearer {legacy}"))).await.unwrap().mfa_verified);
    assert!(session_from_bearer(&cfg, Some(&format!("Bearer {}", es256("key-1", &claims("aal2", Duration::hours(1)))))).await.unwrap().mfa_verified);
    // A JWKS outage is reported as such.
    let down = AuthConfig::jwks("http://127.0.0.1:1/jwks.json");
    assert_eq!(session_from_bearer(&down, Some(&format!("Bearer {}", es256("key-1", &claims("aal2", Duration::hours(1)))))).await.err(), Some(AuthError::KeysUnavailable));
}
