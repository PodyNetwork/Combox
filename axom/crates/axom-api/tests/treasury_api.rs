//! Requires DATABASE_URL. The treasury view: MFA admins only, the account
//! from the signer (refused if it's on another network), the balance from
//! NEAR, and an account that isn't on chain yet. A local stand-in plays the
//! signer and the NEAR RPC node; nothing here sends a transaction.

use std::path::Path;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::treasury::{TreasuryConfig, TreasurySource};
use axom_api::{router, AppState, ApprovalConfig};
use axom_core::authz::AuthorizationKey;
use axom_core::Uuid;
use axom_hr::PolicyConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";
const FUNDED: &str = "98793cd91a3f870fb126f66285808c7e094afcfc4eda8a970f6648cdf0dbd6de";
const EMPTY: &str = "a5a4a4f6f4de7f1b4b6d3a1f1a4a7b1b8e0c3b2b1a7f5e4d3c2b1a0f9e8d7c6b";

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

fn jwt(sub: Uuid, aal: &str) -> String {
    let c = json!({ "sub": sub, "email": "owner@example.com", "aal": aal, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

/// Serves `GET /treasury` (signer, reporting `account` on `network`) and
/// NEAR's `view_account` at `POST /rpc`.
async fn stand_in(account: &'static str, network: &'static str) -> String {
    let app = Router::new()
        .route("/treasury", get(move || async move { Json(json!({ "network": network, "account": account, "public_key": "ed25519:x" })) }))
        .route(
            "/rpc",
            post(|Json(b): Json<Value>| async move {
                assert_eq!(b["method"], "query");
                assert_eq!(b["params"]["request_type"], "view_account", "read-only queries only");
                if b["params"]["account_id"] == FUNDED {
                    Json(json!({ "jsonrpc": "2.0", "id": b["id"], "result": { "amount": "12500000000000000000000000", "locked": "0", "storage_usage": 182, "block_height": 1 } }))
                } else {
                    Json(json!({ "jsonrpc": "2.0", "id": b["id"], "error": { "name": "HANDLER_ERROR", "cause": { "name": "UNKNOWN_ACCOUNT" } } }))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn app(pool: &PgPool, source: Option<TreasurySource>, network: &str, rpc_url: String) -> Router {
    let p = policy();
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p), approval_key: Arc::new(AuthorizationKey::generate()) };
    let treasury = TreasuryConfig { source, network: network.into(), rpc_url, tx_api_url: "http://127.0.0.1:1".into() };
    router(AppState { approvals: Some(approvals), treasury, ..AppState::new(pool.clone()) })
}

async fn get_json(app: &Router, bearer: &str) -> (StatusCode, Value) {
    let req = Request::builder().uri("/admin/treasury").header("authorization", format!("Bearer {bearer}")).body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn admins_see_the_treasury_and_its_balance(pool: PgPool) {
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let tok = jwt(admin, "aal2");
    // A pinned testnet rate, so the dollar value doesn't depend on the market.
    std::env::set_var("AXOM_DEMO_USD_PER_NEAR", "1000");

    // From the signer, with the balance NEAR reports.
    let base = stand_in(FUNDED, "testnet").await;
    let a = app(&pool, Some(TreasurySource::Signer(base.clone())), "testnet", format!("{base}/rpc"));
    let (s, v) = get_json(&a, &tok).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["network"], "testnet");
    assert_eq!(v["managed_by"], "signer");
    assert_eq!(v["account"], FUNDED);
    assert_eq!(v["explorer_url"], format!("https://testnet.nearblocks.io/address/{FUNDED}"));
    assert_eq!(v["balance"]["on_chain"], true);
    assert_eq!(v["balance"]["near"], "12.5");
    assert_eq!(v["balance"]["yocto"], "12500000000000000000000000");
    assert_eq!(v["balance"]["available_near"], "12.49818");
    assert_eq!(v["balance"]["usd"], "12500.00");
    assert_eq!(v["price"]["usd_per_near"], "1000");
    assert_eq!(v["price"]["pinned"], true);
    assert!(v["error"].is_null());

    // Only MFA-verified, active admins.
    assert_eq!(get_json(&a, &jwt(admin, "aal1")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get_json(&a, &jwt(Uuid::new_v4(), "aal2")).await.0, StatusCode::FORBIDDEN);

    // A known account that hasn't received anything yet.
    let a = app(&pool, Some(TreasurySource::Account(EMPTY.into())), "testnet", format!("{base}/rpc"));
    let (_, v) = get_json(&a, &tok).await;
    assert_eq!(v["managed_by"], "platform");
    assert_eq!(v["account"], EMPTY);
    assert_eq!(v["balance"]["on_chain"], false);
    assert_eq!(v["balance"]["near"], "0");

    // A signer on another network: no address is shown at all.
    let a = app(&pool, Some(TreasurySource::Signer(base.clone())), "mainnet", format!("{base}/rpc"));
    let (s, v) = get_json(&a, &tok).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["account"].is_null());
    assert!(v["error"].as_str().unwrap().contains("testnet"));

    // No signer reachable, and no treasury configured.
    let a = app(&pool, Some(TreasurySource::Signer("http://127.0.0.1:1".into())), "testnet", format!("{base}/rpc"));
    let (_, v) = get_json(&a, &tok).await;
    assert!(v["account"].is_null());
    assert!(v["error"].as_str().unwrap().contains("signer"));
    let (_, v) = get_json(&app(&pool, None, "testnet", format!("{base}/rpc")), &tok).await;
    assert_eq!((v["configured"].clone(), v["managed_by"].clone()), (json!(false), json!("none")));
    assert!(v["account"].is_null() && v["error"].is_null());
}
