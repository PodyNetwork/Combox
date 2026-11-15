//! NEAR JSON-RPC client (the subset Combox uses) and an in-memory mock that
//! validates signed transactions the way a NEAR node would for our encoding.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use axom_core::near::{decode_signed_transaction, PublicKey, Transaction};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::price::{map_http_status, map_reqwest};
use crate::{ToolError, ToolResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessKeyInfo {
    pub nonce: u64,
    pub full_access: bool,
    /// Block hash from the same query, usable as the transaction's recent block.
    pub block_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    Success,
    Failure(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxOutcome {
    /// Base58 transaction hash.
    pub hash: String,
    pub status: TxStatus,
    /// Whether NEAR reports the transaction as final.
    pub is_final: bool,
}

/// An account's balance as NEAR reports it (`view_account`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBalance {
    /// Liquid balance in yoctoNEAR (includes what storage reserves).
    pub amount_yocto: u128,
    /// Staked (locked) balance in yoctoNEAR.
    pub locked_yocto: u128,
    /// Bytes of storage the account uses; each byte reserves 10^19 yocto.
    pub storage_usage: u64,
}

/// Yocto reserved per byte of account storage (NEAR protocol constant).
pub const STORAGE_YOCTO_PER_BYTE: u128 = 10_000_000_000_000_000_000;

impl AccountBalance {
    /// What can be spent: the balance minus what its storage reserves.
    pub fn available_yocto(&self) -> u128 {
        self.amount_yocto.saturating_sub(self.storage_usage as u128 * STORAGE_YOCTO_PER_BYTE)
    }
}

/// Parse NEAR's `view_account` result.
pub fn parse_account(r: &Value) -> ToolResult<Option<AccountBalance>> {
    // Older nodes report a missing account as a result with an `error` string.
    if let Some(e) = r.get("error").and_then(Value::as_str) {
        return if e.contains("does not exist") { Ok(None) } else { Err(ToolError::Permanent(e.to_string())) };
    }
    let yocto = |k: &str| r.get(k).and_then(Value::as_str).and_then(|v| v.parse::<u128>().ok()).ok_or_else(|| ToolError::Permanent(format!("view_account: bad {k}")));
    Ok(Some(AccountBalance { amount_yocto: yocto("amount")?, locked_yocto: yocto("locked")?, storage_usage: r.get("storage_usage").and_then(Value::as_u64).unwrap_or(0) }))
}

/// How long `tx_status` should wait. NEAR's `tx` method blocks until the
/// requested stage, and for a transaction it has never seen it waits until
/// its own timeout. Use `Probe` to ask "do you know this transaction?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxWait {
    /// `wait_until: NONE`: answer immediately (UNKNOWN_TRANSACTION -> None).
    Probe,
    /// `wait_until: FINAL`.
    Final,
}

#[async_trait]
pub trait NearRpc: Send + Sync {
    /// The access key, or `None` if the account has no such key.
    async fn view_access_key(&self, account_id: &str, key: &PublicKey) -> ToolResult<Option<AccessKeyInfo>>;
    /// Submit a signed transaction and wait until it is executed.
    async fn send_tx(&self, signed_tx: &[u8]) -> ToolResult<TxOutcome>;
    /// Status of a transaction, or `None` if the node does not know it.
    async fn tx_status(&self, tx_hash: &str, sender_id: &str, wait: TxWait) -> ToolResult<Option<TxOutcome>>;
}

pub fn testnet_rpc_url() -> String {
    std::env::var("NEAR_RPC_URL").unwrap_or_else(|_| "https://rpc.testnet.fastnear.com".into())
}

pub struct JsonRpcNear {
    client: reqwest::Client,
    url: String,
}

impl JsonRpcNear {
    pub fn new(url: impl Into<String>) -> Self {
        Self::with_timeout(url, std::time::Duration::from_secs(60))
    }

    /// A client for quick reads (a dashboard shouldn't wait a minute).
    pub fn with_timeout(url: impl Into<String>, timeout: std::time::Duration) -> Self {
        let client = reqwest::Client::builder().timeout(timeout).build().expect("http client");
        Self { client, url: url.into() }
    }

    /// An account's balance, or `None` if the account doesn't exist on chain
    /// yet (an implicit account appears with its first deposit). Read-only.
    pub async fn view_account(&self, account_id: &str) -> ToolResult<Option<AccountBalance>> {
        match self.call("query", json!({ "request_type": "view_account", "finality": "final", "account_id": account_id })).await? {
            Ok(r) => parse_account(&r),
            Err(e) if cause(&e) == "UNKNOWN_ACCOUNT" => Ok(None),
            Err(e) => Err(map_rpc_error(&e)),
        }
    }

    /// Call a contract's view method (finality final) and parse its JSON result.
    pub async fn view_function(&self, contract: &str, method: &str, args: &Value) -> ToolResult<Value> {
        let args_base64 = base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(args).expect("json args"));
        let params = json!({ "request_type": "call_function", "finality": "final", "account_id": contract, "method_name": method, "args_base64": args_base64 });
        let r = match self.call("query", params).await? {
            Ok(r) => r,
            Err(e) => return Err(map_rpc_error(&e)),
        };
        // Contract panics come back as a result with an `error` string.
        if let Some(e) = r.get("error").and_then(Value::as_str) {
            return Err(ToolError::Permanent(format!("{contract}.{method}: {e}")));
        }
        let bytes: Vec<u8> = r
            .get("result")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::Permanent(format!("{contract}.{method}: no result")))?
            .iter()
            .map(|b| b.as_u64().unwrap_or(0) as u8)
            .collect();
        serde_json::from_slice(&bytes).map_err(|e| ToolError::Permanent(format!("{contract}.{method}: {e}")))
    }

    async fn call(&self, method: &str, params: Value) -> ToolResult<Result<Value, Value>> {
        let body = json!({ "jsonrpc": "2.0", "id": "axom", "method": method, "params": params });
        let resp = self.client.post(&self.url).json(&body).send().await.map_err(|e| map_reqwest(e, "near rpc"))?;
        let status = resp.status();
        // A rate limit can come with a JSON error body that has no cause;
        // say what it is rather than reading that body as a permanent error.
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(map_http_status(status, "near rpc"));
        }
        // A gateway in front of the node answering 502/503/504 says nothing
        // about the request itself: try again. Its body is usually HTML.
        if status.is_server_error() {
            return Err(ToolError::Retryable(format!("near rpc: HTTP {}", status.as_u16())));
        }
        let text = resp.text().await.map_err(|e| map_reqwest(e, "near rpc"))?;
        // Not JSON-RPC at all (a proxy's error page, a truncated body): the
        // node never judged the request, so this is not a final answer.
        let v: Value = serde_json::from_str(&text).map_err(|_| ToolError::Retryable("near rpc: unreadable response".into()))?;
        if let Some(err) = v.get("error") {
            return Ok(Err(err.clone()));
        }
        if !status.is_success() {
            return Err(map_http_status(status, "near rpc"));
        }
        Ok(Ok(v.get("result").cloned().unwrap_or(Value::Null)))
    }
}

/// Name of the error cause in a NEAR RPC error object.
fn cause(err: &Value) -> String {
    err.pointer("/cause/name").and_then(Value::as_str).unwrap_or("").to_string()
}

fn map_rpc_error(err: &Value) -> ToolError {
    let name = cause(err);
    // Errors without a cause (from proxies in front of the node) only have a message.
    let detail = err.get("data").or_else(|| err.get("message")).map(|d| d.to_string()).unwrap_or_else(|| err.to_string());
    let summary = format!("{name} {detail}").trim().to_string();
    match name.as_str() {
        "TIMEOUT_ERROR" | "INTERNAL_ERROR" | "NO_SYNCED_BLOCKS" | "UNAVAILABLE_SHARD" => ToolError::Retryable(summary),
        "TOO_MANY_REQUESTS" => ToolError::RateLimited { retry_after_secs: 30 },
        // No cause at all: the error came from something in front of the node
        // (a proxy, a quota or auth layer), not from the node judging the
        // request. Treating it as final would fail a payout for good over an
        // outage, or over a transaction that in fact went through.
        "" => ToolError::Retryable(summary),
        // InvalidTransaction, INVALID_TRANSACTION, PARSE_ERROR, etc.
        _ => ToolError::Permanent(summary),
    }
}

fn parse_outcome(r: &Value) -> ToolResult<TxOutcome> {
    let hash = r.pointer("/transaction/hash").and_then(Value::as_str).unwrap_or_default().to_string();
    let status = match r.get("status") {
        Some(s) if s.get("SuccessValue").is_some() => TxStatus::Success,
        Some(s) if s.get("Failure").is_some() => TxStatus::Failure(s["Failure"].to_string()),
        _ => return Err(ToolError::Retryable("transaction outcome not available yet".into())),
    };
    let fin = r.get("final_execution_status").and_then(Value::as_str).unwrap_or("");
    Ok(TxOutcome { hash, status, is_final: fin == "FINAL" })
}

#[async_trait]
impl NearRpc for JsonRpcNear {
    async fn view_access_key(&self, account_id: &str, key: &PublicKey) -> ToolResult<Option<AccessKeyInfo>> {
        let r = self
            .call(
                "query",
                json!({ "request_type": "view_access_key", "finality": "final", "account_id": account_id, "public_key": key.to_near_string() }),
            )
            .await?;
        let r = match r {
            Ok(r) => r,
            Err(e) if matches!(cause(&e).as_str(), "UNKNOWN_ACCESS_KEY" | "UNKNOWN_ACCOUNT") => return Ok(None),
            Err(e) => return Err(map_rpc_error(&e)),
        };
        // Older nodes report a missing key as a result with an `error` string.
        if r.get("error").is_some() {
            return Ok(None);
        }
        let nonce = r.get("nonce").and_then(Value::as_u64).ok_or_else(|| ToolError::Permanent("access key has no nonce".into()))?;
        let full_access = r.get("permission").and_then(Value::as_str) == Some("FullAccess");
        let bh = r.get("block_hash").and_then(Value::as_str).ok_or_else(|| ToolError::Permanent("no block_hash".into()))?;
        let bytes = bs58::decode(bh).into_vec().map_err(|e| ToolError::Permanent(e.to_string()))?;
        let block_hash: [u8; 32] = bytes.try_into().map_err(|_| ToolError::Permanent("block hash length".into()))?;
        Ok(Some(AccessKeyInfo { nonce, full_access, block_hash }))
    }

    async fn send_tx(&self, signed_tx: &[u8]) -> ToolResult<TxOutcome> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(signed_tx);
        match self.call("send_tx", json!({ "signed_tx_base64": b64, "wait_until": "EXECUTED_OPTIMISTIC" })).await? {
            Ok(r) => parse_outcome(&r),
            Err(e) => Err(map_rpc_error(&e)),
        }
    }

    async fn tx_status(&self, tx_hash: &str, sender_id: &str, wait: TxWait) -> ToolResult<Option<TxOutcome>> {
        let wait = match wait {
            TxWait::Probe => "NONE",
            TxWait::Final => "FINAL",
        };
        match self.call("tx", json!({ "tx_hash": tx_hash, "sender_account_id": sender_id, "wait_until": wait })).await? {
            Ok(r) => parse_outcome(&r).map(Some),
            Err(e) if cause(&e) == "UNKNOWN_TRANSACTION" => Ok(None),
            Err(e) => Err(map_rpc_error(&e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Mock
// ---------------------------------------------------------------------------

/// In-memory NEAR. Checks what a node checks for our transfers: the access
/// key exists, the nonce increases, the block hash is known, and the ed25519
/// signature over sha256(borsh(tx)) verifies.
pub struct MockNear {
    state: Mutex<MockState>,
}

struct MockState {
    keys: HashMap<(String, PublicKey), u64>,
    block_hash: [u8; 32],
    txs: HashMap<String, (Transaction, TxOutcome)>,
    /// Errors to return from the next `send_tx` calls, in order.
    send_failures: Vec<ToolError>,
    /// Report access-key nonces as of before the latest transaction, the way
    /// a node answering at `final` finality lags the chain head.
    lagging_views: bool,
    final_nonces: HashMap<(String, PublicKey), u64>,
    /// Accept a transaction but answer as if the send timed out (once).
    lose_next_answer: bool,
}

impl Default for MockNear {
    fn default() -> Self {
        Self { state: Mutex::new(MockState { keys: HashMap::new(), block_hash: [42u8; 32], txs: HashMap::new(), send_failures: vec![], lagging_views: false, final_nonces: HashMap::new(), lose_next_answer: false }) }
    }
}

impl MockNear {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_access_key(&self, account: &str, key: PublicKey, nonce: u64) {
        self.state.lock().unwrap().keys.insert((account.into(), key), nonce);
    }

    pub fn fail_next_sends(&self, errs: Vec<ToolError>) {
        self.state.lock().unwrap().send_failures = errs;
    }

    /// Answer `view_access_key` with the nonce from before the latest
    /// transaction (a node's `final` view is a couple of blocks behind).
    pub fn lag_views(&self, on: bool) {
        self.state.lock().unwrap().lagging_views = on;
    }

    /// The next transaction is accepted and lands, but the caller is told
    /// the send timed out.
    pub fn lose_next_answer(&self) {
        self.state.lock().unwrap().lose_next_answer = true;
    }

    /// Time passes: transactions naming the old block hash have expired.
    pub fn expire_pending(&self) {
        let mut s = self.state.lock().unwrap();
        s.block_hash = [s.block_hash[0].wrapping_add(1); 32];
    }

    pub fn sent(&self) -> Vec<Transaction> {
        self.state.lock().unwrap().txs.values().map(|(t, _)| t.clone()).collect()
    }
}

#[async_trait]
impl NearRpc for MockNear {
    async fn view_access_key(&self, account_id: &str, key: &PublicKey) -> ToolResult<Option<AccessKeyInfo>> {
        let s = self.state.lock().unwrap();
        let k = (account_id.to_string(), *key);
        let lagged = if s.lagging_views { s.final_nonces.get(&k).copied() } else { None };
        Ok(s.keys.get(&k).map(|n| AccessKeyInfo { nonce: lagged.unwrap_or(*n), full_access: true, block_hash: s.block_hash }))
    }

    async fn send_tx(&self, signed_tx: &[u8]) -> ToolResult<TxOutcome> {
        let mut s = self.state.lock().unwrap();
        if !s.send_failures.is_empty() {
            return Err(s.send_failures.remove(0));
        }
        let (tx, sig) = decode_signed_transaction(signed_tx).map_err(|e| ToolError::Permanent(format!("INVALID_TRANSACTION {e}")))?;
        let hash = tx.signing_hash();
        tx.public_key.verify(&hash, &sig).map_err(|_| ToolError::Permanent("InvalidSignature".into()))?;
        if tx.block_hash != s.block_hash {
            return Err(ToolError::Permanent("Expired (unknown block hash)".into()));
        }
        let k = (tx.signer_id.clone(), tx.public_key);
        let current = *s.keys.get(&k).ok_or_else(|| ToolError::Permanent("InvalidAccessKeyError".into()))?;
        if tx.nonce <= current {
            return Err(ToolError::Permanent(format!("InvalidNonce {} <= {current}", tx.nonce)));
        }
        s.final_nonces.insert(k.clone(), current);
        s.keys.insert(k, tx.nonce);
        let hash_b58 = bs58::encode(hash).into_string();
        let outcome = TxOutcome { hash: hash_b58.clone(), status: TxStatus::Success, is_final: false };
        s.txs.insert(hash_b58, (tx, outcome.clone()));
        if std::mem::take(&mut s.lose_next_answer) {
            return Err(ToolError::Retryable("timeout waiting for execution".into()));
        }
        Ok(outcome)
    }

    async fn tx_status(&self, tx_hash: &str, _sender_id: &str, wait: TxWait) -> ToolResult<Option<TxOutcome>> {
        let s = self.state.lock().unwrap();
        Ok(s.txs.get(tx_hash).map(|(_, o)| TxOutcome { is_final: wait == TxWait::Final, ..o.clone() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn parses_view_account() {
        let r = json!({ "amount": "2500000000000000000000000", "locked": "0", "storage_usage": 182, "code_hash": "11111111111111111111111111111111", "block_height": 1 });
        let b = parse_account(&r).unwrap().unwrap();
        assert_eq!(b.amount_yocto, 2_500_000_000_000_000_000_000_000);
        assert_eq!(b.available_yocto(), 2_500_000_000_000_000_000_000_000 - 182 * STORAGE_YOCTO_PER_BYTE);
        assert_eq!(parse_account(&json!({ "error": "account x.testnet does not exist while viewing" })).unwrap(), None);
        assert!(parse_account(&json!({ "amount": "lots" })).is_err());
    }

    /// A node (or the gateway in front of it) that answers every request
    /// with a fixed status and body.
    async fn node(status: u16, body: &'static str) -> JsonRpcNear {
        let app = axum::Router::new().fallback(move || async move { (axum::http::StatusCode::from_u16(status).unwrap(), body) });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        JsonRpcNear::new(url)
    }

    /// Only the node judging the transaction is a final answer. A gateway
    /// error, an error page, or an error with no cause must be retried: the
    /// signer fails a payout for good on a permanent error.
    #[tokio::test]
    async fn gateway_trouble_is_retryable_and_node_verdicts_are_final() {
        let retryable = [
            (502, "<html><body>Bad Gateway</body></html>"),
            (503, r#"{"error":"upstream unavailable"}"#),
            (504, ""),
            (200, "<html>maintenance</html>"),
            (200, r#"{"jsonrpc":"2.0","id":"axom","error":{"code":-32000,"message":"quota exceeded"}}"#),
            (200, r#"{"jsonrpc":"2.0","id":"axom","error":{"cause":{"name":"TIMEOUT_ERROR"},"message":"Timeout"}}"#),
        ];
        for (status, body) in retryable {
            let err = node(status, body).await.send_tx(b"signed-tx").await.unwrap_err();
            assert!(matches!(err, ToolError::Retryable(_)), "HTTP {status} {body:?} gave {err:?}");
        }
        let err = node(200, r#"{"jsonrpc":"2.0","id":"axom","error":{"cause":{"name":"INVALID_TRANSACTION"},"data":{"TxExecutionError":{"InvalidTxError":"Expired"}}}}"#)
            .await
            .send_tx(b"signed-tx")
            .await
            .unwrap_err();
        assert!(matches!(&err, ToolError::Permanent(m) if m.contains("Expired")), "{err:?}");
        let err = node(429, "slow down").await.send_tx(b"signed-tx").await.unwrap_err();
        assert!(matches!(err, ToolError::RateLimited { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn mock_validates_like_a_node() {
        let sk = SigningKey::generate(&mut rand::rngs::OsRng);
        let pk = PublicKey(sk.verifying_key().to_bytes());
        let near = MockNear::new();
        let signer_id = pk.implicit_account();
        near.add_access_key(&signer_id, pk, 5);
        let info = near.view_access_key(&signer_id, &pk).await.unwrap().unwrap();
        let tx = Transaction::transfer(signer_id.clone(), pk, info.nonce + 1, "bob.testnet".into(), info.block_hash, 1);
        let good = sk.sign(&tx.signing_hash()).to_bytes();
        let bad = sk.sign(&tx.to_borsh()).to_bytes();
        assert!(near.send_tx(&tx.signed_borsh(&bad)).await.is_err(), "signature over raw bytes is rejected");
        let out = near.send_tx(&tx.signed_borsh(&good)).await.unwrap();
        assert_eq!(out.status, TxStatus::Success);
        assert!(near.send_tx(&tx.signed_borsh(&good)).await.is_err(), "nonce replay rejected");
        assert!(near.tx_status(&out.hash, &signer_id, TxWait::Final).await.unwrap().unwrap().is_final);
    }
}
