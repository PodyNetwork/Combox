//! Integration tests against a real OpenBao server in dev mode (judge mode
//! and tests only; never on the VPS). Run with:
//!
//!   AXOM_TEST_BAO_BIN=/path/to/bao cargo test -p axom-signer --test openbao_real -- --ignored
//!
//! They check the three things milestone 4 requires before any real payout:
//! transit signatures are valid NEAR signatures, the public key maps to the
//! expected implicit account, and a sealed OpenBao makes payouts fail safely.
//! They also check the signer policy and key protections.

mod common;

use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use axom_core::near::{decode_signed_transaction, PublicKey, Transaction};
use axom_core::state::PayoutStatus;
use axom_hr::payouts;
use axom_signer::openbao::{OpenBaoTransit, TransitError, TransitSigner};
use axom_signer::{SendResult, Signer};
use axom_tools::near_rpc::NearRpc;
use common::*;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use sqlx::PgPool;

struct Bao {
    child: Child,
    addr: String,
    unseal_key: String,
    dir: std::path::PathBuf,
}

impl Drop for Bao {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const ROOT: &str = "root-token-for-tests";

async fn root(bao: &Bao, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let c = reqwest::Client::new();
    let mut r = c.request(method, format!("{}{path}", bao.addr)).header("X-Vault-Token", ROOT);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let resp = r.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// Start `bao server -dev`, then do the owner's setup: audit log, transit,
/// non-exportable undeletable ed25519 key, signer policy, signer token.
async fn start() -> (Bao, String) {
    let bin = std::env::var("AXOM_TEST_BAO_BIN").expect("set AXOM_TEST_BAO_BIN to the bao binary");
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("axom-bao-{port}"));
    std::fs::create_dir_all(&dir).unwrap();
    let log = std::fs::File::create(dir.join("server.log")).unwrap();
    // OpenBao 2.x only accepts audit devices declared in server config.
    let audit = dir.join("audit.log");
    let config = dir.join("audit.hcl");
    std::fs::write(&config, format!("audit \"file\" \"file\" {{\n  options {{\n    file_path = \"{}\"\n  }}\n}}\n", audit.display())).unwrap();
    let child = Command::new(bin)
        .args(["server", "-dev", &format!("-dev-root-token-id={ROOT}"), &format!("-dev-listen-address=127.0.0.1:{port}"), &format!("-config={}", config.display())])
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let addr = format!("http://127.0.0.1:{port}");
    let mut bao = Bao { child, addr: addr.clone(), unseal_key: String::new(), dir: dir.clone() };
    for _ in 0..100 {
        if reqwest::get(format!("{addr}/v1/sys/health")).await.map(|r| r.status().is_success()).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(100)).await;
    }
    // The banner with the dev unseal key is printed just after the listener is up.
    for _ in 0..100 {
        let out = std::fs::read_to_string(dir.join("server.log")).unwrap();
        if let Some(k) = out.lines().find_map(|l| l.trim().strip_prefix("Unseal Key: ")) {
            bao.unseal_key = k.trim().to_string();
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(100)).await;
    }
    assert!(!bao.unseal_key.is_empty(), "dev unseal key not printed");

    use reqwest::Method as M;
    let (_, audits) = root(&bao, M::GET, "/v1/sys/audit", None).await;
    assert!(audits.get("file/").is_some(), "audit device from config is active");
    assert!(root(&bao, M::POST, "/v1/sys/mounts/transit", Some(json!({ "type": "transit" }))).await.0 < 300);
    assert!(root(&bao, M::POST, "/v1/transit/keys/axom-treasury", Some(json!({ "type": "ed25519", "exportable": false, "allow_plaintext_backup": false }))).await.0 < 300);
    assert!(root(&bao, M::POST, "/v1/transit/keys/axom-treasury/config", Some(json!({ "deletion_allowed": false }))).await.0 < 300);
    let hcl = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../infra/openbao/axom-signer.hcl")).unwrap();
    assert!(root(&bao, M::PUT, "/v1/sys/policies/acl/axom-signer", Some(json!({ "policy": hcl }))).await.0 < 300);
    let (_, tok) = root(&bao, M::POST, "/v1/auth/token/create", Some(json!({ "policies": ["axom-signer"], "no_default_policy": true }))).await;
    let token = tok["auth"]["client_token"].as_str().unwrap().to_string();
    (bao, token)
}

fn transit(bao: &Bao, token: &str) -> OpenBaoTransit {
    OpenBaoTransit::new(&bao.addr, token, "axom-treasury")
}

#[tokio::test]
#[ignore = "needs a bao binary (AXOM_TEST_BAO_BIN)"]
async fn transit_signature_is_a_valid_near_signature_and_key_maps_to_implicit_account() {
    let (bao, token) = start().await;
    let t = transit(&bao, &token);
    assert!(!t.is_sealed().await.unwrap());
    let pk = t.public_key().await.unwrap();

    // Implicit account = lowercase hex of the 32-byte key, as OpenBao reports it.
    let (_, key) = root(&bao, reqwest::Method::GET, "/v1/transit/keys/axom-treasury", None).await;
    let b64 = key["data"]["keys"]["1"]["public_key"].as_str().unwrap();
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    assert_eq!(pk.implicit_account(), hex::encode(&raw));
    assert_eq!(pk.implicit_account().len(), 64);
    assert_eq!(PublicKey::parse(&pk.to_near_string()).unwrap(), pk);
    assert_eq!(key["data"]["exportable"], false);
    assert_eq!(key["data"]["deletion_allowed"], false);

    // NEAR signs sha256(borsh(tx)); OpenBao signs those 32 bytes with ed25519.
    let tx = Transaction::transfer(pk.implicit_account(), pk, 7, "bob.testnet".into(), [3u8; 32], 10u128.pow(24));
    let sig = t.sign(&tx.signing_hash()).await.unwrap();
    pk.verify(&tx.signing_hash(), &sig).expect("transit signature verifies as NEAR does");
    let (decoded, dsig) = decode_signed_transaction(&tx.signed_borsh(&sig)).unwrap();
    assert_eq!((decoded, dsig), (tx.clone(), sig));

    // And a node-like validator accepts it.
    let near = axom_tools::near_rpc::MockNear::new();
    near.add_access_key(&pk.implicit_account(), pk, 6);
    let mut tx2 = tx.clone();
    tx2.block_hash = [42u8; 32];
    let sig2 = t.sign(&tx2.signing_hash()).await.unwrap();
    near.send_tx(&tx2.signed_borsh(&sig2)).await.expect("accepted");

    // Every sign request is in the OpenBao audit log.
    let audit = std::fs::read_to_string(bao.dir.join("audit.log")).unwrap();
    assert!(audit.matches("transit/sign/axom-treasury").count() >= 2);
}

#[tokio::test]
#[ignore = "needs a bao binary (AXOM_TEST_BAO_BIN)"]
async fn signer_token_can_only_sign_and_read_and_key_cannot_be_deleted() {
    let (bao, token) = start().await;
    let c = reqwest::Client::new();
    let as_signer = |m: reqwest::Method, path: &str| c.request(m, format!("{}{path}", bao.addr)).header("X-Vault-Token", &token);
    use reqwest::Method as M;
    for (m, path, body) in [
        (M::POST, "/v1/transit/keys/another-key", json!({ "type": "ed25519" })),
        (M::POST, "/v1/transit/keys/axom-treasury/rotate", json!({})),
        (M::POST, "/v1/transit/keys/axom-treasury/config", json!({ "exportable": true })),
        (M::DELETE, "/v1/transit/keys/axom-treasury", json!({})),
        (M::GET, "/v1/transit/export/signing-key/axom-treasury", json!({})),
        (M::GET, "/v1/sys/policies/acl/axom-signer", json!({})),
    ] {
        let status = as_signer(m.clone(), path).json(&body).send().await.unwrap().status().as_u16();
        assert_eq!(status, 403, "{m} {path} should be forbidden for the signer");
    }
    // Even the root token cannot delete the key or export it.
    assert!(root(&bao, M::DELETE, "/v1/transit/keys/axom-treasury", None).await.0 >= 400);
    assert!(root(&bao, M::GET, "/v1/transit/export/signing-key/axom-treasury", None).await.0 >= 400);
    assert!(transit(&bao, &token).public_key().await.is_ok(), "key still there");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
#[ignore = "needs a bao binary (AXOM_TEST_BAO_BIN)"]
async fn sealed_openbao_makes_payouts_fail_safely(pool: PgPool) {
    let (bao, token) = start().await;
    let t = transit(&bao, &token);
    let pk = t.public_key().await.unwrap();
    let f = Fixture::new(&pool, pk).await;
    let (w, _) = f.ready_worker(&pool, dec!(10), 1).await;
    let id = f.payroll(&pool, w).await;

    // Seal (what a VPS reboot does).
    assert!(root(&bao, reqwest::Method::PUT, "/v1/sys/seal", None).await.0 < 300);
    assert!(t.is_sealed().await.unwrap());
    assert_eq!(t.sign(b"x").await.unwrap_err(), TransitError::Sealed);

    let s = Signer { pool: &pool, policy: &f.policy, cfg: &f.cfg, transit: &t, near: &f.near, notify: &f.notify, intents: None };
    let out = s.process_due(now()).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Paused { .. }), "{out:?}");
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::Approved, "nothing marked sent");
    assert!(f.near.sent().is_empty());

    // The owner unseals; payouts resume with the same key.
    let c = reqwest::Client::new();
    let r: Value = c.put(format!("{}/v1/sys/unseal", bao.addr)).json(&json!({ "key": bao.unseal_key })).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["sealed"], false);
    let out = s.process_due(now()).await.unwrap();
    assert!(matches!(out[0].1, SendResult::Sent { .. }), "{out:?}");
    assert_eq!(f.near.sent()[0].signer_id, pk.implicit_account());
}
