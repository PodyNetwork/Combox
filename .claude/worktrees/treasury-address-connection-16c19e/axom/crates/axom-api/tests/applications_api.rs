//! Requires DATABASE_URL. Admins see jobs-board applications and download
//! CVs; candidate deletion and retention remove the CV too.

use std::path::Path;
use std::sync::Arc;

use axom_api::auth::AuthConfig;
use axom_api::{router, AppState, ApprovalConfig, Services};
use axom_core::authz::AuthorizationKey;
use axom_core::domain::Actor;
use axom_core::Uuid;
use axom_hr::board::{self, Application, Cv};
use axom_hr::notify::NotifyConfig;
use axom_hr::PolicyConfig;
use axom_tools::mock::MockTools;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";
const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

fn jwt(sub: Uuid, aal: &str) -> String {
    let c = json!({ "sub": sub, "email": "owner@example.com", "aal": aal, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

fn app(pool: &PgPool) -> axum::Router {
    let p = policy();
    let mock = Arc::new(MockTools::new());
    let services = Services {
        policy: Arc::new(p.clone()),
        notify: NotifyConfig { owner_email: "owner@example.com".into(), dashboard_url: "https://admin.example.com".into() },
        rooms: mock.clone(),
        mailer: mock.clone(),
        crm: mock,
        livekit_ws_url: "wss://lk.example.com".into(),
        public_api_url: "https://api.example.com".into(),
        meet_web_url: "https://meet.example.com".into(),
        agent_keys: Default::default(),
    };
    let approvals = ApprovalConfig { auth: AuthConfig::hs256(SECRET), policy: Arc::new(p), approval_key: Arc::new(AuthorizationKey::generate()) };
    router(AppState { approvals: Some(approvals), services: Some(Arc::new(services)), ..AppState::new(pool.clone()) })
}

async fn get(app: &axum::Router, uri: &str, bearer: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let req = Request::builder().uri(uri).header("authorization", format!("Bearer {bearer}")).body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    (status, headers, axum::body::to_bytes(res.into_body(), 1 << 23).await.unwrap().to_vec())
}

fn application(name: &str, email: &str) -> Application {
    Application { name: name.into(), email: email.into(), skills: vec!["visual design".into()], message: "<b>hire me</b>".into(), portfolio_url: None, accept_privacy: true }
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn admins_list_applications_and_download_cvs(pool: PgPool) {
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let job = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.board_jobs (id, title, skills, rate_usd) VALUES ($1,'Brand Designer','{visual_design}',25)").bind(job).execute(&pool).await.unwrap();
    let dana = board::apply_with_cv(&pool, job, &application("Dana", "dana@example.com"), Some(&Cv { filename: "dana.pdf".into(), bytes: PDF.to_vec() })).await.unwrap();
    board::apply(&pool, job, &application("Sam", "sam@example.com")).await.unwrap();
    let app = app(&pool);
    let tok = jwt(admin, "aal2");

    let (s, _, body) = get(&app, "/admin/applications", &tok).await;
    assert_eq!(s, StatusCode::OK);
    let b: Value = serde_json::from_slice(&body).unwrap();
    let list = b["applications"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let d = list.iter().find(|a| a["name"] == "Dana").unwrap();
    assert_eq!(d["job_title"], "Brand Designer");
    assert_eq!(d["status"], "applied");
    assert_eq!(d["untrusted_message"], "<b>hire me</b>");
    assert_eq!(d["cv"]["filename"], "dana.pdf");
    assert!(list.iter().find(|a| a["name"] == "Sam").unwrap()["cv"].is_null());

    // The CV downloads as an attachment the browser won't run; audited.
    let (s, h, body) = get(&app, &format!("/admin/applications/{dana}/cv"), &tok).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, PDF);
    assert_eq!(h["content-type"], "application/pdf");
    assert_eq!(h["content-disposition"], "attachment; filename=\"dana.pdf\"");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["content-security-policy"], "sandbox");
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.audit_log WHERE action = 'candidate.cv_download'").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);

    // Only MFA-verified admins.
    assert_eq!(get(&app, &format!("/admin/applications/{dana}/cv"), &jwt(admin, "aal1")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get(&app, &format!("/admin/applications/{dana}/cv"), &jwt(Uuid::new_v4(), "aal2")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get(&app, "/admin/applications", &jwt(admin, "aal1")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get(&app, &format!("/admin/applications/{}/cv", Uuid::new_v4()), &tok).await.0, StatusCode::NOT_FOUND);

    // A deletion request removes the CV with the rest of the candidate's data.
    let (request, posting) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO axom.hiring_requests (id, kind, reason) VALUES ($1,'human_freelancer','test')").bind(request).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO axom.job_postings (id, hiring_request_id, marketplace, external_job_id, spec, contract_template, policy_version) VALUES ($1,$2,'axom_board',$3,'{}','freelancer-services-v1','test')")
        .bind(posting)
        .bind(request)
        .bind(job.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let candidate = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.candidates (id, job_posting_id, external_applicant_id, name, email) VALUES ($1,$2,$3,'Dana','dana@example.com')")
        .bind(candidate)
        .bind(posting)
        .bind(dana.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let owner = Actor::Admin { name: "Owner".into(), mfa_verified: true };
    axom_hr::privacy::delete_candidate_data(&pool, &policy(), &owner, candidate, "asked by email").await.unwrap();
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.board_application_cvs").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0, "the CV is deleted");
    assert_eq!(get(&app, &format!("/admin/applications/{dana}/cv"), &tok).await.0, StatusCode::NOT_FOUND);
}
