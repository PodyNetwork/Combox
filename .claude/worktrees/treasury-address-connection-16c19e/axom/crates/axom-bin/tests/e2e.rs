//! Requires DATABASE_URL and python3. The seed plan end to end against mocks:
//! org chart and routing, an AI interview (the real Python interview logic
//! posting real HTTP callbacks), a firing, and payouts under/over the
//! threshold and over the cap, sent by the real signer code against a local
//! transit key and an in-memory NEAR.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use axom_api::intake::DbTaskIntake;
use axom_api::{router, AppState, Services};
use axom_app::demo;
use axom_core::authz::AuthorizationKey;
use axom_core::near::PublicKey;
use axom_core::state::PayoutStatus;
use axom_core::Uuid;
use axom_hr::approvals::{self, AdminSession};
use axom_hr::notify::NotifyConfig;
use axom_hr::{notify, payouts, PriceQuote};
use axom_signer::openbao::{TransitError, TransitSigner};
use axom_signer::{SendResult, Signer, SignerConfig};
use axom_tools::mock::MockTools;
use axom_tools::near_rpc::MockNear;
use chrono::{Duration, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use rust_decimal_macros::dec;
use sqlx::PgPool;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

struct LocalTransit(SigningKey);

#[async_trait]
impl TransitSigner for LocalTransit {
    async fn is_sealed(&self) -> Result<bool, TransitError> {
        Ok(false)
    }
    async fn public_key(&self) -> Result<PublicKey, TransitError> {
        Ok(PublicKey(self.0.verifying_key().to_bytes()))
    }
    async fn sign(&self, m: &[u8]) -> Result<[u8; 64], TransitError> {
        Ok(self.0.sign(m).to_bytes())
    }
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn seed_plan_end_to_end_against_mocks(pool: PgPool) {
    let c = demo::Configs::load(&root()).unwrap();
    let now = Utc::now();
    let mocks = Arc::new(MockTools::new());
    let intake = DbTaskIntake { pool: pool.clone() };
    let notify_cfg = NotifyConfig { owner_email: "owner@example.com".into(), dashboard_url: "http://localhost:5174".into() };

    // 1. Org chart built and tasks routed.
    let seed = demo::seed(&pool, &c, mocks.as_ref(), &intake, now).await.unwrap();
    assert!(!seed.plan_reused);
    assert_eq!(seed.tasks_assigned, 5);
    assert_eq!(seed.hiring_requests, 1);
    assert_eq!(seed.tasks_done_by_mock_tools, 5);

    // 2. A job posts to Upwork (mock) for the human hiring request.
    let posted = axom_hr::hiring::process_open_requests(&pool, &c.policy, mocks.as_ref()).await.unwrap();
    assert!(posted.iter().any(|(_, r)| r.starts_with("posted job")), "{posted:?}");
    assert_eq!(mocks.calls_to("upwork").iter().filter(|x| x.method == "post_job").count(), 1);

    // 3. Interview: embedded API for callbacks, Python agent logic as the interviewer.
    let services = Services {
        policy: Arc::new(c.policy.clone()),
        notify: notify_cfg.clone(),
        rooms: mocks.clone(),
        mailer: mocks.clone(),
        crm: mocks.clone(),
        livekit_ws_url: "ws://localhost:7880".into(),
        public_api_url: String::new(),
        meet_web_url: "http://localhost:5173".into(),
        agent_keys: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_url = format!("http://{}", listener.local_addr().unwrap());
    let app = router(AppState { services: Some(Arc::new(services)), ..AppState::new(pool.clone()) });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let m2 = mocks.clone();
    let (meeting_id, metadata) = demo::interview(&pool, &c, mocks.as_ref(), &api_url, move || {
        m2.calls_to("livekit").into_iter().rev().find(|x| x.method == "dispatch_agent").and_then(|x| x.args["metadata"].as_str().map(str::to_string))
    })
    .await
    .unwrap();
    let meta_file = std::env::temp_dir().join(format!("axom-job-{meeting_id}.json"));
    std::fs::write(&meta_file, metadata.to_string()).unwrap();
    let out = tokio::process::Command::new("python3")
        .args(["-m", "axom_meet.simulate", "--metadata"])
        .arg(&meta_file)
        .current_dir(root().join("meet-agent"))
        .output()
        .await
        .expect("python3 must be installed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "simulator failed: {stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("AGENT: Hello, I'm an AI interviewer"), "{stdout}");
    assert!(stdout.contains("not able to discuss pay"), "the agent refused to commit on pay: {stdout}");
    let (status, consent, scores): (String, serde_json::Value, serde_json::Value) =
        sqlx::query_as("SELECT status, consent, scores FROM axom.meetings WHERE id = $1").bind(meeting_id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "completed");
    assert_eq!(consent["granted"], true);
    assert!(scores.as_object().unwrap().len() >= 3);

    // 4. The seeded low performer is fired, with an owner notification.
    let fired = demo::fire_low_performer(&pool, &c, &notify_cfg, seed.low_performer, now).await.unwrap();
    assert_eq!(fired.reopened_tasks.len(), 1);
    let n = notify::get(&pool, fired.notification_id).await.unwrap().unwrap();
    assert!(n.body["reason"].as_str().unwrap().contains("consecutive"));

    // 5. Payouts: < 50 sent automatically, >= 50 held, over-cap blocked.
    let attestor = AuthorizationKey::generate();
    let approver = AuthorizationKey::generate();
    let price = PriceQuote { usd_per_near: dec!(3), fetched_at: now - Duration::minutes(1) };
    let p = demo::payouts(&pool, &c, &attestor, price, "testnet", now).await.unwrap();
    assert_eq!(notify::count_kind(&pool, "cap_exceeded").await.unwrap(), 1, "owner notified of the blocked payout");

    let transit = LocalTransit(SigningKey::generate(&mut rand::rngs::OsRng));
    let treasury = transit.public_key().await.unwrap();
    let near = MockNear::new();
    near.add_access_key(&treasury.implicit_account(), treasury, 1);
    let intents = axom_tools::intents::SimulatedIntents::new();
    let cfg = SignerConfig { network: "testnet".into(), attestation_key: attestor.public_key(), approval_key: approver.public_key(), max_attempts: 5, base_backoff: Duration::seconds(30) };
    let signer = Signer { pool: &pool, policy: &c.policy, cfg: &cfg, transit: &transit, near: &near, notify: &notify_cfg, intents: Some(&intents) };
    let sent = signer.process_due(Utc::now()).await.unwrap();
    assert_eq!(sent.len(), 2, "the small payout and the cross-chain payout are due: {sent:?}");
    assert!(sent.iter().all(|(_, r)| matches!(r, SendResult::Sent { .. })), "{sent:?}");
    let txs = near.sent();
    assert!(txs.iter().any(|t| t.receiver_id == p.small_recipient), "NEAR transfer to the small payout's recipient");
    assert!(txs.iter().any(|t| t.receiver_id == "wrap.testnet" && t.actions.len() == 4), "wNEAR deposit into the intent");
    assert_eq!(intents.quotes()[0].recipient, p.cross_chain_destination.address);
    signer.finalize_sent(Utc::now()).await.unwrap();
    assert_eq!(payouts::status(&pool, p.small).await.unwrap(), PayoutStatus::Finalized);
    assert_eq!(payouts::status(&pool, p.cross_chain).await.unwrap(), PayoutStatus::Finalized, "delivered on Base (simulated)");
    assert_eq!(payouts::status(&pool, p.held).await.unwrap(), PayoutStatus::PendingApproval);
    assert_eq!(payouts::status(&pool, p.blocked).await.unwrap(), PayoutStatus::CapExceeded);

    // 6. The held payout proceeds only after a dashboard approval.
    let admin = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,'owner@example.com','Owner')").bind(admin).execute(&pool).await.unwrap();
    let no_mfa = AdminSession { user_id: admin, email: "owner@example.com".into(), session_id: "s".into(), mfa_verified: false };
    assert!(approvals::approve_payout(&pool, &c.policy, &approver, &no_mfa, p.held, Utc::now()).await.is_err());
    let mfa = AdminSession { mfa_verified: true, ..no_mfa };
    approvals::approve_payout(&pool, &c.policy, &approver, &mfa, p.held, Utc::now()).await.unwrap();
    let sent = signer.process_due(Utc::now()).await.unwrap();
    assert!(matches!(sent[0].1, SendResult::Sent { .. }), "{sent:?}");

    // 7. Final report.
    let r = demo::final_report(&pool, Utc::now()).await.unwrap();
    assert_eq!(r.fired.len(), 1);
    assert_eq!(r.blocked.iter().filter(|b| b["reason"].as_str().unwrap_or("").contains("monthly cap")).count(), 1);
    assert!(r.committed_usd > dec!(240), "history 180 + 5 + 60 committed: {}", r.committed_usd);
    assert!(r.waiting_for_approval.is_empty());
    assert_eq!(r.cross_chain.len(), 1);
    assert_eq!(r.cross_chain[0]["intent_status"], "SUCCESS");
}
