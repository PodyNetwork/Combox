//! Encryption of stored secrets (companies' integration keys, custom tool
//! tokens) at rest.
//!
//! AES-256-GCM under a master key from `AXOM_SECRETS_KEY` (32 bytes, hex).
//! Each value gets a fresh random 96-bit nonce, and a context string (for
//! example the company id and key name) is bound in as associated data, so a
//! ciphertext copied to another context fails to decrypt.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use uuid::Uuid;

#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("AXOM_SECRETS_KEY is not set (generate one with `axom secrets-key`)")]
    Missing,
    #[error("AXOM_SECRETS_KEY must be 64 hex characters (32 bytes); generate one with `axom secrets-key`")]
    BadKey,
    #[error("could not decrypt the stored value (wrong key or tampered data)")]
    Decrypt,
}

impl SecretBox {
    pub fn from_hex(key_hex: &str) -> Result<Self, CryptoError> {
        // Tolerate the quotes some .env loaders leave in place.
        let key_hex = key_hex.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let bytes = hex::decode(key_hex).map_err(|_| CryptoError::BadKey)?;
        if bytes.len() != 32 {
            return Err(CryptoError::BadKey);
        }
        Ok(Self { cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&bytes)) })
    }

    /// The key from `AXOM_SECRETS_KEY`. An unset or empty variable is
    /// `Missing`; anything else that is not 32 bytes of hex is `BadKey`, so
    /// callers can tell "not configured" from "configured wrong".
    pub fn from_env() -> Result<Self, CryptoError> {
        match std::env::var("AXOM_SECRETS_KEY") {
            Ok(v) if !v.trim().is_empty() => Self::from_hex(&v),
            _ => Err(CryptoError::Missing),
        }
    }

    /// A new random master key, hex-encoded (for `axom secrets-key`).
    pub fn generate_hex() -> String {
        let mut k = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut k);
        hex::encode(k)
    }

    fn aad(tenant: Uuid, name: &str) -> String {
        format!("axom-secret:{tenant}:{name}")
    }

    /// Encrypt bound to a context string. Returns (nonce, ciphertext).
    pub fn seal_for(&self, context: &str, plaintext: &str) -> (Vec<u8>, Vec<u8>) {
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext.as_bytes(), aad: context.as_bytes() })
            .expect("AES-GCM encryption does not fail for in-memory buffers");
        (nonce.to_vec(), ct)
    }

    pub fn open_for(&self, context: &str, nonce: &[u8], ciphertext: &[u8]) -> Result<String, CryptoError> {
        if nonce.len() != 12 {
            return Err(CryptoError::Decrypt);
        }
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: context.as_bytes() })
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(pt).map_err(|_| CryptoError::Decrypt)
    }

    /// A company's integration key: bound to the company and key name.
    pub fn seal(&self, tenant: Uuid, name: &str, plaintext: &str) -> (Vec<u8>, Vec<u8>) {
        self.seal_for(&Self::aad(tenant, name), plaintext)
    }

    pub fn open(&self, tenant: Uuid, name: &str, nonce: &[u8], ciphertext: &[u8]) -> Result<String, CryptoError> {
        self.open_for(&Self::aad(tenant, name), nonce, ciphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_binding() {
        let b = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
        let (t1, t2) = (Uuid::new_v4(), Uuid::new_v4());
        let (n, ct) = b.seal(t1, "SMTP_PASSWORD", "hunter2");
        assert_eq!(b.open(t1, "SMTP_PASSWORD", &n, &ct).unwrap(), "hunter2");
        assert_eq!(b.open(t2, "SMTP_PASSWORD", &n, &ct), Err(CryptoError::Decrypt), "bound to the company");
        assert_eq!(b.open(t1, "POSTIZ_API_KEY", &n, &ct), Err(CryptoError::Decrypt), "bound to the key name");
        let other = SecretBox::from_hex(&SecretBox::generate_hex()).unwrap();
        assert_eq!(other.open(t1, "SMTP_PASSWORD", &n, &ct), Err(CryptoError::Decrypt));
        let mut tampered = ct.clone();
        tampered[0] ^= 1;
        assert_eq!(b.open(t1, "SMTP_PASSWORD", &n, &tampered), Err(CryptoError::Decrypt));
        assert!(SecretBox::from_hex("abcd").is_err());
    }

    #[test]
    fn key_parsing() {
        let k = SecretBox::generate_hex();
        assert!(SecretBox::from_hex(&format!("\"{k}\"")).is_ok(), "double quotes");
        assert!(SecretBox::from_hex(&format!("'{k}'")).is_ok(), "single quotes");
        assert!(SecretBox::from_hex(&format!("  {k}\n")).is_ok(), "whitespace");
        assert_eq!(SecretBox::from_hex(&k[..63]).err(), Some(CryptoError::BadKey), "63 characters");
        assert_eq!(SecretBox::from_hex(&k[..62]).err(), Some(CryptoError::BadKey), "31 bytes");
    }
}
