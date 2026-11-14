//! Requires DATABASE_URL (a role with CREATEDB). Company sign-up with its own
//! database, isolation between companies, routing, integration keys,
//! invitations, and NEAR subscription payments.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axom_api::auth::AuthConfig;
use axom_core::authz::AuthorizationKey;
use axom_hr::approvals::AdminSession;
use axom_hr::{PolicyConfig, PriceQuote};
use axom_platform::billing::{PaymentError, PaymentLookup, PlanConfig, Transfer};
use axom_platform::crypto::SecretBox;
use axom_platform::http::{self, PlatformState};
use axom_platform::{Platform, Subscription, TenantError};
use axom_tools::config::ConfigSource;
use axom_tools::price::FixedPriceFeed;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &[u8] = b"test-supabase-jwt-secret-at-least-32-bytes";

fn policy() -> PolicyConfig {
    PolicyConfig::parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml")).unwrap()).unwrap()
}

fn plan() -> PlanConfig {
    PlanConfig { name: "ForeMan".into(), price_usd: dec!(49), trial_days: 14, pay_to: Some("axom-billing.testnet".into()), network: "testnet".into(), invoice_ttl_hours: 24 }
}

async fn platform(pool: &PgPool) -> Arc<Platform> {
    Arc::new(Platform::new(pool.clone(), SecretBox::from_hex(&SecretBox::generate_hex()).unwrap(), Arc::new(policy()), plan()).await.unwrap())
}

fn user(email: &str) -> AdminSession {
    AdminSession { user_id: Uuid::new_v4(), email: email.into(), session_id: "s".into(), mfa_verified: true }
}

fn jwt(s: &AdminSession) -> String {
    let c = json!({ "sub": s.user_id, "email": s.email, "aal": if s.mfa_verified { "aal2" } else { "aal1" }, "session_id": "s", "aud": "authenticated", "exp": (Utc::now() + Duration::hours(1)).timestamp() });
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

/// Company databases live outside the per-test database: drop them.
async fn cleanup(p: &Platform) {
    let dbs: Vec<(String,)> = sqlx::query_as("SELECT db_name FROM platform.tenants").fetch_all(&p.db).await.unwrap();
    for t in p.active_tenants().await.unwrap() {
        p.pool(&t).await.close().await;
    }
    for (db,) in dbs {
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db}\" WITH (FORCE)")).execute(&p.db).await.unwrap();
    }
}

#[sqlx::test(migrations = false)]
async fn companies_get_their_own_databases(pool: PgPool) {
    let p = platform(&pool).await;
    let (ada, bob) = (user("ada@acme.example"), user("bob@globex.example"));
    let now = Utc::now();

    let acme = p.create_tenant(&ada, "Acme Labs", "acme", now).await.unwrap();
    let globex = p.create_tenant(&bob, "Globex", "globex", now).await.unwrap();
    assert_eq!(acme.status, "active");
    assert!(matches!(acme.subscription(now), Subscription::Trial { .. }));
    assert_ne!(acme.db_name, globex.db_name);

    // Taken, reserved, and unverified sign-ups are refused.
    assert!(matches!(p.create_tenant(&bob, "Other", "acme", now).await, Err(TenantError::SlugTaken)));
    assert!(matches!(p.create_tenant(&bob, "Other", "admin", now).await, Err(TenantError::Invalid(_))));
    let no_mfa = AdminSession { mfa_verified: false, ..user("eve@x.example") };
    assert!(matches!(p.create_tenant(&no_mfa, "Eve", "eve-co", now).await, Err(TenantError::Forbidden(_))));

    // Each owner is an admin only in their own company's database, and data
    // written for one company is invisible to the other.
    let (a, g) = (p.pool(&acme).await, p.pool(&globex).await);
    let admins = |pool: PgPool| async move { sqlx::query_as::<_, (String,)>("SELECT email FROM axom.admins").fetch_all(&pool).await.unwrap() };
    assert_eq!(admins(a.clone()).await, vec![("ada@acme.example".to_string(),)]);
    assert_eq!(admins(g.clone()).await, vec![("bob@globex.example".to_string(),)]);
    sqlx::query("UPDATE axom.cap_settings SET cap_usd = 900, updated_by = 'test' WHERE id = 1").execute(&a).await.unwrap();
    let cap = |pool: PgPool| async move { sqlx::query_as::<_, (rust_decimal::Decimal,)>("SELECT cap_usd FROM axom.cap_settings").fetch_one(&pool).await.unwrap().0 };
    assert_eq!(cap(a.clone()).await, dec!(900));
    assert_eq!(cap(g.clone()).await, dec!(500), "the other company keeps its own cap");

    let mine = p.memberships(&ada).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].0.slug.as_str(), mine[0].1.as_str()), ("acme", "owner"));
    cleanup(&p).await;
}

fn state(p: Arc<Platform>, payments: Arc<dyn PaymentLookup>) -> Arc<PlatformState> {
    let price = Arc::new(FixedPriceFeed(PriceQuote { usd_per_near: dec!(3), fetched_at: Utc::now() }));
    Arc::new(PlatformState::new(
        p,
        AuthConfig::hs256(SECRET),
        Arc::new(AuthorizationKey::generate()),
        price,
        payments,
        "https://api.axom.example".into(),
        "https://jobs.axom.example".into(),
        "https://app.axom.example".into(),
    ))
}

async fn call(app: &axum::Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let res = app.clone().oneshot(req.body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct NoPayments;

#[async_trait]
impl PaymentLookup for NoPayments {
    async fn transfer(&self, _: &str, _: &str) -> anyhow::Result<Option<Transfer>> {
        Ok(None)
    }
}

#[sqlx::test(migrations = false)]
async fn signup_and_routing_over_http(pool: PgPool) {
    let p = platform(&pool).await;
    let app = http::router(state(p.clone(), Arc::new(NoPayments)));
    let (ada, bob) = (user("ada@acme.example"), user("bob@globex.example"));
    let (ta, tb) = (jwt(&ada), jwt(&bob));

    let (s, _) = call(&app, "GET", "/platform/me", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, b) = call(&app, "POST", "/platform/companies", Some(&ta), Some(json!({ "name": "Acme Labs", "slug": "acme" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["subscription"]["state"], "trial");
    let (_, b) = call(&app, "GET", "/platform/me", Some(&ta), None).await;
    assert_eq!(b["companies"][0]["company"]["slug"], "acme");
    assert!(b["companies"][0]["company"].get("db_name").is_none(), "internal names are not exposed");

    // The company's own API, under /t/acme/.
    let (s, b) = call(&app, "GET", "/t/acme/health", None, None).await;
    assert_eq!((s, b["status"].as_str()), (StatusCode::OK, Some("ok")));
    let (s, b) = call(&app, "GET", "/t/acme/admin/cap", Some(&ta), None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    // Another signed-in person is not an admin of Acme.
    let (s, _) = call(&app, "GET", "/t/acme/admin/cap", Some(&tb), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&app, "GET", "/platform/companies/acme", Some(&tb), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "non-members can't even see the company");
    let (s, _) = call(&app, "GET", "/t/nope/health", None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // The public jobs board works per company, with query strings intact.
    let (s, b) = call(&app, "GET", "/t/acme/jobs?x=1", None, None).await;
    assert_eq!((s, b["jobs"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)));

    // Company routes with an id in the path work under /t/<company>/.
    let (s, b) = call(&app, "GET", &format!("/t/acme/jobs/{}", Uuid::new_v4()), None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
    assert!(b["error"].is_string(), "the company's own 404, not a failed extraction: {b}");
    let (s, b) = call(&app, "GET", &format!("/t/acme/admin/meetings/{}", Uuid::new_v4()), Some(&ta), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
    // Members-only data can't be reached by hiding the path behind an
    // encoded `?` or `/`, and a signed-in outsider gets nothing either.
    for path in ["/t/acme/tasks%3F", "/t/acme/tasks%3Fx=1", "/t/acme/notifications%3F", "/t/acme/org-chart%2F", "/t/acme/%74asks", "/t/acme//tasks", "/t/acme/spot-checks%3F/stats"] {
        let (s, b) = call(&app, "GET", path, None, None).await;
        assert!(s == StatusCode::NOT_FOUND || s == StatusCode::UNAUTHORIZED, "{path}: {s} {b}");
    }
    assert_eq!(call(&app, "GET", "/t/acme/tasks", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", "/t/acme/payouts/pending", Some(&tb), None).await.0, StatusCode::FORBIDDEN);
    let (s, _) = call(&app, "POST", "/t/acme/intake/tasks", Some(&tb), Some(json!({ "objective": "not my company" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "GET", "/t/acme/payouts/pending", Some(&ta), None).await.0, StatusCode::OK);

    // Integration keys: stored encrypted, secrets never returned.
    let (s, b) = call(&app, "PUT", "/platform/companies/acme/integrations/DOCUSEAL_API_KEY", Some(&ta), Some(json!({ "value": "pk_live_abcdef123456" }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    call(&app, "PUT", "/platform/companies/acme/integrations/DOCUSEAL_URL", Some(&ta), Some(json!({ "value": "https://docuseal.acme.example" }))).await;
    // A company can't aim the platform at its own network.
    for bad in ["http://docuseal.acme.example", "https://127.0.0.1/x", "https://169.254.169.254/latest/meta-data/", "https://[::1]:8200", "https://user:pw@docuseal.acme.example", "file:///etc/passwd"] {
        for name in ["DOCUSEAL_URL", "CRM_URL", "UPWORK_MCP_URL"] {
            let (s, b) = call(&app, "PUT", &format!("/platform/companies/acme/integrations/{name}"), Some(&ta), Some(json!({ "value": bad }))).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{name}={bad}: {b}");
        }
    }
    let (s, _) = call(&app, "PUT", "/platform/companies/acme/integrations/DATABASE_URL", Some(&ta), Some(json!({ "value": "x" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "only catalog settings can be stored");
    let (s, _) = call(&app, "PUT", "/platform/companies/acme/integrations/DOCUSEAL_API_KEY", Some(&tb), Some(json!({ "value": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, b) = call(&app, "GET", "/platform/companies/acme/integrations", Some(&ta), None).await;
    let values = b["values"].as_array().unwrap();
    let key = values.iter().find(|v| v["name"] == "DOCUSEAL_API_KEY").unwrap();
    assert_eq!((key["value"].clone(), key["hint"].as_str()), (Value::Null, Some("••••3456")));
    let url = values.iter().find(|v| v["name"] == "DOCUSEAL_URL").unwrap();
    assert_eq!(url["value"], "https://docuseal.acme.example");
    let raw: (Vec<u8>,) = sqlx::query_as("SELECT ciphertext FROM platform.secrets WHERE name = 'DOCUSEAL_API_KEY'").fetch_one(&pool).await.unwrap();
    assert!(!String::from_utf8_lossy(&raw.0).contains("abcdef"), "stored encrypted");
    let acme = p.by_slug("acme").await.unwrap();
    assert_eq!(p.config(acme.id).await.unwrap().get("DOCUSEAL_API_KEY").as_deref(), Some("pk_live_abcdef123456"));
    let (audit,): (i64,) = sqlx::query_as("SELECT count(*) FROM platform.audit_log WHERE details::text LIKE '%abcdef%'").fetch_one(&pool).await.unwrap();
    assert_eq!(audit, 0, "values never reach the audit log");

    // Invitations: Bob is invited by email and joins when he signs in.
    let (s, b) = call(&app, "POST", "/platform/companies/acme/invites", Some(&ta), Some(json!({ "email": "BOB@globex.example" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, _) = call(&app, "POST", "/platform/companies/acme/invites", Some(&tb), Some(json!({ "email": "x@y.example" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "only members (and only the owner) can invite");
    let (_, b) = call(&app, "GET", "/platform/me", Some(&tb), None).await;
    assert_eq!(b["companies"][0]["role"], "admin");
    let (s, _) = call(&app, "GET", "/t/acme/admin/cap", Some(&tb), None).await;
    assert_eq!(s, StatusCode::OK, "now an admin in Acme's database");
    let (s, _) = call(&app, "POST", "/platform/companies/acme/invites", Some(&tb), Some(json!({ "email": "x@y.example" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "admins can't invite; only the owner");
    let (s, _) = call(&app, "DELETE", &format!("/platform/companies/acme/members/{}", bob.user_id), Some(&ta), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&app, "GET", "/t/acme/admin/cap", Some(&tb), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "removed members lose access");
    cleanup(&p).await;
}

/// Test transaction hashes (valid base58, 32 bytes).
fn h(name: &str) -> String {
    let mut b = [0u8; 32];
    for (i, c) in name.bytes().enumerate() {
        b[i] = c;
    }
    bs58::encode(b).into_string()
}

/// A NEAR node that knows a fixed set of transactions.
#[derive(Default)]
struct FakeChain(Mutex<Vec<(String, Transfer)>>);

#[async_trait]
impl PaymentLookup for FakeChain {
    async fn transfer(&self, tx_hash: &str, sender: &str) -> anyhow::Result<Option<Transfer>> {
        Ok(self.0.lock().unwrap().iter().find(|(h, t)| h == tx_hash && t.signer_id == sender).map(|(_, t)| t.clone()))
    }
}

#[sqlx::test(migrations = false)]
async fn subscription_paid_in_near(pool: PgPool) {
    let p = platform(&pool).await;
    let ada = user("ada@acme.example");
    let now = Utc::now();
    let acme = p.create_tenant(&ada, "Acme Labs", "acme", now).await.unwrap();
    let price = FixedPriceFeed(PriceQuote { usd_per_near: dec!(3.5), fetched_at: now });

    let inv = p.create_invoice(acme.id, &ada, 2, &price, now).await.unwrap();
    assert_eq!(inv.amount_usd, dec!(98));
    // 98 USD at 3.5 USD/NEAR is 28 NEAR, plus the digits that identify this
    // invoice (decimal places 3 to 8).
    let needed: u128 = inv.amount_yocto.to_string().parse().unwrap();
    let near = 10u128.pow(24);
    assert!(needed > 28 * near && needed < 28 * near + near / 100, "{}", inv.amount_near());
    assert_eq!(needed % 10u128.pow(16), 0, "eight decimal places: {}", inv.amount_near());
    assert_eq!(inv.pay_to, "axom-billing.testnet");
    let outsider = user("eve@x.example");
    assert!(p.create_invoice(acme.id, &outsider, 1, &price, now).await.is_err());

    let chain = FakeChain::default();
    let tx = |hash: &str, signer: &str, receiver: &str, deposit: u128, success: bool, at: chrono::DateTime<Utc>| {
        chain.0.lock().unwrap().push((hash.into(), Transfer { signer_id: signer.into(), receiver_id: receiver.into(), deposit_yocto: deposit, success, is_final: true, block_time: at }));
    };
    tx(&h("WRONGTO"), "ada.testnet", "someone.testnet", needed, true, now);
    tx(&h("SHORT"), "ada.testnet", "axom-billing.testnet", needed - 1, true, now);
    tx(&h("FAILED"), "ada.testnet", "axom-billing.testnet", needed, false, now);
    tx(&h("OLD"), "ada.testnet", "axom-billing.testnet", needed, true, now - Duration::days(3));
    tx(&h("OVER"), "ada.testnet", "axom-billing.testnet", needed + 5, true, now + Duration::minutes(2));
    tx(&h("ROUND"), "ada.testnet", "axom-billing.testnet", 29 * near, true, now + Duration::minutes(2));
    tx(&h("EARLY"), "ada.testnet", "axom-billing.testnet", needed, true, now - Duration::seconds(30));
    tx(&h("GOOD"), "ada.testnet", "axom-billing.testnet", needed, true, now + Duration::minutes(2));

    // Another company opens an invoice for the same plan at the same price:
    // it asks for a different amount, so it can't claim Acme's payment even
    // though the hash and the sending account are public.
    let bob = user("bob@globex.example");
    let globex = p.create_tenant(&bob, "Globex", "globex", now).await.unwrap();
    let theirs = p.create_invoice(globex.id, &bob, 2, &price, now).await.unwrap();
    assert_ne!(theirs.amount_yocto, inv.amount_yocto);
    let stolen = p.verify_payment(globex.id, &bob, theirs.id, &h("GOOD"), "ada.testnet", &chain, now + Duration::minutes(3)).await;
    assert!(matches!(stolen, Err(PaymentError::TooLittle { .. } | PaymentError::NotExact { .. })), "{stolen:?}");
    // Nor can an invoice opened after the payment, whatever its amount.
    let late = p.create_invoice(globex.id, &bob, 2, &price, now + Duration::minutes(4)).await.unwrap();
    sqlx::query("UPDATE platform.invoices SET amount_yocto = $2::numeric WHERE id = $1").bind(late.id).bind(needed.to_string()).execute(&p.db).await.unwrap();
    let stolen = p.verify_payment(globex.id, &bob, late.id, &h("GOOD"), "ada.testnet", &chain, now + Duration::minutes(5)).await;
    assert_eq!(stolen, Err(PaymentError::OutsideWindow));
    assert!(matches!(p.by_id(globex.id).await.unwrap().subscription(now), Subscription::Trial { .. }), "nothing was credited");
    // Amounts stay distinct across everything still payable.
    let mut seen = std::collections::HashSet::new();
    for i in 0..40 {
        let t = if i % 2 == 0 { globex.id } else { acme.id };
        let who = if i % 2 == 0 { &bob } else { &ada };
        // Later than the payments above, so Acme's invoice under test stays as it is.
        let other = p.create_invoice(t, who, 3, &price, now + Duration::minutes(10)).await.unwrap();
        assert!(seen.insert(other.amount_yocto), "two payable invoices ask for the same amount");
    }

    let verify = |hash: String, payer: &'static str| {
        let (p, ada, chain) = (p.clone(), ada.clone(), &chain);
        async move { p.verify_payment(acme.id, &ada, inv.id, &hash, payer, chain, now + Duration::minutes(3)).await }
    };
    assert_eq!(verify("not-a-hash".into(), "ada.testnet").await, Err(PaymentError::BadHash));
    assert_eq!(verify(h("NOPE"), "ada.testnet").await, Err(PaymentError::UnknownTx("testnet".into())));
    assert!(matches!(verify(h("WRONGTO"), "ada.testnet").await, Err(PaymentError::WrongReceiver(..))));
    assert!(matches!(verify(h("SHORT"), "ada.testnet").await, Err(PaymentError::TooLittle { .. })));
    assert!(matches!(verify(h("OVER"), "ada.testnet").await, Err(PaymentError::NotExact { .. })), "more than the amount is not this invoice's payment");
    assert!(matches!(verify(h("ROUND"), "ada.testnet").await, Err(PaymentError::NotExact { .. })));
    assert_eq!(verify(h("EARLY"), "ada.testnet").await, Err(PaymentError::OutsideWindow), "made before the invoice was opened");
    assert_eq!(verify(h("FAILED"), "ada.testnet").await, Err(PaymentError::Failed));
    assert_eq!(verify(h("OLD"), "ada.testnet").await, Err(PaymentError::OutsideWindow));
    assert_eq!(verify(h("GOOD"), "bob.testnet").await, Err(PaymentError::UnknownTx("testnet".into())), "the sender must match");

    let paid_until = verify(h("GOOD"), "ada.testnet").await.unwrap();
    // Paying during the trial keeps the trial days: 14 + 60 days from sign-up.
    let expected = acme.trial_ends_at + Duration::days(60);
    assert!((paid_until - expected).num_seconds().abs() < 2, "{paid_until} vs {expected}");
    assert!(matches!(p.by_id(acme.id).await.unwrap().subscription(now), Subscription::Active { .. }));
    // The same transaction can't pay twice.
    assert!(matches!(verify(h("GOOD"), "ada.testnet").await, Err(PaymentError::NotOpen(_))));
    let inv2 = p.create_invoice(acme.id, &ada, 1, &price, now).await.unwrap();
    assert_eq!(p.verify_payment(acme.id, &ada, inv2.id, &h("GOOD"), "ada.testnet", &chain, now).await, Err(PaymentError::AlreadyUsed));
    cleanup(&p).await;
}

struct FakeKeys(Mutex<Vec<String>>);

#[async_trait]
impl axom_platform::treasury::TreasuryKeys for FakeKeys {
    async fn create(&self, key_name: &str) -> anyhow::Result<axom_core::near::PublicKey> {
        self.0.lock().unwrap().push(key_name.to_string());
        Ok(axom_core::near::PublicKey([7u8; 32]))
    }
}

#[sqlx::test(migrations = false)]
async fn treasury_and_plan_for_a_new_company(pool: PgPool) {
    let p = platform(&pool).await;
    let keys = Arc::new(FakeKeys(Mutex::new(vec![])));
    let planner = axom_planner::PlannerConfig::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/planner.toml")).unwrap();
    let st = Arc::try_unwrap(state(p.clone(), Arc::new(NoPayments))).ok().unwrap().with_planner(Arc::new(planner)).with_treasury(keys.clone());
    let app = http::router(Arc::new(st));
    let (ada, bob) = (user("ada@acme.example"), user("bob@acme.example"));
    let ta = jwt(&ada);
    call(&app, "POST", "/platform/companies", Some(&ta), Some(json!({ "name": "Acme Labs", "slug": "acme" }))).await;
    let acme = p.by_slug("acme").await.unwrap();

    // Only the owner can create the treasury; it is created once.
    p.invite(acme.id, &ada, &bob.email).await.unwrap();
    p.memberships(&bob).await.unwrap();
    let (s, _) = call(&app, "POST", "/platform/companies/acme/treasury", Some(&jwt(&bob)), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, b) = call(&app, "POST", "/platform/companies/acme/treasury", Some(&ta), None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["treasury_account"], "07".repeat(32));
    call(&app, "POST", "/platform/companies/acme/treasury", Some(&ta), None).await;
    assert_eq!(keys.0.lock().unwrap().len(), 1, "idempotent");
    assert!(keys.0.lock().unwrap()[0].starts_with("axom-treasury-"));
    let (_, b) = call(&app, "GET", "/platform/companies/acme", Some(&ta), None).await;
    assert_eq!((b["company"]["treasury_account"].clone(), b["has_plan"].clone()), (json!("07".repeat(32)), json!(false)));

    // The first plan builds the org chart; a second one is refused.
    let plan = json!({ "goal": "Launch our product", "budget_usd": "400", "deadline": (Utc::now() + Duration::days(90)).date_naive(),
        "objectives": [{ "title": "Run the launch marketing campaign", "description": "Social content" }, { "title": "Start outbound sales" }] });
    let (s, b) = call(&app, "POST", "/platform/companies/acme/plan", Some(&ta), Some(plan.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert!(b["departments"].as_u64().unwrap() >= 1, "{b}");
    let (s, _) = call(&app, "POST", "/platform/companies/acme/plan", Some(&ta), Some(plan)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Company data is members-only on the platform.
    let (s, _) = call(&app, "GET", "/t/acme/org-chart", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(&app, "GET", "/t/acme/notifications", Some(&jwt(&user("eve@x.example"))), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, org) = call(&app, "GET", "/t/acme/org-chart", Some(&ta), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!org.is_null());
    let (s, _) = call(&app, "POST", "/platform/companies/acme/plan", Some(&ta), Some(json!({ "goal": "", "budget_usd": "0", "deadline": "2000-01-01", "objectives": [] }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    cleanup(&p).await;
}

#[sqlx::test(migrations = false)]
async fn invitation_emails_and_subscription_reminders(pool: PgPool) {
    use axom_platform::mail::PlatformMail;
    let p = platform(&pool).await;
    let mock = Arc::new(axom_tools::mock::MockTools::new());
    let mail = Arc::new(PlatformMail { mailer: mock.clone(), policy: Arc::new(policy()), dashboard_url: "https://app.axom.example/".into() });
    let st = Arc::try_unwrap(state(p.clone(), Arc::new(NoPayments))).ok().unwrap().with_mail(mail.clone());
    let app = http::router(Arc::new(st));
    let ada = user("ada@acme.example");
    let ta = jwt(&ada);
    call(&app, "POST", "/platform/companies", Some(&ta), Some(json!({ "name": "Acme Labs", "slug": "acme" }))).await;

    // The invitee gets an email naming the company and the address to use; no secret link.
    let (s, b) = call(&app, "POST", "/platform/companies/acme/invites", Some(&ta), Some(json!({ "email": "bob@globex.example" }))).await;
    assert_eq!((s, b["emailed"].clone()), (StatusCode::CREATED, json!(true)), "{b}");
    let sent = mock.calls_to("smtp");
    assert_eq!(sent.len(), 1);
    let email = &sent[0].args["email"];
    assert_eq!(email["to"][0], "bob@globex.example");
    assert!(email["subject"].as_str().unwrap().contains("Acme Labs"));
    assert!(email["body"].as_str().unwrap().contains("https://app.axom.example"));

    // Reminders: nothing while the trial has a while to run.
    let now = Utc::now();
    assert_eq!(mail.send_reminders(&p, now).await.unwrap(), 0);
    // Two days left: one reminder to the owner, once.
    sqlx::query("UPDATE platform.tenants SET trial_ends_at = $1").bind(now + Duration::days(2)).execute(&pool).await.unwrap();
    assert_eq!(mail.send_reminders(&p, now).await.unwrap(), 1);
    assert_eq!(mail.send_reminders(&p, now).await.unwrap(), 0, "sent once per period");
    let last = mock.calls_to("smtp").pop().unwrap();
    assert_eq!(last.args["email"]["to"][0], "ada@acme.example");
    assert!(last.args["email"]["body"].as_str().unwrap().contains("https://app.axom.example/c/acme/billing"));
    // Ended: a "paused" email, once.
    sqlx::query("UPDATE platform.tenants SET trial_ends_at = $1").bind(now - Duration::hours(1)).execute(&pool).await.unwrap();
    assert_eq!(mail.send_reminders(&p, now).await.unwrap(), 1);
    assert_eq!(mail.send_reminders(&p, now).await.unwrap(), 0);
    assert!(mock.calls_to("smtp").pop().unwrap().args["email"]["subject"].as_str().unwrap().contains("paused"));
    // Paid with a few days left: a renewal reminder.
    sqlx::query("UPDATE platform.tenants SET paid_until = $1").bind(now + Duration::days(1)).execute(&pool).await.unwrap();
    assert_eq!(mail.send_reminders(&p, now).await.unwrap(), 1);
    assert!(mock.calls_to("smtp").pop().unwrap().args["email"]["subject"].as_str().unwrap().contains("subscription"));
    cleanup(&p).await;
}

#[sqlx::test(migrations = false)]
async fn operator_sees_companies_and_suspends(pool: PgPool) {
    use axom_platform::operator::Operators;
    let p = platform(&pool).await;
    let st = Arc::try_unwrap(state(p.clone(), Arc::new(NoPayments))).ok().unwrap().with_operators(Operators::new(["Ops@ForeMan.example"]));
    let app = http::router(Arc::new(st));
    let (ada, ops) = (user("ada@acme.example"), user("ops@foreman.example"));
    let (ta, to) = (jwt(&ada), jwt(&ops));
    call(&app, "POST", "/platform/companies", Some(&ta), Some(json!({ "name": "Acme Labs", "slug": "acme" }))).await;

    // Only operators (with two-factor) see the overview.
    let (s, _) = call(&app, "GET", "/platform/operator/companies", Some(&ta), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let no_mfa = jwt(&AdminSession { mfa_verified: false, ..ops.clone() });
    let (s, _) = call(&app, "GET", "/platform/operator/companies", Some(&no_mfa), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, me) = call(&app, "GET", "/platform/me", Some(&to), None).await;
    assert_eq!(me["operator"], true);
    let (s, b) = call(&app, "GET", "/platform/operator/companies", Some(&to), None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!((b["totals"]["companies"].as_u64(), b["totals"]["trial"].as_u64()), (Some(1), Some(1)));
    assert_eq!((b["companies"][0]["slug"].as_str(), b["companies"][0]["members"].as_i64()), (Some("acme"), Some(1)));

    // Suspending needs a reason; a suspended company's API stops, and it
    // drops out of the companies whose jobs and payouts run.
    let (s, _) = call(&app, "POST", "/platform/operator/companies/acme/suspend", Some(&to), Some(json!({}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(&app, "POST", "/platform/operator/companies/acme/suspend", Some(&ta), Some(json!({ "reason": "x" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "owners can't suspend");
    let (s, _) = call(&app, "POST", "/platform/operator/companies/acme/suspend", Some(&to), Some(json!({ "reason": "chargeback investigation" }))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = call(&app, "GET", "/t/acme/admin/cap", Some(&ta), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    let (s, _) = call(&app, "GET", "/t/acme/jobs", None, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(p.active_tenants().await.unwrap().is_empty());
    let (s, _) = call(&app, "GET", "/platform/companies/acme/billing", Some(&ta), None).await;
    assert_eq!(s, StatusCode::OK, "members can still see billing");
    let (audit,): (i64,) = sqlx::query_as("SELECT count(*) FROM platform.audit_log WHERE action = 'company.suspend' AND actor = 'operator:ops@foreman.example'").fetch_one(&pool).await.unwrap();
    assert_eq!(audit, 1);

    let (s, _) = call(&app, "POST", "/platform/operator/companies/acme/resume", Some(&to), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&app, "GET", "/t/acme/admin/cap", Some(&ta), None).await;
    assert_eq!(s, StatusCode::OK);
    cleanup(&p).await;
}

#[sqlx::test(migrations = false)]
async fn existing_company_database_is_imported(pool: PgPool) {
    use axom_platform::tenants::Import;
    let p = platform(&pool).await;
    // An existing single-company ForeMan database with data and two admins.
    let old = format!("axom_t_import_{}", &Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query(&format!("CREATE DATABASE \"{old}\"")).execute(&pool).await.unwrap();
    let old_pool = sqlx::postgres::PgPoolOptions::new().connect_with((*pool.connect_options()).clone().database(&old).options([("search_path", "public")])).await.unwrap();
    axom_core::MIGRATOR.run(&old_pool).await.unwrap();
    axom_hr::cap::seed_cap(&old_pool, &policy()).await.unwrap();
    sqlx::query("UPDATE axom.cap_settings SET cap_usd = 321, updated_by = 'old' WHERE id = 1").execute(&old_pool).await.unwrap();
    let (ada, bob) = (user("ada@acme.example"), user("bob@acme.example"));
    sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,$2,'Bob')").bind(bob.user_id).bind(&bob.email).execute(&old_pool).await.unwrap();
    old_pool.close().await;

    fn imp<'a>(db: &'a str, slug: &'a str, owner: Uuid) -> Import<'a> {
        Import { database: db, slug, name: "Acme Labs", owner_user_id: owner, owner_email: "Ada@Acme.example" }
    }
    let o = ada.user_id;
    assert!(matches!(p.import_tenant(&imp("no_such_db", "acme", o), Utc::now()).await, Err(TenantError::Invalid(_))));
    assert!(matches!(p.import_tenant(&imp("bad;name", "acme", o), Utc::now()).await, Err(TenantError::Invalid(_))));
    let own: (String,) = sqlx::query_as("SELECT current_database()").fetch_one(&pool).await.unwrap();
    assert!(matches!(p.import_tenant(&imp(&own.0, "acme", o), Utc::now()).await, Err(TenantError::Invalid(_))), "never the platform's own database");

    let t = p.import_tenant(&imp(&old, "acme", o), Utc::now()).await.unwrap();
    assert_eq!((t.slug.as_str(), t.status.as_str(), t.owner_email.as_str()), ("acme", "active", "ada@acme.example"));
    assert!(matches!(p.import_tenant(&imp(&old, "acme-2", o), Utc::now()).await, Err(TenantError::Invalid(_))), "a database belongs to one company");

    // Data kept; the owner and the old admin can sign in to it.
    let app = http::router(state(p.clone(), Arc::new(NoPayments)));
    let (s, b) = call(&app, "GET", "/t/acme/admin/cap", Some(&jwt(&ada)), None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["cap_usd"].as_str().map(|v| v.parse::<f64>().unwrap()), Some(321.0));
    let (_, me) = call(&app, "GET", "/platform/me", Some(&jwt(&bob)), None).await;
    assert_eq!(me["companies"][0]["role"], "admin");
    cleanup(&p).await;
}
