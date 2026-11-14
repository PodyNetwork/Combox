//! Treasury yield on Rhea lending, executed by the signer.
//!
//! Each tick: settle the move in flight (if any), otherwise read the
//! treasury's position, ask the deterministic planner (`axom_hr::treasury`)
//! for at most one move, run it past the policy engine, build it from the
//! fixed template (`axom_core::rhea`), check it, sign it through OpenBao, and
//! send it. The same rules as payouts apply: kill switch halts everything,
//! a sealed OpenBao pauses, signed bytes are stored before broadcast and
//! resubmitted (never re-signed) on an unknown outcome.

use axom_core::audit::{self, AuditEvent};
use axom_core::domain::Actor;
use axom_core::near::yocto_to_near_string;
use axom_core::rhea;
use axom_hr::payouts::kill_switch_on;
use axom_hr::policy::{self, Action, Context, Outcome, PriceQuote};
use axom_hr::{notify, treasury};
use axom_tools::near_rpc::{TxStatus, TxWait};
use axom_tools::rhea::TreasuryViews;
use axom_tools::ToolError;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::openbao::TransitError;
use crate::Signer;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TreasuryResult {
    /// Kill switch on: no treasury moves.
    Halted,
    /// A move is in flight and not settled yet.
    Waiting { tx_hash: String },
    /// The move in flight settled this tick.
    Settled { kind: String, status: String, tx_hash: String },
    /// Nothing to do (or the planner refused, with the reason).
    Held { reason: String },
    /// Denied by the policy engine.
    Refused { reason: String },
    /// OpenBao sealed or unreachable.
    Paused { reason: String },
    /// A move was signed and sent.
    Moved { kind: String, amount_near: String, tx_hash: String, status: String },
}

/// Resubmit the stored bytes if the move is still unknown after this long.
const RESUBMIT_AFTER_SECS: i64 = 60;

impl Signer<'_> {
    async fn treasury_audit(&self, outcome: &str, inputs: serde_json::Value, reason: &str, now: DateTime<Utc>) -> Result<Uuid, sqlx::Error> {
        let ev = AuditEvent::new(&Actor::System, "treasury.move", outcome).inputs(inputs).reasoning(reason).policy_version(self.policy.version()).at(now);
        audit::append(self.pool, &ev).await
    }

    async fn settle_move(&self, id: Uuid, status: &str, error: Option<String>, now: DateTime<Utc>) -> anyhow::Result<()> {
        sqlx::query("UPDATE axom.treasury_moves SET status = $2, error = $3, updated_at = $4 WHERE id = $1")
            .bind(id)
            .bind(status)
            .bind(&error)
            .bind(now)
            .execute(self.pool)
            .await?;
        let audit_id = self
            .treasury_audit(status, json!({ "move_id": id, "error": error }), error.as_deref().unwrap_or("settled on NEAR"), now)
            .await?;
        if status == "failed" {
            notify::enqueue(
                self.pool,
                "treasury_move_failed",
                "Treasury move to/from Rhea failed",
                json!({ "move_id": id, "error": error, "audit_link": self.notify.audit_link(audit_id) }),
                Some(audit_id),
            )
            .await?;
        }
        Ok(())
    }

    /// One tick of treasury management. `price` is the latest NEAR quote
    /// (dropped if stale).
    pub async fn manage_treasury(&self, views: &dyn TreasuryViews, price: Option<PriceQuote>, now: DateTime<Utc>) -> anyhow::Result<TreasuryResult> {
        if kill_switch_on(self.pool).await? {
            return Ok(TreasuryResult::Halted);
        }
        // Companies that never turned yield on cost no RPC calls.
        if !self.policy.treasury().yield_enabled {
            let (ever,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM axom.treasury_moves)").fetch_one(self.pool).await?;
            if !ever {
                return Ok(TreasuryResult::Held { reason: "treasury yield is off".into() });
            }
        }
        let network = self.cfg.network.clone();
        let (pk, treasury) = match self.treasury_account().await {
            Ok(t) => t,
            Err(e) => return Ok(TreasuryResult::Paused { reason: e.to_string() }),
        };

        // 1. A move in flight: settle it before planning anything new.
        type InFlight = (Uuid, String, String, String, DateTime<Utc>);
        let open: Option<InFlight> = sqlx::query_as(
            "SELECT id, kind, tx_hash, signed_tx, updated_at FROM axom.treasury_moves WHERE network = $1 AND status = 'submitted'",
        )
        .bind(&network)
        .fetch_optional(self.pool)
        .await?;
        if let Some((id, kind, hash, signed, updated)) = open {
            match self.near.tx_status(&hash, &treasury, TxWait::Probe).await {
                Ok(Some(o)) if o.status == TxStatus::Success => {
                    self.settle_move(id, "succeeded", None, now).await?;
                    return Ok(TreasuryResult::Settled { kind, status: "succeeded".into(), tx_hash: hash });
                }
                Ok(Some(o)) => {
                    self.settle_move(id, "failed", Some(format!("failed on chain: {:?}", o.status)), now).await?;
                    return Ok(TreasuryResult::Settled { kind, status: "failed".into(), tx_hash: hash });
                }
                Ok(None) | Err(ToolError::Retryable(_)) if now - updated >= Duration::seconds(RESUBMIT_AFTER_SECS) => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(&signed)?;
                    sqlx::query("UPDATE axom.treasury_moves SET updated_at = $2 WHERE id = $1").bind(id).bind(now).execute(self.pool).await?;
                    match self.near.send_tx(&bytes).await {
                        Ok(o) if o.status == TxStatus::Success => self.settle_move(id, "succeeded", None, now).await?,
                        Ok(o) => self.settle_move(id, "failed", Some(format!("failed on chain: {:?}", o.status)), now).await?,
                        Err(ToolError::Permanent(m)) if m.contains("Expired") || m.contains("InvalidNonce") => {
                            // Never landed, and never can: the planner re-reads the chain next tick.
                            self.settle_move(id, "failed", Some(format!("did not land: {m}")), now).await?
                        }
                        Err(_) => {}
                    }
                    return Ok(TreasuryResult::Waiting { tx_hash: hash });
                }
                _ => return Ok(TreasuryResult::Waiting { tx_hash: hash }),
            }
        }

        // 2. Plan from the chain and the payroll books.
        let position = match views.position(&network, &treasury).await {
            Ok(p) => p,
            Err(e) => return Ok(TreasuryResult::Held { reason: format!("could not read the treasury position: {e}") }),
        };
        let obligations = treasury::obligations(self.pool, now).await?;
        let price = treasury::fresh(self.policy, price, now);
        let plan = treasury::plan(self.policy.treasury(), &position, obligations, price);
        let plan_json = serde_json::to_value(&plan)?;
        sqlx::query(
            "INSERT INTO axom.treasury_status (id, network, account, plan, checked_at) VALUES (1, $1, $2, $3, $4)
             ON CONFLICT (id) DO UPDATE SET network = EXCLUDED.network, account = EXCLUDED.account, plan = EXCLUDED.plan, checked_at = EXCLUDED.checked_at",
        )
        .bind(&network)
        .bind(&treasury)
        .bind(&plan_json)
        .bind(now)
        .execute(self.pool)
        .await?;
        let Some(m) = plan.action else { return Ok(TreasuryResult::Held { reason: plan.reason }) };

        // 3. Policy engine: only the planner (System), supply only with yield enabled, nothing under the kill switch.
        let amount_near: Decimal = yocto_to_near_string(m.amount_yocto()).parse().unwrap_or_default();
        let mut ctx = Context::new(Actor::System, now);
        ctx.kill_switch = kill_switch_on(self.pool).await?;
        let d = policy::evaluate(self.policy, &Action::TreasuryMove { kind: m.kind().into(), amount_near }, &ctx);
        if d.outcome != Outcome::Allow {
            self.treasury_audit("denied", json!({ "move": m, "plan": plan_json }), &d.reason, now).await?;
            return Ok(TreasuryResult::Refused { reason: d.reason });
        }

        // 4. Build from the template, check, sign, store, send.
        match self.transit.is_sealed().await {
            Ok(false) => {}
            Ok(true) => return Ok(TreasuryResult::Paused { reason: TransitError::Sealed.to_string() }),
            Err(e) => return Ok(TreasuryResult::Paused { reason: e.to_string() }),
        }
        let access = match self.near.view_access_key(&treasury, &pk).await {
            Ok(Some(a)) => a,
            Ok(None) => return Ok(TreasuryResult::Held { reason: format!("treasury {treasury} has no access key for the OpenBao key (unfunded?)") }),
            Err(e) => return Ok(TreasuryResult::Held { reason: format!("NEAR RPC: {e}") }),
        };
        // The chain reports the key's nonce a couple of blocks late: also count nonces signed here.
        let nonce = access.nonce.max(crate::last_nonce_used(self.pool, &network).await?) + 1;
        let tx = rhea::transaction(&m, &network, treasury.clone(), pk, nonce, access.block_hash);
        if let Err(e) = rhea::check_tx(&tx, &m, &network, &treasury) {
            self.treasury_audit("refused", json!({ "move": m }), &e, now).await?;
            return Ok(TreasuryResult::Refused { reason: e });
        }
        let hash = tx.signing_hash();
        let sig = match self.transit.sign(&hash).await {
            Ok(s) => s,
            Err(e) => return Ok(TreasuryResult::Paused { reason: e.to_string() }),
        };
        if pk.verify(&hash, &sig).is_err() {
            return Ok(TreasuryResult::Refused { reason: "OpenBao signature does not verify against the treasury key".into() });
        }
        let bytes = tx.signed_borsh(&sig);
        let tx_hash = bs58::encode(hash).into_string();
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO axom.treasury_moves (id, network, kind, amount_yocto, contract, reason, plan, policy_version, tx_hash, signed_tx, tx_nonce, status, created_at, updated_at)
             VALUES ($1,$2,$3,$4::numeric,$5,$6,$7,$8,$9,$10,$12,'submitted',$11,$11)",
        )
        .bind(id)
        .bind(&network)
        .bind(m.kind())
        .bind(m.amount_yocto().to_string())
        .bind(m.receiver(&network))
        .bind(&plan.reason)
        .bind(&plan_json)
        .bind(&d.policy_version)
        .bind(&tx_hash)
        .bind(base64::engine::general_purpose::STANDARD.encode(&bytes))
        .bind(now)
        .bind(tx.nonce as i64)
        .execute(self.pool)
        .await?;
        self.treasury_audit(
            "signed",
            json!({ "move_id": id, "move": m, "contract": m.receiver(&network), "tx_hash": tx_hash, "nonce": tx.nonce, "plan": plan_json }),
            &plan.reason,
            now,
        )
        .await?;
        let status = match self.near.send_tx(&bytes).await {
            Ok(o) if o.status == TxStatus::Success => {
                self.settle_move(id, "succeeded", None, now).await?;
                "succeeded"
            }
            Ok(o) => {
                self.settle_move(id, "failed", Some(format!("failed on chain: {:?}", o.status)), now).await?;
                "failed"
            }
            Err(ToolError::Permanent(e)) => {
                self.settle_move(id, "failed", Some(e), now).await?;
                "failed"
            }
            // Unknown outcome: stays submitted; probed and resubmitted next ticks.
            Err(_) => "submitted",
        };
        Ok(TreasuryResult::Moved { kind: m.kind().into(), amount_near: yocto_to_near_string(m.amount_yocto()), tx_hash, status: status.into() })
    }
}
