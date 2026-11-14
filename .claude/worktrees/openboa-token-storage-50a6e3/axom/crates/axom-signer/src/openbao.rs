//! OpenBao transit client. The treasury key is an ed25519 transit key that
//! is not exportable and cannot be deleted; the private key never leaves
//! OpenBao. The signer's token may only sign with that key and read its
//! public key (see `infra/openbao/axom-signer.hcl`).

use async_trait::async_trait;
use axom_core::near::PublicKey;
use base64::Engine;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransitError {
    #[error("OpenBao is sealed; payouts are paused until the owner unseals it")]
    Sealed,
    #[error("OpenBao unavailable: {0}")]
    Unavailable(String),
    #[error("OpenBao refused the request (permission denied)")]
    Forbidden,
    #[error("unexpected OpenBao response: {0}")]
    Bad(String),
}

#[async_trait]
pub trait TransitSigner: Send + Sync {
    async fn is_sealed(&self) -> Result<bool, TransitError>;
    async fn public_key(&self) -> Result<PublicKey, TransitError>;
    /// ed25519 signature over `message` (the raw bytes, not prehashed).
    async fn sign(&self, message: &[u8]) -> Result<[u8; 64], TransitError>;
}

pub struct OpenBaoTransit {
    client: reqwest::Client,
    addr: String,
    token: String,
    key: String,
}

impl std::fmt::Debug for OpenBaoTransit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("OpenBaoTransit").field("addr", &self.addr).field("key", &self.key).finish_non_exhaustive()
    }
}

/// The signer's OpenBao token, from `OPENBAO_TOKEN` only. It is never read
/// from a file: a leftover `OPENBAO_TOKEN_FILE` is refused so a stale setup
/// fails loudly instead of silently running without payouts.
pub fn token_from_env() -> anyhow::Result<String> {
    // (Empty counts as unset: older .env files have `OPENBAO_TOKEN_FILE=`.)
    if std::env::var_os("OPENBAO_TOKEN_FILE").is_some_and(|v| !v.is_empty()) {
        anyhow::bail!("OPENBAO_TOKEN_FILE is no longer supported: set OPENBAO_TOKEN (in .env, or your deployment's environment) and delete the token file");
    }
    match std::env::var("OPENBAO_TOKEN") {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        _ => anyhow::bail!("OPENBAO_TOKEN is not set"),
    }
}

impl OpenBaoTransit {
    pub fn new(addr: impl Into<String>, token: impl Into<String>, key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().expect("http client");
        Self { client, addr: addr.into().trim_end_matches('/').to_string(), token: token.into(), key: key.into() }
    }

    /// From `OPENBAO_ADDR`, `OPENBAO_TOKEN` (see [`token_from_env`]), and
    /// `OPENBAO_TRANSIT_KEY` (default `axom-treasury`).
    pub fn from_env() -> anyhow::Result<Self> {
        let addr = std::env::var("OPENBAO_ADDR").map_err(|_| anyhow::anyhow!("OPENBAO_ADDR is not set"))?;
        let token = token_from_env()?;
        let key = std::env::var("OPENBAO_TRANSIT_KEY").unwrap_or_else(|_| "axom-treasury".into());
        Ok(Self::new(addr, token, key))
    }

    /// Create this key as a company treasury: ed25519, not exportable, no
    /// plaintext backup, deletion not allowed. Idempotent. Needs the
    /// provisioner policy (`infra/openbao/axom-provisioner.hcl`), which can
    /// create and configure `axom-treasury-*` keys but not sign, export, or
    /// delete them.
    pub async fn create_treasury_key(&self) -> Result<PublicKey, TransitError> {
        self.request(
            reqwest::Method::POST,
            &format!("/v1/transit/keys/{}", self.key),
            Some(json!({ "type": "ed25519", "exportable": false, "allow_plaintext_backup": false })),
        )
        .await?;
        self.request(reqwest::Method::POST, &format!("/v1/transit/keys/{}/config", self.key), Some(json!({ "deletion_allowed": false, "exportable": false }))).await?;
        self.public_key().await
    }

    /// Extend this token's life. The signer's token is periodic (it expires
    /// one period after its last renewal, 32 days as created by
    /// `setup-treasury.sh`), so a signer that never renews stops being able
    /// to sign a month after setup. Needs `auth/token/renew-self` in the
    /// token's policy (`infra/openbao/axom-signer.hcl`).
    pub async fn renew_self(&self) -> Result<(), TransitError> {
        self.request(reqwest::Method::POST, "/v1/auth/token/renew-self", Some(json!({}))).await.map(|_| ())
    }

    async fn request(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, TransitError> {
        let mut req = self.client.request(method, format!("{}{path}", self.addr)).header("X-Vault-Token", &self.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| TransitError::Unavailable(e.without_url().to_string()))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        let errors = v.get("errors").map(|e| e.to_string()).unwrap_or_default();
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE || errors.contains("sealed") {
            return Err(TransitError::Sealed);
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(TransitError::Forbidden);
        }
        if !status.is_success() {
            return Err(TransitError::Bad(format!("HTTP {status}: {errors}")));
        }
        Ok(v)
    }
}

#[async_trait]
impl TransitSigner for OpenBaoTransit {
    async fn is_sealed(&self) -> Result<bool, TransitError> {
        let resp = self
            .client
            .get(format!("{}/v1/sys/seal-status", self.addr))
            .send()
            .await
            .map_err(|e| TransitError::Unavailable(e.without_url().to_string()))?;
        let v: Value = resp.json().await.map_err(|e| TransitError::Bad(e.without_url().to_string()))?;
        v.get("sealed").and_then(Value::as_bool).ok_or_else(|| TransitError::Bad("seal-status has no sealed field".into()))
    }

    async fn public_key(&self) -> Result<PublicKey, TransitError> {
        let v = self.request(reqwest::Method::GET, &format!("/v1/transit/keys/{}", self.key), None).await?;
        let data = v.get("data").ok_or_else(|| TransitError::Bad("no data".into()))?;
        if data.get("type").and_then(Value::as_str) != Some("ed25519") {
            return Err(TransitError::Bad("treasury key is not ed25519".into()));
        }
        let latest = data.get("latest_version").and_then(Value::as_u64).ok_or_else(|| TransitError::Bad("no latest_version".into()))?;
        let b64 = data
            .pointer(&format!("/keys/{latest}/public_key"))
            .and_then(Value::as_str)
            .ok_or_else(|| TransitError::Bad("no public_key".into()))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| TransitError::Bad(e.to_string()))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| TransitError::Bad("public key is not 32 bytes".into()))?;
        Ok(PublicKey(arr))
    }

    async fn sign(&self, message: &[u8]) -> Result<[u8; 64], TransitError> {
        let input = base64::engine::general_purpose::STANDARD.encode(message);
        let v = self.request(reqwest::Method::POST, &format!("/v1/transit/sign/{}", self.key), Some(json!({ "input": input }))).await?;
        let sig = v.pointer("/data/signature").and_then(Value::as_str).ok_or_else(|| TransitError::Bad("no signature".into()))?;
        // Format: "vault:v<version>:<base64>".
        let b64 = sig.rsplit(':').next().unwrap_or_default();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| TransitError::Bad(e.to_string()))?;
        bytes.try_into().map_err(|_| TransitError::Bad("signature is not 64 bytes".into()))
    }
}
