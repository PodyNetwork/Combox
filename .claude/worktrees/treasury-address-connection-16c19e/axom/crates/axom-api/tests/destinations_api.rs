//! Requires DATABASE_URL. Workers name a cross-chain payout destination
//! (NEAR Intents) by signing it with their verified NEAR key; admins see the
//! destination on held payouts.

use std::path::Path;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::intake::issue_worker_token;
use axom_api::{router, AppState, ApprovalConfig};
use axom_core::authz::AuthorizationKey;
use axom_core::db;
use axom_core::domain::{PayUnit, Worker, WorkerKind};
use axom_core::near::PublicKey;
use axom_core::state::WorkerStatus;
use axom_core::Uuid;
use axom_hr::payouts::{self, IntentRoute, PayoutRequest};
use axom_hr::{cap, gates, PolicyConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";
const USDC_BASE: &str = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
const ADDR: &str = "0x52908400098527886E0F7030069857D2E4169EE7";

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

async fn call(app: &axum::Router, method: &str, uri: &str, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let resp = app.clone().oneshot(b.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn worker(pool: &PgPool) -> Uuid {
    let w = Worker {
        id: Uuid::new_v4(),
        name: "Ade".into(),
        kind: WorkerKind::HumanFreelancer,
        role: "designer".into(),
        status: WorkerStatus::Active,
        skills: Default::default(),
        tool_allowlist: Default::default(),
        pay_rate_usd: dec!(10),
        pay_unit: PayUnit::Task,
        wallet_address: None,
        kpis: vec![],
    };
    db::insert_worker(pool, None, None, &w).await.unwrap();
    w.id
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn worker_signs_a_cross_chain_destination_and_admin_sees_it(pool: PgPool) {
    let p = policy();
    cap::seed_cap(&pool, &p).await.unwrap();
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p.clone()), approval_key: Arc::new(AuthorizationKey::generate()) };
    let app = router(AppState { approvals: Some(approvals), ..AppState::new(pool.clone()) });

    let w = worker(&pool).await;
    let token = issue_worker_token(&pool, w).await.unwrap();
    let dest = json!({ "chain": "base", "asset_id": USDC_BASE, "address": ADDR });

    // No token: refused. No verified NEAR address yet: refused.
    assert_eq!(call(&app, "GET", "/workers/me/payout-destination", None, Value::Null).await.0, StatusCode::UNAUTHORIZED);
    let (st, body) = call(&app, "POST", "/workers/me/payout-destination/challenge", Some(&token), dest.clone()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    // Verify a NEAR address (the worker's own key).
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let pk = PublicKey(sk.verifying_key().to_bytes());
    let account = pk.implicit_account();
    let ch = gates::issue_challenge(&pool, w, "testnet", &account, &pk.to_near_string(), Utc::now()).await.unwrap();
    gates::verify_challenge(&pool, &axom_tools::near_rpc::MockNear::new(), "testnet", ch.id, &hex::encode(sk.sign(ch.message.as_bytes()).to_bytes()), Utc::now())
        .await
        .unwrap();

    // Bad address: refused before any signature.
    let bad = json!({ "chain": "base", "asset_id": USDC_BASE, "address": "0x123" });
    assert_eq!(call(&app, "POST", "/workers/me/payout-destination/challenge", Some(&token), bad).await.0, StatusCode::BAD_REQUEST);

    let (st, ch) = call(&app, "POST", "/workers/me/payout-destination/challenge", Some(&token), dest.clone()).await;
    assert_eq!(st, StatusCode::CREATED, "{ch}");
    let message = ch["message"].as_str().unwrap();
    assert!(message.contains(ADDR) && message.contains(&account), "{message}");
    // A wrong signature is refused.
    let wrong = hex::encode(SigningKey::generate(&mut rand::rngs::OsRng).sign(message.as_bytes()).to_bytes());
    let (st, _) = call(&app, "POST", "/workers/me/payout-destination/verify", Some(&token), json!({ "challenge_id": ch["id"], "signature": wrong })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // The worker's own signature is accepted.
    let sig = hex::encode(sk.sign(message.as_bytes()).to_bytes());
    let (st, body) = call(&app, "POST", "/workers/me/payout-destination/verify", Some(&token), json!({ "challenge_id": ch["id"], "signature": sig })).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (_, mine) = call(&app, "GET", "/workers/me/payout-destination", Some(&token), Value::Null).await;
    assert_eq!(mine["paid_in"], "cross_chain");
    assert_eq!(mine["destination"]["destination"]["address"], ADDR);
    assert_eq!(mine["near_account"], account);

    // Another worker cannot use this worker's challenge.
    let other = worker(&pool).await;
    let other_token = issue_worker_token(&pool, other).await.unwrap();
    let (st, _) = call(&app, "POST", "/workers/me/payout-destination/verify", Some(&other_token), json!({ "challenge_id": ch["id"], "signature": sig })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A held cross-chain payout shows the admin where the money lands.
    let active = axom_hr::destinations::active(&pool, w, "testnet").await.unwrap().unwrap();
    let req = PayoutRequest {
        worker_id: Some(w),
        recipient: account.clone(),
        amount_usd: dec!(80),
        route: Some(IntentRoute { destination_id: active.id, destination: active.destination.clone(), route_fee_usd: dec!(0.8) }),
        ..Default::default()
    };
    payouts::create_payout(&pool, &p, &axom_core::domain::Actor::System, &req, None, Utc::now()).await.unwrap();
    let jwt = {
        let claims = json!({ "sub": admin, "email": "owner@example.com", "aal": "aal2", "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
        encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET)).unwrap()
    };
    let (st, pending) = call(&app, "GET", "/payouts/pending", Some(&jwt), Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pending["payouts"][0]["destination"]["address"], ADDR);
    assert_eq!(pending["payouts"][0]["destination"]["chain_name"], "Base");
    let (st, x) = call(&app, "GET", "/payouts/cross-chain", Some(&jwt), Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(x["payouts"][0]["status"], "pending_approval");
    assert_eq!(call(&app, "GET", "/payouts/cross-chain", Some(&token), Value::Null).await.0, StatusCode::UNAUTHORIZED, "workers cannot list payouts");

    // Revoking returns the worker to NEAR payouts.
    let (st, r) = call(&app, "DELETE", "/workers/me/payout-destination", Some(&token), Value::Null).await;
    assert_eq!((st, r["revoked"].clone()), (StatusCode::OK, json!(true)));
    let (_, mine) = call(&app, "GET", "/workers/me/payout-destination", Some(&token), Value::Null).await;
    assert_eq!(mine["paid_in"], "near");
}

/// A worker proves they hold the NEAR account they are paid at; until then
/// every payout to them is blocked.
#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn worker_verifies_their_near_address(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let w = worker(&pool).await;
    let other = worker(&pool).await;
    let token = issue_worker_token(&pool, w).await.unwrap();
    let other_token = issue_worker_token(&pool, other).await.unwrap();
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    let pk = PublicKey(sk.verifying_key().to_bytes());
    let account = pk.implicit_account();
    assert!(gates::check_payout_gates(&pool, w, "testnet").await.unwrap().unwrap_err().contains(&gates::GateBlock::AddressUnverified));

    let body = json!({ "account_id": account, "public_key": pk.to_near_string() });
    let path = "/workers/me/payout-address/challenge";
    assert_eq!(call(&app, "POST", path, None, body.clone()).await.0, StatusCode::UNAUTHORIZED);
    // Not an account, and an implicit account that isn't this key's.
    assert_eq!(call(&app, "POST", path, Some(&token), json!({ "account_id": "Not An Account", "public_key": pk.to_near_string() })).await.0, StatusCode::BAD_REQUEST);
    let someone_else = PublicKey(SigningKey::generate(&mut rand::rngs::OsRng).verifying_key().to_bytes()).implicit_account();
    assert_eq!(call(&app, "POST", path, Some(&token), json!({ "account_id": someone_else, "public_key": pk.to_near_string() })).await.0, StatusCode::BAD_REQUEST);

    let (st, ch) = call(&app, "POST", path, Some(&token), body).await;
    assert_eq!(st, StatusCode::CREATED, "{ch}");
    let message = ch["message"].as_str().unwrap();
    assert!(message.contains(&account), "{message}");
    let verify = "/workers/me/payout-address/verify";
    let sig = hex::encode(sk.sign(message.as_bytes()).to_bytes());
    // A signature by another key, and another worker using this challenge.
    let wrong = hex::encode(SigningKey::generate(&mut rand::rngs::OsRng).sign(message.as_bytes()).to_bytes());
    assert_eq!(call(&app, "POST", verify, Some(&token), json!({ "challenge_id": ch["id"], "signature": wrong })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", verify, Some(&other_token), json!({ "challenge_id": ch["id"], "signature": sig })).await.0, StatusCode::BAD_REQUEST);
    assert!(gates::verified_address(&pool, w, "testnet").await.unwrap().is_none());

    let (st, body) = call(&app, "POST", verify, Some(&token), json!({ "challenge_id": ch["id"], "signature": sig })).await;
    assert_eq!((st, body["near_account"].as_str()), (StatusCode::OK, Some(account.as_str())), "{body}");
    assert_eq!(gates::verified_address(&pool, w, "testnet").await.unwrap().as_deref(), Some(account.as_str()));
    assert!(gates::verified_address(&pool, other, "testnet").await.unwrap().is_none());
    // The address gate is now open (the tax form is still missing).
    assert!(!gates::check_payout_gates(&pool, w, "testnet").await.unwrap().unwrap_err().contains(&gates::GateBlock::AddressUnverified));
    // A challenge is used once.
    assert_eq!(call(&app, "POST", verify, Some(&token), json!({ "challenge_id": ch["id"], "signature": sig })).await.0, StatusCode::BAD_REQUEST);
    let (_, mine) = call(&app, "GET", "/workers/me/payout-destination", Some(&token), Value::Null).await;
    assert_eq!(mine["near_account"], account);
}
