//! Requires DATABASE_URL. Single-company setup without SQL: the first
//! two-factor sign-in by AXOM_OWNER_EMAIL becomes the first admin, and
//! nobody else ever does through this path.

use std::path::Path;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::{router, AppState, ApprovalConfig};
use axom_core::authz::AuthorizationKey;
use axom_core::Uuid;
use axom_hr::PolicyConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";
const OWNER: &str = "owner@example.com";

fn policy() -> PolicyConfig {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml");
    PolicyConfig::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn token(sub: Uuid, email: &str, aal: &str) -> String {
    let claims = json!({
        "sub": sub, "email": email, "aal": aal, "session_id": "sess-1",
        "aud": "authenticated", "role": "authenticated", "exp": (Utc::now() + Duration::minutes(5)).timestamp(),
    });
    encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET)).unwrap()
}

fn app(pool: &PgPool, owner_email: Option<&str>) -> axum::Router {
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(policy()), approval_key: Arc::new(AuthorizationKey::generate()) };
    router(AppState { approvals: Some(approvals), owner_email: owner_email.map(str::to_string), ..AppState::new(pool.clone()) })
}

/// An admin-only page, as the dashboard calls it after sign-in.
async fn get_admin(app: &axum::Router, bearer: &str) -> StatusCode {
    let req = Request::get("/admin/meetings").header("authorization", format!("Bearer {bearer}")).body(Body::empty()).unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn admins(pool: &PgPool) -> Vec<(Uuid, String, bool)> {
    sqlx::query_as("SELECT user_id, email, active FROM axom.admins ORDER BY created_at").fetch_all(pool).await.unwrap()
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn owner_becomes_first_admin_on_two_factor_sign_in(pool: PgPool) {
    let app = app(&pool, Some(OWNER));
    let owner = Uuid::new_v4();
    // Email matching ignores case and surrounding spaces.
    assert_eq!(get_admin(&app, &token(owner, "Owner@Example.com", "aal2")).await, StatusCode::OK);
    assert_eq!(admins(&pool).await, vec![(owner, "Owner@Example.com".into(), true)]);
    let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM axom.audit_log WHERE action = 'admin.bootstrap_owner'").fetch_one(&pool).await.unwrap();
    assert_eq!(audited, 1);
    // Later requests keep working and add nothing.
    assert_eq!(get_admin(&app, &token(owner, OWNER, "aal2")).await, StatusCode::OK);
    assert_eq!(admins(&pool).await.len(), 1);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn owner_without_two_factor_is_not_added(pool: PgPool) {
    let app = app(&pool, Some(OWNER));
    assert_eq!(get_admin(&app, &token(Uuid::new_v4(), OWNER, "aal1")).await, StatusCode::FORBIDDEN);
    assert!(admins(&pool).await.is_empty());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn someone_else_is_not_added(pool: PgPool) {
    let app = app(&pool, Some(OWNER));
    assert_eq!(get_admin(&app, &token(Uuid::new_v4(), "someone@example.com", "aal2")).await, StatusCode::FORBIDDEN);
    assert!(admins(&pool).await.is_empty());
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn nobody_is_added_once_an_admin_exists(pool: PgPool) {
    let existing = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'first@example.com','First')").bind(existing).execute(&pool).await.unwrap();
    let app = app(&pool, Some(OWNER));
    assert_eq!(get_admin(&app, &token(Uuid::new_v4(), OWNER, "aal2")).await, StatusCode::FORBIDDEN);
    assert_eq!(admins(&pool).await.len(), 1);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn a_deactivated_owner_is_not_restored(pool: PgPool) {
    let owner = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name, active) VALUES ($1,$2,'Owner',false)").bind(owner).bind(OWNER).execute(&pool).await.unwrap();
    let app = app(&pool, Some(OWNER));
    assert_eq!(get_admin(&app, &token(owner, OWNER, "aal2")).await, StatusCode::FORBIDDEN);
    assert_eq!(admins(&pool).await, vec![(owner, OWNER.into(), false)]);
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn off_without_an_owner_email(pool: PgPool) {
    // The platform and judge mode leave owner_email unset.
    for owner_email in [None, Some(""), Some("   ")] {
        let app = app(&pool, owner_email);
        assert_eq!(get_admin(&app, &token(Uuid::new_v4(), OWNER, "aal2")).await, StatusCode::FORBIDDEN);
    }
    assert!(admins(&pool).await.is_empty());
}
