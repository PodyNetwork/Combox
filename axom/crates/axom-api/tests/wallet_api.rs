//! Requires DATABASE_URL. The treasury's transaction history: deposits read
//! from a stand-in for FastNEAR's transactions API, payouts and Rhea moves
//! from the database, paged with a cursor and filtered in SQL.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::treasury::{TreasuryConfig, TreasurySource};
use axom_api::{router, AppState, ApprovalConfig};
use axom_core::authz::AuthorizationKey;
use axom_core::Uuid;
use axom_hr::PolicyConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Duration, TimeZone, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";
const TREASURY: &str = "35dd27a773ff9bba5182efa197c7c4907fdb84413205457e32cb3dd81b9cd7d4";
const NEAR: u128 = 1_000_000_000_000_000_000_000_000;

fn jwt(sub: Uuid, aal: &str) -> String {
    let c = json!({ "sub": sub, "email": "owner@example.com", "aal": aal, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

fn day(d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, d, 12, 0, 0).unwrap()
}

/// A transfer of `near` NEAR from `from` to the treasury, as FastNEAR shows it.
fn transfer(hash: &str, from: &str, near: u128, at: DateTime<Utc>) -> Value {
    let ns = at.timestamp_nanos_opt().unwrap();
    json!({
        "transaction": { "hash": hash, "signer_id": from, "receiver_id": TREASURY },
        "execution_outcome": { "block_height": 100, "block_timestamp": ns },
        "receipts": [
            { "execution_outcome": { "outcome": { "status": { "SuccessValue": "" } } },
              "receipt": { "predecessor_id": from, "receiver_id": TREASURY, "block_height": 101, "block_timestamp": ns,
                           "receipt": { "Action": { "actions": [{ "Transfer": { "deposit": (near * NEAR).to_string() } }] } } } },
            { "execution_outcome": { "outcome": { "status": { "SuccessValue": "" } } },
              "receipt": { "predecessor_id": "system", "receiver_id": from, "block_height": 102, "block_timestamp": ns,
                           "receipt": { "Action": { "actions": [{ "Transfer": { "deposit": "1000" } }] } } } }
        ]
    })
}

/// FastNEAR stand-in: the account lists two inbound transfers and one the
/// treasury signed itself (never fetched); counts detail requests.
async fn fastnear(detail_calls: Arc<AtomicUsize>) -> String {
    let app = Router::new()
        .route(
            "/v0/account",
            post(|Json(b): Json<Value>| async move {
                assert_eq!(b["account_id"], TREASURY);
                Json(json!({ "account_txs": [
                    { "transaction_hash": "OWN", "is_signer": true },
                    { "transaction_hash": "DEP2", "is_signer": false },
                    { "transaction_hash": "DEP1", "is_signer": false },
                ], "resume_token": null }))
            }),
        )
        .route(
            "/v0/transactions",
            post(move |Json(b): Json<Value>| {
                let calls = detail_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let hashes: Vec<String> = serde_json::from_value(b["tx_hashes"].clone()).unwrap();
                    assert!(!hashes.contains(&"OWN".to_string()), "the treasury's own transactions are recorded already");
                    let all = [transfer("DEP1", "alice.testnet", 5, day(1)), transfer("DEP2", "bob.testnet", 3, day(4))];
                    Json(json!({ "transactions": all.into_iter().filter(|t| hashes.contains(&t["transaction"]["hash"].as_str().unwrap().to_string())).collect::<Vec<_>>() }))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn app(pool: &PgPool, tx_api_url: String) -> Router {
    let p = PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap();
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p), approval_key: Arc::new(AuthorizationKey::generate()) };
    let treasury = TreasuryConfig { source: Some(TreasurySource::Account(TREASURY.into())), network: "testnet".into(), rpc_url: "http://127.0.0.1:1".into(), tx_api_url };
    router(AppState { approvals: Some(approvals), treasury, ..AppState::new(pool.clone()) })
}

async fn get(app: &Router, tok: &str, query: &str) -> (StatusCode, Value) {
    let req = Request::builder().uri(format!("/admin/treasury/transactions{query}")).header("authorization", format!("Bearer {tok}")).body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn kinds(v: &Value) -> Vec<String> {
    v["transactions"].as_array().unwrap().iter().map(|t| t["kind"].as_str().unwrap().to_string()).collect()
}

async fn seed(pool: &PgPool) {
    let worker = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.workers (id, name, kind, role, status, pay_rate_usd, pay_unit) VALUES ($1, 'Ada', 'human_freelancer', 'engineer', 'active', 50, 'hour')")
        .bind(worker)
        .execute(pool)
        .await
        .unwrap();
    // Payouts: one finalized, one sent, one never signed (not in the history),
    // and one on mainnet (not this treasury's network).
    for (status, tx, network, d) in [("finalized", Some("PAY1"), "testnet", 2), ("sent", Some("PAY2"), "testnet", 5), ("pending_approval", None, "testnet", 6), ("finalized", Some("PAYM"), "mainnet", 6)] {
        sqlx::query(
            "INSERT INTO axom.payouts (id, worker_id, recipient, amount_usd, status, policy_version, network, amount_yocto, tx_hash, sent_at, created_at)
             VALUES ($1, $2, 'ada.testnet', 10, $3, 'v1', $4, $5::numeric, $6, $7, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(worker)
        .bind(status)
        .bind(network)
        .bind((2 * NEAR).to_string())
        .bind(tx)
        .bind(day(d))
        .execute(pool)
        .await
        .unwrap();
    }
    // Rhea moves: a supply that succeeded and a withdraw that failed.
    for (kind, status, d) in [("supply", "succeeded", 3), ("withdraw", "failed", 7)] {
        sqlx::query(
            "INSERT INTO axom.treasury_moves (id, network, kind, amount_yocto, contract, reason, plan, policy_version, tx_hash, signed_tx, tx_nonce, status, created_at)
             VALUES ($1, 'testnet', $2, $3::numeric, 'contract.burrow.testnet', 'test', '{}', 'v1', $4, 'x', 1, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(kind)
        .bind(NEAR.to_string())
        .bind(format!("MOVE-{kind}"))
        .bind(status)
        .bind(day(d))
        .execute(pool)
        .await
        .unwrap();
    }
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn history_pages_and_filters_in_sql(pool: PgPool) {
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let tok = jwt(admin, "aal2");
    seed(&pool).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let a = app(&pool, fastnear(calls.clone()).await);

    // Everything, newest first: deposits synced from the chain, payouts that
    // reached the chain, and moves. Not the unsigned or mainnet payouts.
    let (s, v) = get(&a, &tok, "").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(kinds(&v), ["withdraw", "payout", "deposit", "supply", "payout", "deposit"]);
    assert_eq!(v["total"], 6);
    assert_eq!(v["account"], TREASURY);
    assert!(v["deposits"]["error"].is_null());
    assert!(v["next_cursor"].is_null());
    let dep = &v["transactions"][5];
    assert_eq!((dep["direction"].as_str(), dep["counterparty"].as_str(), dep["amount_near"].as_str()), (Some("in"), Some("alice.testnet"), Some("5")));
    assert_eq!(dep["explorer_url"], "https://testnet.nearblocks.io/txns/DEP1");
    let pay = &v["transactions"][1];
    assert_eq!((pay["direction"].as_str(), pay["label"].as_str(), pay["status"].as_str()), (Some("out"), Some("Ada"), Some("submitted")));
    assert_eq!(v["transactions"][0]["direction"], "internal");
    // Succeeded only: 5 + 3 in; 2 out (the sent payout isn't final yet).
    assert_eq!(v["totals"], json!({ "in_near": "8", "out_near": "2" }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM axom.treasury_deposits").fetch_one(&pool).await.unwrap();
    assert_eq!(stored, 2);

    // Pages of 2, by cursor, cover every row once in the same order.
    let mut seen = Vec::new();
    let mut cursor = String::new();
    for page in 0.. {
        let (s, v) = get(&a, &tok, &format!("?limit=2&cursor={cursor}")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["total"], 6, "the total ignores the cursor");
        seen.extend(kinds(&v));
        match v["next_cursor"].as_str() {
            Some(c) => cursor = c.to_string(),
            None => {
                assert_eq!(page, 2);
                break;
            }
        }
    }
    assert_eq!(seen, ["withdraw", "payout", "deposit", "supply", "payout", "deposit"]);

    // Filters: direction, kind, status, and a date range (from inclusive, to exclusive).
    let (_, v) = get(&a, &tok, "?direction=in").await;
    assert_eq!((kinds(&v), v["total"].clone()), (vec!["deposit".to_string(), "deposit".into()], json!(2)));
    let (_, v) = get(&a, &tok, "?direction=out,internal&status=succeeded").await;
    assert_eq!(kinds(&v), ["supply", "payout"]);
    let (_, v) = get(&a, &tok, "?kind=register,supply,withdraw,unwrap").await;
    assert_eq!(kinds(&v), ["withdraw", "supply"]);
    let (_, v) = get(&a, &tok, "?status=failed").await;
    assert_eq!(kinds(&v), ["withdraw"]);
    let (_, v) = get(&a, &tok, "?from=2026-11-02&to=2026-11-05").await;
    assert_eq!(kinds(&v), ["deposit", "supply", "payout"]);
    assert_eq!(v["totals"], json!({ "in_near": "3", "out_near": "2" }));
    let (_, v) = get(&a, &tok, "?direction=in&from=2026-11-02&limit=1").await;
    assert_eq!((kinds(&v), v["total"].clone(), v["next_cursor"].is_null()), (vec!["deposit".to_string()], json!(1), true));

    // Within 30 s the chain isn't asked again.
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Bad input is refused; only MFA-verified admins.
    for bad in ["?direction=sideways", "?limit=500", "?cursor=nope", "?from=2026-11-05&to=2026-11-01"] {
        let (s, v) = get(&a, &tok, bad).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
        assert!(v["error"].is_string());
    }
    assert_eq!(get(&a, &jwt(admin, "aal1"), "").await.0, StatusCode::FORBIDDEN);
    assert_eq!(get(&a, &jwt(Uuid::new_v4(), "aal2"), "").await.0, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn history_answers_when_the_chain_cant_be_read(pool: PgPool) {
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    seed(&pool).await;
    // Nothing listens here.
    let a = app(&pool, "http://127.0.0.1:1".into());
    let (s, v) = get(&a, &jwt(admin, "aal2"), "").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(kinds(&v), ["withdraw", "payout", "supply", "payout"]);
    assert!(v["deposits"]["error"].as_str().unwrap().contains("NEAR transactions API"));
}
