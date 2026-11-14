//! Signed payout authorizations.
//!
//! The signer accepts a payout only with one of:
//! - an automatic under-threshold attestation signed by the policy engine's
//!   attestation key (held by axom-hr), or
//! - a human approval signed by the dashboard's approval key (held by
//!   axom-api), issued only for an MFA-verified named admin.
//!
//! The signer holds only the two public keys, so a compromised signer or
//! database cannot mint authorizations.

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::intents::Destination;
use crate::near::{NearError, PublicKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationKind {
    AutoAttestation,
    HumanApproval,
}

impl AuthorizationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorizationKind::AutoAttestation => "auto_attestation",
            AuthorizationKind::HumanApproval => "human_approval",
        }
    }
}

/// What is being authorized. Field order is fixed; the signed message is this
/// struct's JSON encoding prefixed with a domain tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayoutAuthorization {
    pub kind: AuthorizationKind,
    pub payout_id: Uuid,
    pub network: String,
    pub recipient: String,
    /// yoctoNEAR as a decimal string.
    pub amount_yocto: String,
    /// USD amount as a decimal string.
    pub amount_usd: String,
    pub policy_version: String,
    /// For human approvals: the approval record and the admin who approved.
    pub approval_id: Option<Uuid>,
    pub approver: Option<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// For cross-chain payouts (NEAR Intents): the chain, asset, and address
    /// the worker is paid at. `recipient` stays the worker's verified NEAR
    /// account, whose key signed this destination. Omitted from the signed
    /// message when absent, so NEAR-only authorizations are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<Destination>,
}

const DOMAIN: &[u8] = b"axom-payout-authorization:v1:";

impl PayoutAuthorization {
    pub fn message(&self) -> Vec<u8> {
        let mut m = DOMAIN.to_vec();
        m.extend_from_slice(&serde_json::to_vec(self).expect("authorization serializes"));
        m
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedAuthorization {
    pub payload: PayoutAuthorization,
    /// Hex-encoded ed25519 signature over `payload.message()`.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthzError {
    #[error("bad key: {0}")]
    BadKey(String),
    #[error("signature does not verify")]
    BadSignature,
    #[error("authorization expired at {0}")]
    Expired(DateTime<Utc>),
}

/// An ed25519 signing key for authorizations, loaded from a hex seed.
pub struct AuthorizationKey(SigningKey);

impl AuthorizationKey {
    pub fn from_hex_seed(hex_seed: &str) -> Result<Self, AuthzError> {
        let bytes = hex::decode(hex_seed.trim()).map_err(|e| AuthzError::BadKey(e.to_string()))?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| AuthzError::BadKey("seed must be 32 bytes".into()))?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }

    /// Load from an environment variable holding a hex seed. The value is
    /// never logged.
    pub fn from_env(var: &str) -> Result<Self, AuthzError> {
        let v = std::env::var(var).map_err(|_| AuthzError::BadKey(format!("{var} is not set")))?;
        Self::from_hex_seed(&v)
    }

    pub fn generate() -> Self {
        Self(SigningKey::generate(&mut rand::rngs::OsRng))
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }

    pub fn seed_hex(&self) -> String {
        hex::encode(self.0.to_bytes())
    }

    pub fn sign(&self, payload: PayoutAuthorization) -> SignedAuthorization {
        let sig = self.0.sign(&payload.message());
        SignedAuthorization { payload, signature: hex::encode(sig.to_bytes()) }
    }
}

impl std::fmt::Debug for AuthorizationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AuthorizationKey({})", self.public_key().to_near_string())
    }
}

/// Verify the signature and expiry. The caller still checks every field
/// against the payout record.
pub fn verify(signed: &SignedAuthorization, key: &PublicKey, now: DateTime<Utc>) -> Result<(), AuthzError> {
    let bytes = hex::decode(&signed.signature).map_err(|_| AuthzError::BadSignature)?;
    let sig: [u8; 64] = bytes.try_into().map_err(|_| AuthzError::BadSignature)?;
    key.verify(&signed.payload.message(), &sig).map_err(|_: NearError| AuthzError::BadSignature)?;
    if now > signed.payload.expires_at {
        return Err(AuthzError::Expired(signed.payload.expires_at));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn payload() -> PayoutAuthorization {
        PayoutAuthorization {
            kind: AuthorizationKind::AutoAttestation,
            payout_id: Uuid::new_v4(),
            network: "testnet".into(),
            recipient: "bob.testnet".into(),
            amount_yocto: "1000".into(),
            amount_usd: "10".into(),
            policy_version: "v+1".into(),
            approval_id: None,
            approver: None,
            issued_at: Utc::now(),
            expires_at: Utc::now() + Duration::hours(1),
            destination: None,
        }
    }

    #[test]
    fn sign_and_verify() {
        let k = AuthorizationKey::generate();
        let s = k.sign(payload());
        verify(&s, &k.public_key(), Utc::now()).unwrap();
        // Wrong key.
        assert_eq!(verify(&s, &AuthorizationKey::generate().public_key(), Utc::now()), Err(AuthzError::BadSignature));
        // Tampered field.
        let mut t = s.clone();
        t.payload.amount_yocto = "999999".into();
        assert_eq!(verify(&t, &k.public_key(), Utc::now()), Err(AuthzError::BadSignature));
        // Expired.
        assert!(matches!(verify(&s, &k.public_key(), Utc::now() + Duration::hours(2)), Err(AuthzError::Expired(_))));
        // The destination is covered by the signature, and its absence keeps
        // the NEAR-only message unchanged.
        assert!(!String::from_utf8_lossy(&s.payload.message()).contains("destination"));
        let mut t = s.clone();
        t.payload.destination = Some(Destination { chain: "base".into(), asset_id: "nep141:x".into(), address: "0x1".into() });
        assert_eq!(verify(&t, &k.public_key(), Utc::now()), Err(AuthzError::BadSignature));
        let mut p2 = payload();
        p2.destination = Some(Destination { chain: "base".into(), asset_id: "nep141:x".into(), address: "0x1".into() });
        let s2 = k.sign(p2);
        verify(&s2, &k.public_key(), Utc::now()).unwrap();
        let mut t2 = s2.clone();
        t2.payload.destination.as_mut().unwrap().address = "0x2".into();
        assert_eq!(verify(&t2, &k.public_key(), Utc::now()), Err(AuthzError::BadSignature));
        // Seed round trip.
        let k2 = AuthorizationKey::from_hex_seed(&k.seed_hex()).unwrap();
        assert_eq!(k2.public_key(), k.public_key());
    }
}
