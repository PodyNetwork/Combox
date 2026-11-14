//! Supabase Auth session verification for the dashboard API.
//!
//! The bearer token is a Supabase access token (JWT). Two verification modes,
//! usable together during a key migration:
//! - JWKS (`SUPABASE_JWKS_URL`, e.g. `<SUPABASE_URL>/auth/v1/.well-known/jwks.json`):
//!   asymmetric signing keys (ES256 or RS256), selected by `kid`, cached and
//!   refreshed when an unknown `kid` appears (at most once a minute).
//! - Legacy shared secret (`SUPABASE_JWT_SECRET`): HS256.
//!
//! The algorithm is taken from the verifying key, never trusted from the
//! token alone, so an HS256 token can't be checked against a public key.
//! `aal` is `aal2` only after the user completed TOTP MFA in this session;
//! approvals and all admin routes require it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axom_hr::approvals::AdminSession;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Default)]
struct JwksCache {
    keys: HashMap<String, (DecodingKey, Algorithm)>,
    fetched: Option<Instant>,
}

#[derive(Clone)]
pub struct AuthConfig {
    hs256: Option<DecodingKey>,
    jwks_url: Option<String>,
    jwks: Arc<RwLock<JwksCache>>,
    client: reqwest::Client,
}

impl AuthConfig {
    fn empty() -> Self {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client");
        Self { hs256: None, jwks_url: None, jwks: Arc::default(), client }
    }

    pub fn hs256(secret: &[u8]) -> Self {
        Self { hs256: Some(DecodingKey::from_secret(secret)), ..Self::empty() }
    }

    pub fn jwks(url: impl Into<String>) -> Self {
        Self { jwks_url: Some(url.into()), ..Self::empty() }
    }

    pub fn with_hs256(mut self, secret: &[u8]) -> Self {
        self.hs256 = Some(DecodingKey::from_secret(secret));
        self
    }

    /// From `SUPABASE_JWKS_URL` and/or `SUPABASE_JWT_SECRET`.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("SUPABASE_JWKS_URL").ok().filter(|s| !s.is_empty());
        let secret = std::env::var("SUPABASE_JWT_SECRET").ok().filter(|s| !s.is_empty());
        match (url, secret) {
            (Some(u), Some(s)) => Some(Self::jwks(u).with_hs256(s.as_bytes())),
            (Some(u), None) => Some(Self::jwks(u)),
            (None, Some(s)) => Some(Self::hs256(s.as_bytes())),
            (None, None) => None,
        }
    }

    fn key_from_jwk(jwk: &Jwk) -> Option<(DecodingKey, Algorithm)> {
        use jsonwebtoken::jwk::AlgorithmParameters as P;
        let alg = match &jwk.algorithm {
            P::EllipticCurve(_) => Algorithm::ES256,
            P::RSA(_) => Algorithm::RS256,
            _ => return None, // symmetric keys are never accepted from a JWKS
        };
        DecodingKey::from_jwk(jwk).ok().map(|k| (k, alg))
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        let Some(url) = &self.jwks_url else { return Ok(()) };
        {
            let c = self.jwks.read().await;
            if c.fetched.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
                return Ok(());
            }
        }
        let set: JwkSet = self.client.get(url).send().await.map_err(|_| AuthError::KeysUnavailable)?.json().await.map_err(|_| AuthError::KeysUnavailable)?;
        let mut c = self.jwks.write().await;
        c.keys = set.keys.iter().filter_map(|k| Some((k.common.key_id.clone()?, Self::key_from_jwk(k)?))).collect();
        c.fetched = Some(Instant::now());
        Ok(())
    }

    async fn key_for(&self, kid: &str) -> Result<Option<(DecodingKey, Algorithm)>, AuthError> {
        if let Some(k) = self.jwks.read().await.keys.get(kid) {
            return Ok(Some(k.clone()));
        }
        self.refresh().await?;
        Ok(self.jwks.read().await.keys.get(kid).cloned())
    }
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: Uuid,
    #[serde(default)]
    email: String,
    #[serde(default)]
    aal: String,
    #[serde(default)]
    session_id: String,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("missing bearer token")]
    Missing,
    #[error("invalid or expired token")]
    Invalid,
    #[error("signing keys unavailable")]
    KeysUnavailable,
}

fn validation(alg: Algorithm) -> Validation {
    let mut v = Validation::new(alg);
    v.set_audience(&["authenticated"]);
    v
}

/// Verify a bearer token and return the session it represents.
pub async fn session_from_bearer(cfg: &AuthConfig, header: Option<&str>) -> Result<AdminSession, AuthError> {
    let token = header.and_then(|h| h.strip_prefix("Bearer ")).ok_or(AuthError::Missing)?;
    let head = decode_header(token).map_err(|_| AuthError::Invalid)?;
    let claims = match (head.alg, head.kid.as_deref()) {
        (Algorithm::HS256, _) => {
            let key = cfg.hs256.as_ref().ok_or(AuthError::Invalid)?;
            decode::<Claims>(token, key, &validation(Algorithm::HS256)).map_err(|_| AuthError::Invalid)?.claims
        }
        (Algorithm::ES256 | Algorithm::RS256, Some(kid)) => {
            let (key, alg) = cfg.key_for(kid).await?.ok_or(AuthError::Invalid)?;
            if alg != head.alg {
                return Err(AuthError::Invalid);
            }
            decode::<Claims>(token, &key, &validation(alg)).map_err(|_| AuthError::Invalid)?.claims
        }
        _ => return Err(AuthError::Invalid),
    };
    Ok(AdminSession { user_id: claims.sub, email: claims.email, session_id: claims.session_id, mfa_verified: claims.aal == "aal2" })
}
