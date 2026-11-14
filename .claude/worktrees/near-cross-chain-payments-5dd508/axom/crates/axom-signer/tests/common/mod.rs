//! Shared fixtures for signer tests.
#![allow(dead_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use axom_core::authz::AuthorizationKey;
use axom_core::db;
use axom_core::domain::{PayUnit, Worker, WorkerKind};
use axom_core::near::PublicKey;
use axom_core::state::WorkerStatus;
use axom_core::Uuid;
use axom_hr::gates;
use axom_hr::notify::NotifyConfig;
use axom_hr::payroll::{self, PayrollOutcome, PayrollRun};
use axom_hr::{cap, PolicyConfig, PriceQuote};
use axom_signer::openbao::{TransitError, TransitSigner};
use axom_signer::SignerConfig;
use axom_tools::near_rpc::MockNear;
use axom_tools::TaxFormKind;
use chrono::{DateTime, Duration, TimeZone, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::PgPool;

pub fn policy() -> PolicyConfig {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/policy.toml");
    PolicyConfig::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
}

pub fn period() -> (DateTime<Utc>, DateTime<Utc>) {
    let end = Utc::now().date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    (end - Duration::days(7), end)
}

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

pub fn quote() -> Option<PriceQuote> {
    Some(PriceQuote { usd_per_near: dec!(4), fetched_at: now() - Duration::minutes(1) })
}

/// Transit signer backed by a local key (stands in for OpenBao in unit tests).
pub struct LocalTransit {
    pub key: SigningKey,
    pub sealed: AtomicBool,
}

impl LocalTransit {
    pub fn new() -> Self {
        Self { key: SigningKey::generate(&mut rand::rngs::OsRng), sealed: AtomicBool::new(false) }
    }
    /// A fixed key (live tests reuse one throwaway treasury across runs).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { key: SigningKey::from_bytes(&seed), sealed: AtomicBool::new(false) }
    }
    pub fn public(&self) -> PublicKey {
        PublicKey(self.key.verifying_key().to_bytes())
    }
    pub fn seal(&self, s: bool) {
        self.sealed.store(s, Ordering::SeqCst);
    }
}

#[async_trait]
impl TransitSigner for LocalTransit {
    async fn is_sealed(&self) -> Result<bool, TransitError> {
        Ok(self.sealed.load(Ordering::SeqCst))
    }
    async fn public_key(&self) -> Result<PublicKey, TransitError> {
        if self.sealed.load(Ordering::SeqCst) {
            return Err(TransitError::Sealed);
        }
        Ok(self.public())
    }
    async fn sign(&self, message: &[u8]) -> Result<[u8; 64], TransitError> {
        if self.sealed.load(Ordering::SeqCst) {
            return Err(TransitError::Sealed);
        }
        Ok(self.key.sign(message).to_bytes())
    }
}

pub struct Fixture {
    pub policy: PolicyConfig,
    pub attestor: AuthorizationKey,
    pub approver: AuthorizationKey,
    pub cfg: SignerConfig,
    pub notify: NotifyConfig,
    pub near: MockNear,
}

impl Fixture {
    pub async fn new(pool: &PgPool, treasury: PublicKey) -> Self {
        let policy = policy();
        cap::seed_cap(pool, &policy).await.unwrap();
        let attestor = AuthorizationKey::generate();
        let approver = AuthorizationKey::generate();
        let cfg = SignerConfig {
            network: "testnet".into(),
            attestation_key: attestor.public_key(),
            approval_key: approver.public_key(),
            max_attempts: 5,
            base_backoff: Duration::seconds(30),
        };
        let near = MockNear::new();
        near.add_access_key(&treasury.implicit_account(), treasury, 100);
        let notify = NotifyConfig { owner_email: "owner@example.com".into(), dashboard_url: "https://admin.example.com".into() };
        Self { policy, attestor, approver, cfg, notify, near }
    }

    /// A worker with a verified address, a tax form, and `tasks` completed
    /// tasks at `rate` USD each in the pay period. Returns (worker, account).
    pub async fn ready_worker(&self, pool: &PgPool, rate: Decimal, tasks: i64) -> (Uuid, String) {
        let (w, account, _) = self.ready_worker_keyed(pool, rate, tasks).await;
        (w, account)
    }

    /// Like [`Self::ready_worker`], also returning the worker's NEAR key
    /// (the worker's own; tests play the worker).
    pub async fn ready_worker_keyed(&self, pool: &PgPool, rate: Decimal, tasks: i64) -> (Uuid, String, SigningKey) {
        let w = Worker {
            id: Uuid::new_v4(),
            name: "worker".into(),
            kind: WorkerKind::HumanFreelancer,
            role: "designer".into(),
            status: WorkerStatus::Active,
            skills: Default::default(),
            tool_allowlist: Default::default(),
            pay_rate_usd: rate,
            pay_unit: PayUnit::Task,
            wallet_address: None,
            kpis: vec![],
        };
        db::insert_worker(pool, None, None, &w).await.unwrap();
        let sk = SigningKey::generate(&mut rand::rngs::OsRng);
        let pk = PublicKey(sk.verifying_key().to_bytes());
        let account = pk.implicit_account();
        let ch = gates::issue_challenge(pool, w.id, "testnet", &account, &pk.to_near_string(), now()).await.unwrap();
        let sig = hex::encode(sk.sign(ch.message.as_bytes()).to_bytes());
        gates::verify_challenge(pool, &self.near, "testnet", ch.id, &sig, now()).await.unwrap();
        gates::record_tax_form(pool, w.id, TaxFormKind::W9, Some("sub")).await.unwrap();
        for i in 0..tasks {
            sqlx::query("INSERT INTO axom.tasks (id, objective, assignee, status, completed_at, finished_at) VALUES ($1,'t',$2,'done',$3,$3)")
                .bind(Uuid::new_v4())
                .bind(w.id)
                .bind(period().0 + Duration::hours(1 + i))
                .execute(pool)
                .await
                .unwrap();
        }
        (w.id, account, sk)
    }

    /// Run payroll and return the payout id for `worker`.
    pub async fn payroll(&self, pool: &PgPool, worker: Uuid) -> Uuid {
        let run = PayrollRun { policy: &self.policy, price: quote(), fee_near: dec!(0.001), network: "testnet", attestor: &self.attestor, period: period() };
        let lines = payroll::run_payroll(pool, &run, now()).await.unwrap();
        match &lines.iter().find(|l| l.owed.worker_id == worker).unwrap().outcome {
            PayrollOutcome::Created { created, .. } => created.payout_id,
            other => panic!("unexpected {other:?}"),
        }
    }
}

pub fn ymd(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
}
