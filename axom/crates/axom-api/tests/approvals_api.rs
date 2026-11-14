//! Requires DATABASE_URL. Approval endpoints accept only a named admin with
//! an MFA-verified (aal2) Supabase session.

use std::path::Path;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::{router, AppState, ApprovalConfig};
use axom_core::authz::AuthorizationKey;
use axom_core::domain::Actor;
use axom_core::state::PayoutStatus;
use axom_core::Uuid;
use axom_hr::payouts::{self, PayoutRequest};
use axom_hr::{cap, PolicyConfig, PriceQuote};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_decimal_macros::dec;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";

fn policy() -> PolicyConfig {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml");
    PolicyConfig::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn token(sub: Uuid, aal: &str, secret: &[u8], exp_in: Duration) -> String {
    let claims = json!({
        "sub": sub, "email": "owner@example.com", "aal": aal, "session_id": "sess-123",
        "aud": "authenticated", "role": "authenticated", "exp": (Utc::now() + exp_in).timestamp(),
    });
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret)).unwrap()
}

async fn setup(pool: &PgPool) -> (axum::Router, Uuid, Uuid) {
    let p = policy();
    cap::seed_cap(pool, &p).await.unwrap();
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(pool).await.unwrap();
    let now = Utc::now();
    let quote = PriceQuote { usd_per_near: dec!(4), fetched_at: now };
    let req = PayoutRequest { recipient: "big.testnet".into(), amount_usd: dec!(80), ..Default::default() };
    let c = payouts::create_payout(pool, &p, &Actor::System, &req, Some(quote), now).await.unwrap();
    assert_eq!(c.status, PayoutStatus::PendingApproval);
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p), approval_key: Arc::new(AuthorizationKey::generate()) };
    (router(AppState { approvals: Some(approvals), ..AppState::new(pool.clone()) }), admin, c.payout_id)
}

async fn post(app: &axum::Router, uri: &str, bearer: Option<&str>) -> StatusCode {
    let mut b = Request::post(uri).header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    app.clone().oneshot(b.body(Body::from("{\"reason\":\"no\"}")).unwrap()).await.unwrap().status()
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn approval_requires_mfa_admin_session(pool: PgPool) {
    let (app, admin, id) = setup(&pool).await;
    let uri = format!("/payouts/{id}/approve");

    assert_eq!(post(&app, &uri, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(post(&app, &uri, Some("not-a-jwt")).await, StatusCode::UNAUTHORIZED);
    assert_eq!(post(&app, &uri, Some(&token(admin, "aal2", b"wrong-secret-wrong-secret-wrong!!", Duration::hours(1)))).await, StatusCode::UNAUTHORIZED);
    assert_eq!(post(&app, &uri, Some(&token(admin, "aal2", SECRET, Duration::hours(-1)))).await, StatusCode::UNAUTHORIZED, "expired");
    assert_eq!(post(&app, &uri, Some(&token(admin, "aal1", SECRET, Duration::hours(1)))).await, StatusCode::FORBIDDEN, "no MFA");
    assert_eq!(post(&app, &uri, Some(&token(Uuid::new_v4(), "aal2", SECRET, Duration::hours(1)))).await, StatusCode::FORBIDDEN, "not an admin");
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::PendingApproval);

    assert_eq!(post(&app, &uri, Some(&token(admin, "aal2", SECRET, Duration::hours(1)))).await, StatusCode::OK);
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::Approved);
    assert_eq!(post(&app, &uri, Some(&token(admin, "aal2", SECRET, Duration::hours(1)))).await, StatusCode::CONFLICT, "already approved");
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn reject_requires_mfa_and_releases_cap(pool: PgPool) {
    let (app, admin, id) = setup(&pool).await;
    let uri = format!("/payouts/{id}/reject");
    assert_eq!(post(&app, &uri, Some(&token(admin, "aal1", SECRET, Duration::hours(1)))).await, StatusCode::FORBIDDEN);
    assert_eq!(post(&app, &uri, Some(&token(admin, "aal2", SECRET, Duration::hours(1)))).await, StatusCode::OK);
    assert_eq!(payouts::status(&pool, id).await.unwrap(), PayoutStatus::Rejected);
    assert_eq!(cap::usage(&pool, cap::month_of(Utc::now())).await.unwrap().used_usd(), dec!(0));
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn approvals_disabled_without_config(pool: PgPool) {
    let app = router(AppState::new(pool));
    assert_eq!(post(&app, &format!("/payouts/{}/approve", Uuid::new_v4()), Some("x")).await, StatusCode::SERVICE_UNAVAILABLE);
}
