//! NEAR primitives Combox needs without pulling in the full NEAR SDK:
//! ed25519 public keys, implicit accounts, Borsh encoding of transactions
//! (Transfer and FunctionCall actions), and yoctoNEAR conversion.
//!
//! NEAR signs `sha256(borsh(Transaction))` with ed25519, treating the 32-byte
//! hash as the message. [`Transaction::signing_hash`] returns that hash.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

pub const YOCTO_PER_NEAR: u128 = 1_000_000_000_000_000_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NearError {
    #[error("invalid public key: {0}")]
    BadPublicKey(String),
    #[error("invalid signature: {0}")]
    BadSignature(String),
    #[error("invalid account id: {0}")]
    BadAccount(String),
    #[error("invalid amount: {0}")]
    BadAmount(String),
}

/// An ed25519 public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    /// Parse `ed25519:<base58>`.
    pub fn parse(s: &str) -> Result<Self, NearError> {
        let b58 = s.strip_prefix("ed25519:").ok_or_else(|| NearError::BadPublicKey("missing ed25519: prefix".into()))?;
        let bytes = bs58::decode(b58).into_vec().map_err(|e| NearError::BadPublicKey(e.to_string()))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| NearError::BadPublicKey("expected 32 bytes".into()))?;
        VerifyingKey::from_bytes(&arr).map_err(|e| NearError::BadPublicKey(e.to_string()))?;
        Ok(Self(arr))
    }

    /// `ed25519:<base58>`, the format NEAR RPC uses.
    pub fn to_near_string(&self) -> String {
        format!("ed25519:{}", bs58::encode(self.0).into_string())
    }

    /// The implicit account for this key: lowercase hex of the 32 bytes.
    pub fn implicit_account(&self) -> String {
        hex::encode(self.0)
    }

    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<(), NearError> {
        let vk = VerifyingKey::from_bytes(&self.0).map_err(|e| NearError::BadPublicKey(e.to_string()))?;
        vk.verify(message, &Signature::from_bytes(signature)).map_err(|e| NearError::BadSignature(e.to_string()))
    }
}

/// True for a NEAR implicit account id (64 lowercase hex chars).
pub fn is_implicit_account(account: &str) -> bool {
    account.len() == 64 && account.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The public key an implicit account id encodes.
pub fn implicit_account_key(account: &str) -> Result<PublicKey, NearError> {
    if !is_implicit_account(account) {
        return Err(NearError::BadAccount(format!("{account} is not an implicit account")));
    }
    let bytes = hex::decode(account).map_err(|e| NearError::BadAccount(e.to_string()))?;
    Ok(PublicKey(bytes.try_into().expect("64 hex chars = 32 bytes")))
}

/// Basic NEAR account id validation (length and allowed characters).
pub fn validate_account_id(account: &str) -> Result<(), NearError> {
    let ok_len = (2..=64).contains(&account.len());
    let ok_chars = account.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b));
    let ok_edges = !account.starts_with(['-', '_', '.']) && !account.ends_with(['-', '_', '.']);
    if ok_len && ok_chars && ok_edges {
        Ok(())
    } else {
        Err(NearError::BadAccount(account.to_string()))
    }
}

/// Convert a NEAR amount (up to 24 decimal places) to yoctoNEAR.
pub fn near_to_yocto(near: Decimal) -> Result<u128, NearError> {
    if near <= Decimal::ZERO {
        return Err(NearError::BadAmount(near.to_string()));
    }
    let n = near.normalize();
    let scale = n.scale();
    if scale > 24 {
        return Err(NearError::BadAmount(format!("{near} has more than 24 decimals")));
    }
    let mantissa = u128::try_from(n.mantissa()).map_err(|_| NearError::BadAmount(near.to_string()))?;
    mantissa
        .checked_mul(10u128.pow(24 - scale))
        .ok_or_else(|| NearError::BadAmount(near.to_string()))
}

/// Convert USD to yoctoNEAR at a price, rounded to 6 decimal places of NEAR.
pub fn usd_to_yocto(usd: Decimal, usd_per_near: Decimal) -> Result<u128, NearError> {
    if usd_per_near <= Decimal::ZERO {
        return Err(NearError::BadAmount(format!("price {usd_per_near}")));
    }
    near_to_yocto((usd / usd_per_near).round_dp(6))
}

/// yoctoNEAR as a human-readable NEAR decimal string.
pub fn yocto_to_near_string(yocto: u128) -> String {
    let whole = yocto / YOCTO_PER_NEAR;
    let frac = yocto % YOCTO_PER_NEAR;
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{}", format!("{frac:024}").trim_end_matches('0'))
    }
}

/// Serde for yoctoNEAR as a decimal string (JSON numbers lose precision
/// above 2^53 in most readers, including JSONB round trips).
pub mod yocto_str {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }

    /// The same for `Option<u128>`.
    pub mod opt {
        use serde::{Deserialize, Deserializer, Serializer};
        pub fn serialize<S: Serializer>(v: &Option<u128>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(v) => s.serialize_some(&v.to_string()),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u128>, D::Error> {
            Option::<String>::deserialize(d)?.map(|s| s.parse().map_err(serde::de::Error::custom)).transpose()
        }
    }
}

// ---------------------------------------------------------------------------
// Borsh encoding of a transaction (Transfer and FunctionCall actions)
// ---------------------------------------------------------------------------

fn borsh_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// The NEAR actions Axom signs. Everything else is rejected by the decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Move native NEAR to the receiver.
    Transfer { deposit: u128 },
    /// Call a method on the receiver contract.
    FunctionCall { method_name: String, args: Vec<u8>, gas: u64, deposit: u128 },
}

/// A NEAR transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub signer_id: String,
    pub public_key: PublicKey,
    pub nonce: u64,
    pub receiver_id: String,
    pub block_hash: [u8; 32],
    pub actions: Vec<Action>,
}

/// Borsh enum tags.
const KEY_TYPE_ED25519: u8 = 0;
const ACTION_FUNCTION_CALL: u8 = 2;
const ACTION_TRANSFER: u8 = 3;

impl Transaction {
    /// A transaction with one Transfer action.
    pub fn transfer(signer_id: String, public_key: PublicKey, nonce: u64, receiver_id: String, block_hash: [u8; 32], deposit_yocto: u128) -> Self {
        Self { signer_id, public_key, nonce, receiver_id, block_hash, actions: vec![Action::Transfer { deposit: deposit_yocto }] }
    }

    /// The deposit of a single-Transfer transaction, if that is what this is.
    pub fn transfer_deposit(&self) -> Option<u128> {
        match self.actions.as_slice() {
            [Action::Transfer { deposit }] => Some(*deposit),
            _ => None,
        }
    }

    /// Sum of every action's attached deposit (what leaves the signer, before gas).
    pub fn total_deposit(&self) -> u128 {
        self.actions
            .iter()
            .map(|a| match a {
                Action::Transfer { deposit } | Action::FunctionCall { deposit, .. } => *deposit,
            })
            .sum()
    }

    /// Borsh serialization (Transaction V0).
    pub fn to_borsh(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(160);
        borsh_string(&mut out, &self.signer_id);
        out.push(KEY_TYPE_ED25519);
        out.extend_from_slice(&self.public_key.0);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        borsh_string(&mut out, &self.receiver_id);
        out.extend_from_slice(&self.block_hash);
        out.extend_from_slice(&(self.actions.len() as u32).to_le_bytes());
        for a in &self.actions {
            match a {
                Action::Transfer { deposit } => {
                    out.push(ACTION_TRANSFER);
                    out.extend_from_slice(&deposit.to_le_bytes());
                }
                Action::FunctionCall { method_name, args, gas, deposit } => {
                    out.push(ACTION_FUNCTION_CALL);
                    borsh_string(&mut out, method_name);
                    out.extend_from_slice(&(args.len() as u32).to_le_bytes());
                    out.extend_from_slice(args);
                    out.extend_from_slice(&gas.to_le_bytes());
                    out.extend_from_slice(&deposit.to_le_bytes());
                }
            }
        }
        out
    }

    /// `sha256(borsh(tx))`: the message NEAR expects the ed25519 signature over.
    /// Its base58 encoding is the transaction hash.
    pub fn signing_hash(&self) -> [u8; 32] {
        Sha256::digest(self.to_borsh()).into()
    }

    /// Borsh-encoded SignedTransaction.
    pub fn signed_borsh(&self, signature: &[u8; 64]) -> Vec<u8> {
        let mut out = self.to_borsh();
        out.push(KEY_TYPE_ED25519);
        out.extend_from_slice(signature);
        out
    }
}

/// Decode a SignedTransaction produced by [`Transaction::signed_borsh`].
/// Used by tests and by the signer's pre-broadcast self-check.
pub fn decode_signed_transaction(bytes: &[u8]) -> Result<(Transaction, [u8; 64]), NearError> {
    struct R<'a>(&'a [u8]);
    impl<'a> R<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], NearError> {
            if self.0.len() < n {
                return Err(NearError::BadSignature("truncated transaction".into()));
            }
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            Ok(a)
        }
        fn u8(&mut self) -> Result<u8, NearError> {
            Ok(self.take(1)?[0])
        }
        fn u32(&mut self) -> Result<u32, NearError> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
        fn u64(&mut self) -> Result<u64, NearError> {
            Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
        }
        fn u128(&mut self) -> Result<u128, NearError> {
            Ok(u128::from_le_bytes(self.take(16)?.try_into().unwrap()))
        }
        fn bytes(&mut self) -> Result<Vec<u8>, NearError> {
            let n = self.u32()? as usize;
            Ok(self.take(n)?.to_vec())
        }
        fn string(&mut self) -> Result<String, NearError> {
            String::from_utf8(self.bytes()?).map_err(|e| NearError::BadAccount(e.to_string()))
        }
    }
    let mut r = R(bytes);
    let signer_id = r.string()?;
    if r.u8()? != KEY_TYPE_ED25519 {
        return Err(NearError::BadPublicKey("not ed25519".into()));
    }
    let public_key = PublicKey(r.take(32)?.try_into().unwrap());
    let nonce = r.u64()?;
    let receiver_id = r.string()?;
    let block_hash: [u8; 32] = r.take(32)?.try_into().unwrap();
    let n = r.u32()?;
    if n == 0 || n > 16 {
        return Err(NearError::BadAmount(format!("unexpected action count {n}")));
    }
    let mut actions = Vec::with_capacity(n as usize);
    for _ in 0..n {
        actions.push(match r.u8()? {
            ACTION_TRANSFER => Action::Transfer { deposit: r.u128()? },
            ACTION_FUNCTION_CALL => {
                let method_name = r.string()?;
                let args = r.bytes()?;
                let gas = r.u64()?;
                let deposit = r.u128()?;
                Action::FunctionCall { method_name, args, gas, deposit }
            }
            t => return Err(NearError::BadAmount(format!("unsupported action tag {t}"))),
        });
    }
    if r.u8()? != KEY_TYPE_ED25519 {
        return Err(NearError::BadSignature("not ed25519".into()));
    }
    let sig: [u8; 64] = r.take(64)?.try_into().unwrap();
    if !r.0.is_empty() {
        return Err(NearError::BadSignature("trailing bytes".into()));
    }
    Ok((Transaction { signer_id, public_key, nonce, receiver_id, block_hash, actions }, sig))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rust_decimal_macros::dec;

    #[test]
    fn implicit_account_round_trip() {
        let sk = SigningKey::generate(&mut rand::rngs::OsRng);
        let pk = PublicKey(sk.verifying_key().to_bytes());
        let account = pk.implicit_account();
        assert_eq!(account.len(), 64);
        assert_eq!(account, account.to_lowercase());
        assert!(is_implicit_account(&account));
        assert_eq!(implicit_account_key(&account).unwrap(), pk);
        assert_eq!(PublicKey::parse(&pk.to_near_string()).unwrap(), pk);
        assert!(!is_implicit_account("alice.testnet"));
        assert!(!is_implicit_account(&account.to_uppercase()));
    }

    #[test]
    fn known_implicit_account_vector() {
        // All-zero-but-one key: hex is fixed and easy to check by eye.
        let mut bytes = [0u8; 32];
        bytes[31] = 0xab;
        assert_eq!(PublicKey(bytes).implicit_account(), format!("{}ab", "0".repeat(62)));
    }

    #[test]
    fn yocto_conversion() {
        assert_eq!(near_to_yocto(dec!(1)).unwrap(), YOCTO_PER_NEAR);
        assert_eq!(near_to_yocto(dec!(0.5)).unwrap(), YOCTO_PER_NEAR / 2);
        assert_eq!(usd_to_yocto(dec!(10), dec!(4)).unwrap(), 2_500_000_000_000_000_000_000_000);
        assert_eq!(usd_to_yocto(dec!(1), dec!(3)).unwrap(), 333_333_000_000_000_000_000_000);
        assert!(near_to_yocto(dec!(0)).is_err());
        assert!(usd_to_yocto(dec!(1), dec!(0)).is_err());
        assert_eq!(yocto_to_near_string(2_500_000_000_000_000_000_000_000), "2.5");
        assert_eq!(yocto_to_near_string(YOCTO_PER_NEAR * 3), "3");
    }

    #[test]
    fn transfer_borsh_layout_and_signature() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = PublicKey(sk.verifying_key().to_bytes());
        let tx = Transaction::transfer(pk.implicit_account(), pk, 42, "bob.testnet".into(), [9u8; 32], YOCTO_PER_NEAR);
        let b = tx.to_borsh();
        // 4+64 signer, 1+32 key, 8 nonce, 4+11 receiver, 32 hash, 4 action count, 1 tag, 16 deposit.
        assert_eq!(b.len(), 68 + 33 + 8 + 15 + 32 + 4 + 1 + 16);
        assert_eq!(&b[0..4], &64u32.to_le_bytes());
        let sig = sk.sign(&tx.signing_hash()).to_bytes();
        pk.verify(&tx.signing_hash(), &sig).unwrap();
        let (decoded, dsig) = decode_signed_transaction(&tx.signed_borsh(&sig)).unwrap();
        assert_eq!(decoded, tx);
        assert_eq!(dsig, sig);
        // Signing the raw bytes instead of the hash is not what NEAR verifies.
        let wrong = sk.sign(&b).to_bytes();
        assert!(pk.verify(&tx.signing_hash(), &wrong).is_err());
    }

    /// Golden vector produced independently with the `near-api-py` Borsh
    /// serializer (same inputs), so our hand-written encoder is checked
    /// against a separate implementation of NEAR's transaction schema.
    #[test]
    fn borsh_matches_independent_implementation() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let tx = Transaction::transfer(hex::encode(key), PublicKey(key), 42, "bob.testnet".into(), [9u8; 32], YOCTO_PER_NEAR + 123);
        let expected = "400000003030303130323033303430353036303730383039306130623063306430653066313031313132313331343135313631373138313931613162316331643165316600000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2a000000000000000b000000626f622e746573746e6574090909090909090909090909090909090909090909090909090909090909090901000000037b0000a1edccce1bc2d3000000000000";
        assert_eq!(hex::encode(tx.to_borsh()), expected);
        assert_eq!(hex::encode(tx.signing_hash()), "716c3fed4168259629ef8aee2dbc9bcedf7158d97f56bd925d202e0d34bf69ce");
    }

    #[test]
    fn account_id_validation() {
        assert!(validate_account_id("alice.testnet").is_ok());
        assert!(validate_account_id("a").is_err());
        assert!(validate_account_id("Alice.testnet").is_err());
        assert!(validate_account_id(".alice").is_err());
    }
}
