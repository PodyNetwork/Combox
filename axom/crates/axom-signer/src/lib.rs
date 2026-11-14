//! axom-signer: the only service that calls OpenBao to sign NEAR payouts.
//!
//! Before every sign request it re-checks the payout itself (OpenBao's
//! controls do not replace ours): a valid signed authorization (automatic
//! attestation or human approval), the recipient is the worker's verified
//! address, the amount matches the authorization, the tax form is on file,
//! the cap reservation is open, and the kill switch is off. Automatic
//! attestations are re-evaluated against the policy engine.
//!
//! Cross-chain payouts (NEAR Intents): the authorization also names the
//! destination chain, asset, and address, which must still be the worker's
//! active destination signed with their verified NEAR key. The signer then
//! gets a 1Click quote, checks it against the authorization (recipient,
//! assets, exact input amount, USD output within the policy's shortfall
//! bound, a NEAR deposit address), and signs one transaction to the wNEAR
//! contract that wraps the amount and transfers it to the deposit address.
//! The payout is finalized only when 1Click reports the destination-chain
//! delivery; a refund lands back in the treasury as wNEAR.
//!
//! Failure modes: a sealed OpenBao pauses payouts (nothing is marked sent);
//! network errors retry with exponential backoff and raise an owner alert
//! after the final attempt; a retry after an unknown outcome resubmits the
//! exact same signed transaction so a transfer can never land twice.

pub mod openbao;
pub mod treasury;

use axom_core::audit::{self, AuditEvent};
use axom_core::authz::{self, AuthorizationKind, PayoutAuthorization, SignedAuthorization};
use axom_core::domain::Actor;
use axom_core::intents::{self, Destination};
use axom_core::near::{self as near_core, PublicKey, Transaction};
use axom_core::state::PayoutStatus;
use axom_hr::payouts::{self, kill_switch_on};
use axom_hr::policy::{self, Action, Context, Outcome, PolicyConfig, PriceQuote};
use axom_hr::{destinations, gates, notify};
use axom_tools::intents::{ExecutionStatus, IntentsApi, QuoteRequest};
use axom_tools::near_rpc::{NearRpc, TxStatus, TxWait};
use axom_tools::ToolError;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::openbao::{TransitError, TransitSigner};

#[derive(Debug, Clone)]
pub struct SignerConfig {
    pub network: String,
    /// Verifies automatic attestations (key held by axom-hr).
    pub attestation_key: PublicKey,
    /// Verifies human approvals (key held by axom-api).
    pub approval_key: PublicKey,
    pub max_attempts: i32,
    pub base_backoff: Duration,
}

impl SignerConfig {
    /// `testnet` unless `NEAR_NETWORK=mainnet` exactly.
    pub fn network_from_env() -> String {
        match std::env::var("NEAR_NETWORK").as_deref() {
            Ok("mainnet") => "mainnet".into(),
            _ => "testnet".into(),
        }
    }
}

pub struct Signer<'a> {
    pub pool: &'a PgPool,
    pub policy: &'a PolicyConfig,
    pub cfg: &'a SignerConfig,
    pub transit: &'a dyn TransitSigner,
    pub near: &'a dyn NearRpc,
    pub notify: &'a notify::NotifyConfig,
    /// NEAR Intents (1Click on mainnet, simulated on testnet). Without it,
    /// cross-chain payouts are refused and NEAR payouts are unaffected.
    pub intents: Option<&'a dyn IntentsApi>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SendResult {
    Sent { tx_hash: String },
    /// OpenBao sealed or unavailable; nothing sent, nothing counted.
    Paused { reason: String },
    /// Authorization or safety check failed; nothing signed.
    Rejected { reason: String },
    /// Send failed; will retry at `next_attempt_at` unless `final_attempt`.
    Failed { reason: String, attempt: i32, next_attempt_at: Option<DateTime<Utc>>, final_attempt: bool },
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PayoutRow {
    id: Uuid,
    worker_id: Option<Uuid>,
    recipient: String,
    amount_usd: Decimal,
    amount_yocto: Option<String>,
    network: String,
    status: String,
    attempts: i32,
    tx_hash: Option<String>,
    signed_tx: Option<String>,
    created_at: DateTime<Utc>,
    usd_per_near: Option<Decimal>,
    price_fetched_at: Option<DateTime<Utc>>,
    fee_usd: Decimal,
    destination_id: Option<Uuid>,
    dest_chain: Option<String>,
    dest_asset: Option<String>,
    dest_address: Option<String>,
    last_error: Option<String>,
}

impl PayoutRow {
    fn destination(&self) -> Option<Destination> {
        match (&self.dest_chain, &self.dest_asset, &self.dest_address) {
            (Some(chain), Some(asset_id), Some(address)) => Some(Destination { chain: chain.clone(), asset_id: asset_id.clone(), address: address.clone() }),
            _ => None,
        }
    }
}

async fn load_payout(pool: &PgPool, id: Uuid) -> Result<Option<PayoutRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, worker_id, recipient, amount_usd, amount_yocto::text AS amount_yocto, network, status, attempts, tx_hash, signed_tx,
                created_at, usd_per_near, price_fetched_at, fee_usd,
                destination_id, dest_chain, dest_asset, dest_address, last_error
         FROM axom.payouts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// The memo on the wNEAR transfer into an intent; ties the deposit to the payout.
fn deposit_memo(payout_id: Uuid) -> String {
    format!("axom payout {payout_id}")
}

/// Why a 1Click quote was refused.
enum QuoteRefusal {
    /// Retry later (price moved, 1Click unavailable).
    Retry(String),
    /// Do not retry (the quote contradicts the authorization).
    Refuse(String),
}

/// The highest nonce this signer has signed with on `network`, across
/// payouts and treasury moves (they share the treasury key).
pub(crate) async fn last_nonce_used(pool: &PgPool, network: &str) -> Result<u64, sqlx::Error> {
    let (last,): (Option<i64>,) = sqlx::query_as(
        "SELECT max(n) FROM (SELECT max(tx_nonce) AS n FROM axom.payouts WHERE network = $1
                             UNION ALL SELECT max(tx_nonce) FROM axom.treasury_moves WHERE network = $1) t",
    )
    .bind(network)
    .fetch_one(pool)
    .await?;
    Ok(last.unwrap_or(0).max(0) as u64)
}

impl Signer<'_> {
    /// The treasury's NEAR implicit account, derived from the OpenBao key.
    pub async fn treasury_account(&self) -> Result<(PublicKey, String), TransitError> {
        let pk = self.transit.public_key().await?;
        Ok((pk, pk.implicit_account()))
    }

    async fn audit(&self, action: &str, outcome: &str, payout_id: Uuid, inputs: serde_json::Value, reason: &str, now: DateTime<Utc>) -> Result<Uuid, sqlx::Error> {
        let ev = AuditEvent::new(&Actor::agent("axom-signer"), action, outcome)
            .inputs(json!({ "payout_id": payout_id, "details": inputs }))
            .reasoning(reason)
            .policy_version(self.policy.version())
            .at(now);
        audit::append(self.pool, &ev).await
    }

    /// Every check the signer makes before asking OpenBao for a signature.
    async fn verify(&self, p: &PayoutRow, now: DateTime<Utc>) -> anyhow::Result<Result<SignedAuthorization, String>> {
        let fail = |m: String| Ok(Err(m));
        if kill_switch_on(self.pool).await? {
            return fail("kill switch is on".into());
        }
        if p.network != self.cfg.network {
            return fail(format!("payout is for {}, signer runs on {}", p.network, self.cfg.network));
        }
        if p.status != "approved" && p.status != "failed" {
            return fail(format!("payout status is {}", p.status));
        }
        if p.attempts >= self.cfg.max_attempts {
            return fail("retry budget exhausted".into());
        }
        let Some(yocto) = &p.amount_yocto else { return fail("payout has no NEAR amount".into()) };

        // Signed authorization.
        let row: Option<(String, serde_json::Value, String)> =
            sqlx::query_as("SELECT kind, payload, signature FROM axom.payout_authorizations WHERE payout_id = $1")
                .bind(p.id)
                .fetch_optional(self.pool)
                .await?;
        let Some((kind, payload, signature)) = row else { return fail("no authorization for this payout".into()) };
        let payload: PayoutAuthorization = match serde_json::from_value(payload) {
            Ok(v) => v,
            Err(e) => return fail(format!("malformed authorization: {e}")),
        };
        if payload.kind.as_str() != kind {
            return fail("authorization kind mismatch".into());
        }
        let signed = SignedAuthorization { payload, signature };
        let key = match signed.payload.kind {
            AuthorizationKind::AutoAttestation => &self.cfg.attestation_key,
            AuthorizationKind::HumanApproval => &self.cfg.approval_key,
        };
        if let Err(e) = authz::verify(&signed, key, now) {
            return fail(format!("authorization invalid: {e}"));
        }
        let a = &signed.payload;
        if a.payout_id != p.id {
            return fail("authorization is for a different payout".into());
        }
        if a.network != p.network {
            return fail("authorization network mismatch".into());
        }
        if a.recipient != p.recipient {
            return fail(format!("recipient {} does not match authorization {}", p.recipient, a.recipient));
        }
        if &a.amount_yocto != yocto {
            return fail(format!("amount {yocto} yocto does not match authorization {}", a.amount_yocto));
        }
        if a.amount_usd.parse::<Decimal>().ok() != Some(p.amount_usd) {
            return fail("USD amount does not match authorization".into());
        }
        if a.destination != p.destination() {
            return fail("cross-chain destination does not match authorization".into());
        }

        // Recipient is the worker's verified address; tax form on file.
        let Some(worker_id) = p.worker_id else { return fail("payout has no worker".into()) };
        if !gates::is_verified_address(self.pool, worker_id, &p.network, &p.recipient).await? {
            return fail(format!("recipient {} is not a verified address of the worker", p.recipient));
        }
        if !gates::has_tax_form(self.pool, worker_id).await? {
            return fail("no tax form on file".into());
        }
        if let Some(d) = p.destination() {
            if self.intents.is_none() {
                return fail("cross-chain payouts are not enabled on this signer (NEAR Intents client not configured)".into());
            }
            if let Err(e) = intents::validate_destination(&d) {
                return fail(format!("invalid destination: {e}"));
            }
            let Some(dest_id) = p.destination_id else { return fail("payout has no destination record".into()) };
            if !destinations::is_active(self.pool, worker_id, &p.network, dest_id, &p.recipient, &d).await? {
                return fail("destination is no longer the worker's active destination signed with their verified NEAR key".into());
            }
        }

        // Open cap reservation (reserved, not committed or released).
        let (open,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM axom.cap_ledger WHERE payout_id = $1 AND kind = 'reservation')
                AND NOT EXISTS (SELECT 1 FROM axom.cap_ledger WHERE payout_id = $1 AND kind IN ('commit','release'))",
        )
        .bind(p.id)
        .fetch_one(self.pool)
        .await?;
        if !open {
            return fail("no open cap reservation".into());
        }

        match a.kind {
            AuthorizationKind::AutoAttestation => {
                // Re-run the policy decision as of creation.
                let (window,): (Decimal,) = sqlx::query_as(
                    "SELECT COALESCE(SUM(amount_usd), 0) FROM axom.payouts
                     WHERE recipient = $1 AND id <> $2 AND created_at > $3 AND created_at <= $4
                       AND status IN ('pending_approval','approved','sent','finalized','failed')",
                )
                .bind(&p.recipient)
                .bind(p.id)
                .bind(p.created_at - self.policy.aggregation_window())
                .bind(p.created_at)
                .fetch_one(self.pool)
                .await?;
                let price = match (p.usd_per_near, p.price_fetched_at) {
                    (Some(u), Some(f)) => Some(PriceQuote { usd_per_near: u, fetched_at: f }),
                    _ => None,
                };
                let mut ctx = Context::new(Actor::agent("axom-signer"), p.created_at);
                ctx.price = price;
                ctx.recipient_window_usd = window;
                let d = policy::evaluate(
                    self.policy,
                    &Action::Payout { recipient: p.recipient.clone(), amount_usd: p.amount_usd, fee_usd: p.fee_usd },
                    &ctx,
                );
                if d.outcome != Outcome::Allow {
                    return fail(format!("automatic attestation not supported by policy: {}", d.reason));
                }
                let (approvals,): (i64,) = sqlx::query_as("SELECT count(*) FROM axom.approvals WHERE payout_id = $1").bind(p.id).fetch_one(self.pool).await?;
                if approvals > 0 {
                    return fail("payout has a human decision; automatic attestation not applicable".into());
                }
            }
            AuthorizationKind::HumanApproval => {
                let row: Option<(Uuid, String, String, bool, bool)> = sqlx::query_as(
                    "SELECT a.id, a.decision, a.admin_name, a.mfa_verified, ad.active
                     FROM axom.approvals a JOIN axom.admins ad ON ad.user_id = a.admin_id WHERE a.payout_id = $1",
                )
                .bind(p.id)
                .fetch_optional(self.pool)
                .await?;
                match row {
                    Some((id, decision, name, true, true))
                        if decision == "approve" && Some(id) == a.approval_id && Some(&name) == a.approver.as_ref() => {}
                    _ => return fail("no matching MFA-verified approval by an active admin".into()),
                }
            }
        }
        Ok(Ok(signed))
    }

    /// Attempt to send one payout.
    pub async fn send_payout(&self, payout_id: Uuid, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        let Some(p) = load_payout(self.pool, payout_id).await? else { anyhow::bail!("payout {payout_id} not found") };

        // A transaction already signed for this payout is settled first, and
        // without re-checking the authorization: it may have landed, and an
        // authorization that expired since must not hide that. Resubmitting
        // the same bytes authorizes nothing new. The kill switch still stops it.
        let stored = match (&p.tx_hash, &p.signed_tx) {
            (Some(hash), Some(b64)) if p.status == "approved" || p.status == "failed" => Some((hash.clone(), b64.clone())),
            _ => None,
        };
        let signed = if stored.is_some() {
            if kill_switch_on(self.pool).await? {
                return self.reject(&p, "kill switch is on".into(), now).await;
            }
            None
        } else {
            match self.verify(&p, now).await? {
                Ok(s) => Some(s),
                Err(reason) => return self.reject(&p, reason, now).await,
            }
        };

        // OpenBao must be unsealed. Sealed = pause; nothing is counted or sent.
        match self.transit.is_sealed().await {
            Ok(false) => {}
            Ok(true) => return self.pause(&p, TransitError::Sealed.to_string(), now).await,
            Err(e) => return self.pause(&p, e.to_string(), now).await,
        }
        let (pk, treasury) = match self.treasury_account().await {
            Ok(t) => t,
            // Sealed, unreachable, or the signer's token refused (expired or
            // revoked): none of that is about this payout, so nothing is counted.
            Err(e @ (TransitError::Sealed | TransitError::Unavailable(_) | TransitError::Forbidden)) => return self.pause(&p, e.to_string(), now).await,
            Err(e) => return self.fail(&p, e.to_string(), false, now).await,
        };

        // Check the stored transaction, and resubmit the same bytes rather
        // than signing a new transfer.
        if let Some((hash, b64)) = &stored {
            match self.near.tx_status(hash, &treasury, TxWait::Probe).await {
                Ok(Some(o)) if o.status == TxStatus::Success => return self.mark_sent(&p, hash, now).await,
                Ok(Some(o)) => return self.fail(&p, format!("transaction failed on chain: {:?}", o.status), true, now).await,
                // Unknown, or known but not executed yet: resubmitting the same
                // bytes is idempotent (same nonce), so it can never pay twice.
                Ok(None) | Err(ToolError::Retryable(_)) => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
                    return self.broadcast(&p, hash.clone(), &bytes, now).await;
                }
                Err(e) => return self.fail_tool(&p, e, now).await,
            }
        }
        let Some(signed) = signed else { anyhow::bail!("payout {payout_id}: no stored transaction and no authorization") };

        let access = match self.near.view_access_key(&treasury, &pk).await {
            Ok(Some(a)) => a,
            // Not the payout's fault and the same for every payout: wait for
            // the treasury to be funded instead of using up the retry budget
            // (a treasury can't be funded before its key, and so its
            // address, exists).
            Ok(None) => return self.pause(&p, format!("treasury {treasury} has no access key for the OpenBao key yet; fund it to start payouts"), now).await,
            Err(e) => return self.fail_tool(&p, e, now).await,
        };
        // NEAR reports the key's nonce as of the last final block, a couple
        // of blocks behind. A payout sent a moment ago has already used the
        // next one, so take the higher of the chain's nonce and the last
        // nonce signed here.
        let nonce = access.nonce.max(last_nonce_used(self.pool, &self.cfg.network).await?) + 1;
        let deposit: u128 = signed.payload.amount_yocto.parse()?;
        let tx = match &signed.payload.destination {
            None => Transaction::transfer(treasury.clone(), pk, nonce, p.recipient.clone(), access.block_hash, deposit),
            Some(dest) => {
                let deposit_address = match self.intent_quote(&p, dest, &treasury, deposit, now).await? {
                    Ok(a) => a,
                    Err(QuoteRefusal::Retry(m)) => return self.fail(&p, m, false, now).await,
                    Err(QuoteRefusal::Refuse(m)) => return self.fail(&p, m, true, now).await,
                };
                let memo = deposit_memo(p.id);
                let tx = Transaction {
                    signer_id: treasury.clone(),
                    public_key: pk,
                    nonce,
                    receiver_id: intents::wnear_contract(&p.network).to_string(),
                    block_hash: access.block_hash,
                    actions: intents::deposit_actions(&deposit_address, deposit, &memo),
                };
                // Belt and braces: exactly the deposit template, nothing else.
                if let Err(e) = intents::check_deposit_tx(&tx, &p.network, &deposit_address, deposit, &memo) {
                    return self.fail(&p, format!("deposit transaction self-check failed: {e}"), true, now).await;
                }
                tx
            }
        };
        let hash = tx.signing_hash();
        let sig = match self.transit.sign(&hash).await {
            Ok(s) => s,
            // Sealed, unreachable, or the signer's token refused (expired or
            // revoked): none of that is about this payout, so nothing is counted.
            Err(e @ (TransitError::Sealed | TransitError::Unavailable(_) | TransitError::Forbidden)) => return self.pause(&p, e.to_string(), now).await,
            Err(e) => return self.fail(&p, e.to_string(), false, now).await,
        };
        // Self-check before anything leaves: the signature must verify the way NEAR verifies it.
        if pk.verify(&hash, &sig).is_err() {
            return self.fail(&p, "OpenBao signature does not verify against the treasury key".into(), true, now).await;
        }
        let bytes = tx.signed_borsh(&sig);
        let tx_hash = bs58::encode(hash).into_string();
        // Only if nothing is stored yet: a second signer working on the same
        // payout must not sign a second transfer over the first.
        let stored = sqlx::query("UPDATE axom.payouts SET tx_hash = $2, signed_tx = $3, tx_nonce = $4 WHERE id = $1 AND tx_hash IS NULL")
            .bind(p.id)
            .bind(&tx_hash)
            .bind(base64::engine::general_purpose::STANDARD.encode(&bytes))
            .bind(tx.nonce as i64)
            .execute(self.pool)
            .await?;
        if stored.rows_affected() != 1 {
            anyhow::bail!("payout {} already has a signed transaction (another signer?); not sending a second one", p.id);
        }
        self.audit(
            "signer.sign",
            "signed",
            p.id,
            json!({ "tx_hash": tx_hash, "treasury": treasury, "recipient": p.recipient, "amount_yocto": signed.payload.amount_yocto,
                    "authorization": signed.payload.kind.as_str(), "nonce": tx.nonce, "receiver_id": tx.receiver_id,
                    "destination": signed.payload.destination }),
            "all signer checks passed",
            now,
        )
        .await?;
        self.broadcast(&p, tx_hash, &bytes, now).await
    }

    async fn broadcast(&self, p: &PayoutRow, tx_hash: String, bytes: &[u8], now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        match self.near.send_tx(bytes).await {
            Ok(o) if o.status == TxStatus::Success => self.mark_sent(p, &tx_hash, now).await,
            Ok(o) => self.fail(p, format!("transaction failed on chain: {:?}", o.status), true, now).await,
            Err(ToolError::Permanent(m)) if m.contains("Expired") => {
                // The stored transaction can never land now; drop it so the next attempt signs afresh.
                // A cross-chain payout also gets a fresh quote: nothing reached the old deposit address.
                self.drop_stored_tx(p).await?;
                self.fail_with(p, format!("transaction expired before inclusion: {m}"), false, false, now).await
            }
            Err(ToolError::Permanent(m)) if m.contains("InvalidNonce") => {
                // The nonce is used up: by this very transaction (it landed)
                // or by another one (this one never can). Find out which.
                let treasury = match self.treasury_account().await {
                    Ok((_, t)) => t,
                    Err(e) => return self.fail_with(p, e.to_string(), false, true, now).await,
                };
                match self.near.tx_status(&tx_hash, &treasury, TxWait::Probe).await {
                    Ok(Some(o)) if o.status == TxStatus::Success => self.mark_sent(p, &tx_hash, now).await,
                    Ok(Some(o)) => self.fail(p, format!("transaction failed on chain: {:?}", o.status), true, now).await,
                    // Unknown twice, a backoff apart: it did not land. Sign
                    // again with a fresh nonce. (Once is not enough: the node
                    // answering may be behind the one that took it.)
                    Ok(None) if p.last_error.as_deref().is_some_and(|e| e.contains("InvalidNonce")) => {
                        self.drop_stored_tx(p).await?;
                        self.fail_with(p, format!("nonce already used and the transaction is unknown; signing again: {m}"), false, false, now).await
                    }
                    _ => self.fail_with(p, format!("nonce already used; checking again whether the transaction landed: {m}"), false, true, now).await,
                }
            }
            // In here a signed transaction always exists, whatever `p` (read
            // before signing) says: a send that didn't get an answer leaves
            // its fate open.
            Err(e @ (ToolError::Permanent(_) | ToolError::NotAuthorized(_))) => self.fail(p, e.to_string(), true, now).await,
            Err(e) => self.fail_with(p, e.to_string(), false, true, now).await,
        }
    }

    async fn drop_stored_tx(&self, p: &PayoutRow) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE axom.payouts SET tx_hash = NULL, signed_tx = NULL,
                    intent_deposit_address = NULL, intent_quote = NULL, intent_quote_signature = NULL WHERE id = $1",
        )
        .bind(p.id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The signer's checks refused this payout. Recorded and reported once
    /// per reason: the payout is looked at again every cycle, and without
    /// this each cycle added an audit entry and an owner email.
    async fn reject(&self, p: &PayoutRow, reason: String, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        if p.last_error.as_deref() == Some(reason.as_str()) {
            return Ok(SendResult::Rejected { reason });
        }
        sqlx::query("UPDATE axom.payouts SET last_error = $2 WHERE id = $1").bind(p.id).bind(&reason).execute(self.pool).await?;
        let audit_id = self.audit("signer.sign", "rejected", p.id, json!({ "status": p.status }), &reason, now).await?;
        notify::enqueue(
            self.pool,
            "signer_rejected",
            "Signer refused a payout",
            json!({ "payout_id": p.id, "reason": reason, "audit_link": self.notify.audit_link(audit_id) }),
            Some(audit_id),
        )
        .await?;
        Ok(SendResult::Rejected { reason })
    }

    async fn mark_sent(&self, p: &PayoutRow, tx_hash: &str, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        payouts::move_payout(self.pool, &Actor::agent("axom-signer"), p.id, PayoutStatus::Sent, &format!("sent: {tx_hash}"), self.policy.version(), now).await?;
        sqlx::query("UPDATE axom.payouts SET sent_at = $2, last_error = NULL, next_attempt_at = NULL WHERE id = $1")
            .bind(p.id)
            .bind(now)
            .execute(self.pool)
            .await?;
        // Cross-chain: tell 1Click about the deposit (optional, speeds up the
        // fill; status polling finds it either way).
        let deposit_address: Option<(Option<String>,)> =
            sqlx::query_as("SELECT intent_deposit_address FROM axom.payouts WHERE id = $1").bind(p.id).fetch_optional(self.pool).await?;
        if let (Some(intents_api), Some((Some(addr),))) = (self.intents, deposit_address) {
            if let Ok((_, treasury)) = self.treasury_account().await {
                if let Err(e) = intents_api.submit_deposit(tx_hash, &addr, &treasury).await {
                    tracing::warn!(payout_id = %p.id, error = %e, "1Click deposit submit failed (status polling will still find it)");
                }
            }
        }
        Ok(SendResult::Sent { tx_hash: tx_hash.to_string() })
    }

    /// Get a 1Click quote for a cross-chain payout and check it against the
    /// authorization. Stores the quote (with 1Click's signature) and returns
    /// the deposit address.
    async fn intent_quote(&self, p: &PayoutRow, dest: &Destination, treasury: &str, amount_yocto: u128, now: DateTime<Utc>) -> anyhow::Result<Result<String, QuoteRefusal>> {
        let Some(api) = self.intents else { return Ok(Err(QuoteRefusal::Refuse("NEAR Intents client not configured".into()))) };
        let rules = self.policy.intents();
        let origin = intents::origin_asset(&p.network);
        let mut req = QuoteRequest::payout(
            origin.clone(),
            dest.asset_id.clone(),
            dest.address.clone(),
            amount_yocto,
            treasury.to_string(),
            rules.slippage_bps,
            now + Duration::hours(1),
            false,
        );
        req.reference_usd_per_near = p.usd_per_near;
        let q = match api.quote(&req).await {
            Ok(q) => q,
            Err(e @ (ToolError::Permanent(_) | ToolError::NotAuthorized(_))) => return Ok(Err(QuoteRefusal::Retry(format!("1Click refused the quote: {e}")))),
            Err(e) => return Ok(Err(QuoteRefusal::Retry(format!("1Click unavailable: {e}")))),
        };
        // 1Click output is untrusted: it must quote exactly what was authorized.
        let echo = |k: &str| q.echoed_request.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let mismatch = [
            ("recipient", dest.address.as_str()),
            ("destinationAsset", dest.asset_id.as_str()),
            ("originAsset", origin.as_str()),
            ("refundTo", treasury),
            ("amount", req.amount.as_str()),
        ]
        .into_iter()
        .find(|(k, want)| echo(k).as_deref() != Some(*want));
        if let Some((k, want)) = mismatch {
            return Ok(Err(QuoteRefusal::Refuse(format!("1Click quote {k} is {:?}, authorized {want:?}", echo(k)))));
        }
        if q.amount_in != req.amount {
            return Ok(Err(QuoteRefusal::Refuse(format!("1Click quote amountIn {} differs from the authorized {}", q.amount_in, req.amount))));
        }
        let Some(deposit_address) = q.deposit_address.clone() else { return Ok(Err(QuoteRefusal::Refuse("1Click quote has no deposit address".into()))) };
        if near_core::validate_account_id(&deposit_address).is_err() || deposit_address == treasury {
            return Ok(Err(QuoteRefusal::Refuse(format!("1Click deposit address {deposit_address:?} is not a NEAR account"))));
        }
        if q.deposit_memo.is_some() {
            return Ok(Err(QuoteRefusal::Refuse("1Click quote requires a deposit memo, which NEAR deposits do not support".into())));
        }
        if let Some(deadline) = q.deadline {
            if deadline < now + Duration::minutes(5) {
                return Ok(Err(QuoteRefusal::Retry(format!("1Click quote deadline {deadline} is too close"))));
            }
        }
        let floor = p.amount_usd * (Decimal::ONE - Decimal::from(rules.max_shortfall_bps) / Decimal::from(10_000));
        match q.amount_out_usd {
            Some(out) if out >= floor => {}
            Some(out) => {
                return Ok(Err(QuoteRefusal::Retry(format!(
                    "1Click quote delivers {out} USD, below the {floor} USD floor for a {} USD payout (max shortfall {} bps)",
                    p.amount_usd, rules.max_shortfall_bps
                ))))
            }
            None => return Ok(Err(QuoteRefusal::Retry("1Click quote has no USD output to check".into()))),
        }
        sqlx::query("UPDATE axom.payouts SET intent_deposit_address = $2, intent_quote = $3, intent_quote_signature = $4, intent_status = 'QUOTED' WHERE id = $1")
            .bind(p.id)
            .bind(&deposit_address)
            .bind(&q.raw)
            .bind(&q.signature)
            .execute(self.pool)
            .await?;
        self.audit(
            "signer.intent_quote",
            "accepted",
            p.id,
            json!({ "mode": api.mode(), "deposit_address": deposit_address, "destination": dest, "amount_in": q.amount_in,
                    "amount_out": q.amount_out_formatted, "amount_out_usd": q.amount_out_usd, "min_amount_out": q.min_amount_out,
                    "time_estimate_secs": q.time_estimate_secs }),
            "quote matches the authorization",
            now,
        )
        .await?;
        Ok(Ok(deposit_address))
    }

    async fn pause(&self, p: &PayoutRow, reason: String, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        // Once per reason, not once per cycle while the vault stays sealed.
        if p.last_error.as_deref() != Some(reason.as_str()) {
            sqlx::query("UPDATE axom.payouts SET last_error = $2 WHERE id = $1").bind(p.id).bind(&reason).execute(self.pool).await?;
            self.audit("signer.sign", "paused", p.id, json!({}), &reason, now).await?;
        }
        Ok(SendResult::Paused { reason })
    }

    async fn fail_tool(&self, p: &PayoutRow, e: ToolError, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        let permanent = matches!(e, ToolError::Permanent(_) | ToolError::NotAuthorized(_));
        self.fail(p, e.to_string(), permanent, now).await
    }

    /// Record a failed attempt. Retryable failures back off exponentially;
    /// the final attempt (or a permanent failure) releases the cap
    /// reservation and alerts the owner.
    async fn fail(&self, p: &PayoutRow, reason: String, permanent: bool, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        // A permanent failure is the chain's own verdict on the transaction.
        // Otherwise a transaction that was signed may still land.
        self.fail_with(p, reason, permanent, !permanent && p.tx_hash.is_some(), now).await
    }

    /// `tx_unresolved`: a transaction was signed for this payout and nothing
    /// says yet whether it landed or never can.
    async fn fail_with(&self, p: &PayoutRow, reason: String, permanent: bool, tx_unresolved: bool, now: DateTime<Utc>) -> anyhow::Result<SendResult> {
        if p.status == "approved" {
            payouts::move_payout(self.pool, &Actor::agent("axom-signer"), p.id, PayoutStatus::Failed, &reason, self.policy.version(), now).await?;
        }
        let attempt = p.attempts + 1;
        let out_of_attempts = attempt >= self.cfg.max_attempts;
        if !permanent && out_of_attempts && tx_unresolved {
            // Out of retries, but the signed transaction could still be
            // included. Giving up now would release the cap and report a
            // failure for a payment that may go through. Keep the reservation
            // and look once more after the transaction can no longer land:
            // by then it is either on chain or expired.
            let next = now + Duration::hours(TX_OUTCOME_WAIT_HOURS);
            sqlx::query("UPDATE axom.payouts SET attempts = $2, next_attempt_at = $3, last_error = $4 WHERE id = $1")
                .bind(p.id)
                .bind(self.cfg.max_attempts - 1)
                .bind(next)
                .bind(&reason)
                .execute(self.pool)
                .await?;
            let audit_id = self.audit("signer.send", "outcome_unknown", p.id, json!({ "attempt": attempt, "next_attempt_at": next, "tx_hash": p.tx_hash }), &reason, now).await?;
            notify::enqueue(
                self.pool,
                "payout_outcome_unknown",
                "Payout sent but not confirmed yet",
                json!({ "payout_id": p.id, "recipient": p.recipient, "amount_usd": p.amount_usd, "tx_hash": p.tx_hash, "checking_again_at": next,
                        "reason": reason, "audit_link": self.notify.audit_link(audit_id) }),
                Some(audit_id),
            )
            .await?;
            return Ok(SendResult::Failed { reason, attempt, next_attempt_at: Some(next), final_attempt: false });
        }
        let final_attempt = permanent || out_of_attempts;
        let next = (!final_attempt).then(|| now + backoff(self.cfg.base_backoff, attempt));
        sqlx::query("UPDATE axom.payouts SET attempts = $2, next_attempt_at = $3, last_error = $4 WHERE id = $1")
            .bind(p.id)
            .bind(attempt)
            .bind(next)
            .bind(&reason)
            .execute(self.pool)
            .await?;
        let audit_id = self
            .audit("signer.send", if final_attempt { "failed_final" } else { "failed_retrying" }, p.id, json!({ "attempt": attempt, "next_attempt_at": next }), &reason, now)
            .await?;
        if final_attempt {
            payouts::release_failed(self.pool, self.policy, p.id, &reason, now).await?;
            notify::enqueue(
                self.pool,
                "payout_failed",
                "Payout failed after final attempt",
                json!({ "payout_id": p.id, "recipient": p.recipient, "amount_usd": p.amount_usd, "attempts": attempt,
                        "reason": reason, "audit_link": self.notify.audit_link(audit_id) }),
                Some(audit_id),
            )
            .await?;
        }
        Ok(SendResult::Failed { reason, attempt, next_attempt_at: next, final_attempt })
    }

    /// Failed payouts left between two steps of [`Self::fail_with`] by a
    /// crash: marked failed, but with no retry scheduled and not given up.
    /// Nothing else would ever look at them again.
    async fn recover_interrupted(&self, now: DateTime<Utc>) -> anyhow::Result<()> {
        let rows: Vec<(Uuid, i32, bool, Option<String>)> = sqlx::query_as(
            "SELECT id, attempts, tx_hash IS NOT NULL, last_error FROM axom.payouts
             WHERE network = $1 AND status = 'failed' AND next_attempt_at IS NULL AND abandoned_at IS NULL",
        )
        .bind(&self.cfg.network)
        .fetch_all(self.pool)
        .await?;
        for (id, attempts, has_tx, last_error) in rows {
            if has_tx || attempts < self.cfg.max_attempts {
                // Try again (a stored transaction is checked on chain first).
                sqlx::query("UPDATE axom.payouts SET attempts = LEAST(attempts, $2), next_attempt_at = $3 WHERE id = $1")
                    .bind(id)
                    .bind(self.cfg.max_attempts - 1)
                    .bind(now)
                    .execute(self.pool)
                    .await?;
            } else {
                payouts::release_failed(self.pool, self.policy, id, last_error.as_deref().unwrap_or("final attempt failed"), now).await?;
            }
        }
        Ok(())
    }

    /// Send every approved payout and every failed payout whose retry is due.
    pub async fn process_due(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<(Uuid, SendResult)>> {
        if let Err(e) = self.recover_interrupted(now).await {
            tracing::error!(error = %e, "recovering interrupted payouts failed");
        }
        let ids: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM axom.payouts
             WHERE network = $1 AND (
                (status = 'approved' AND EXISTS (SELECT 1 FROM axom.payout_authorizations a WHERE a.payout_id = payouts.id))
                OR (status = 'failed' AND attempts < $2 AND next_attempt_at IS NOT NULL AND next_attempt_at <= $3))
             ORDER BY created_at",
        )
        .bind(&self.cfg.network)
        .bind(self.cfg.max_attempts)
        .bind(now)
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::new();
        for (id,) in ids {
            // One payout that errors (a database hiccup, a row it can't
            // read) must not stop the ones after it, this cycle and every
            // cycle after.
            let r = match self.send_payout(id, now).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(payout_id = %id, error = %e, "sending this payout failed; continuing with the others");
                    continue;
                }
            };
            let paused = matches!(r, SendResult::Paused { .. });
            out.push((id, r));
            if paused {
                break; // OpenBao is sealed or down: stop for this cycle.
            }
        }
        Ok(out)
    }

    /// Mark sent payouts finalized once NEAR reports them final. A
    /// cross-chain payout is final only when NEAR Intents reports delivery
    /// on the destination chain; a refund moves it to `refunded`.
    pub async fn finalize_sent(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<Uuid>> {
        let rows: Vec<(Uuid, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, tx_hash, intent_deposit_address, intent_status FROM axom.payouts WHERE status = 'sent' AND network = $1 AND tx_hash IS NOT NULL",
        )
        .bind(&self.cfg.network)
        .fetch_all(self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(vec![]);
        }
        let treasury = match self.treasury_account().await {
            Ok((_, t)) => t,
            Err(_) => return Ok(vec![]),
        };
        let mut done = Vec::new();
        for (id, hash, deposit_address, last_status) in rows {
            let Ok(Some(o)) = self.near.tx_status(&hash, &treasury, TxWait::Final).await else { continue };
            if !(o.is_final && o.status == TxStatus::Success) {
                continue;
            }
            let Some(addr) = deposit_address else {
                payouts::move_payout(self.pool, &Actor::agent("axom-signer"), id, PayoutStatus::Finalized, "final on NEAR", self.policy.version(), now).await?;
                sqlx::query("UPDATE axom.payouts SET finalized_at = $2 WHERE id = $1").bind(id).bind(now).execute(self.pool).await?;
                done.push(id);
                continue;
            };
            let Some(api) = self.intents else { continue };
            let st = match api.status(&addr).await {
                Ok(st) => st,
                Err(e) => {
                    tracing::warn!(payout_id = %id, error = %e, "1Click status unavailable");
                    continue;
                }
            };
            let changed = last_status.as_deref() != Some(st.status.as_str());
            sqlx::query("UPDATE axom.payouts SET intent_status = $2, intent_tx_hashes = $3, intent_checked_at = $4 WHERE id = $1")
                .bind(id)
                .bind(st.status.as_str())
                .bind(serde_json::to_value(&st.destination_txs)?)
                .bind(now)
                .execute(self.pool)
                .await?;
            match st.status {
                ExecutionStatus::Success => {
                    let reason = format!("delivered on destination chain via NEAR Intents ({})", api.mode());
                    payouts::move_payout(self.pool, &Actor::agent("axom-signer"), id, PayoutStatus::Finalized, &reason, self.policy.version(), now).await?;
                    sqlx::query("UPDATE axom.payouts SET finalized_at = $2 WHERE id = $1").bind(id).bind(now).execute(self.pool).await?;
                    self.audit("signer.intent_status", "delivered", id, json!({ "deposit_address": addr, "destination_txs": st.destination_txs, "mode": api.mode() }), &reason, now).await?;
                    done.push(id);
                }
                ExecutionStatus::Refunded => {
                    let reason = format!("NEAR Intents refunded the deposit to the treasury as wNEAR: {}", st.refund_reason.clone().unwrap_or_default());
                    payouts::move_payout(self.pool, &Actor::agent("axom-signer"), id, PayoutStatus::Refunded, &reason, self.policy.version(), now).await?;
                    let audit_id = self.audit("signer.intent_status", "refunded", id, json!({ "deposit_address": addr }), &reason, now).await?;
                    notify::enqueue(
                        self.pool,
                        "payout_refunded",
                        "Cross-chain payout refunded: worker not paid",
                        json!({ "payout_id": id, "deposit_address": addr, "reason": reason, "audit_link": self.notify.audit_link(audit_id) }),
                        Some(audit_id),
                    )
                    .await?;
                }
                ExecutionStatus::Failed | ExecutionStatus::IncompleteDeposit if changed => {
                    let reason = format!("NEAR Intents reports {} for the deposit; needs a look", st.status.as_str());
                    let audit_id = self.audit("signer.intent_status", "attention", id, json!({ "deposit_address": addr }), &reason, now).await?;
                    notify::enqueue(
                        self.pool,
                        "payout_intent_attention",
                        "Cross-chain payout needs attention",
                        json!({ "payout_id": id, "deposit_address": addr, "status": st.status.as_str(), "audit_link": self.notify.audit_link(audit_id) }),
                        Some(audit_id),
                    )
                    .await?;
                }
                _ => {}
            }
        }
        Ok(done)
    }
}

/// How long after the last failed attempt a signed transaction is looked at
/// once more before the payout is given up. A NEAR transaction can be
/// included for about a day after the block it names; after this wait it is
/// either on chain or expired.
pub const TX_OUTCOME_WAIT_HOURS: i64 = 26;

/// `base * 2^(attempt-1)`, capped at one day.
pub fn backoff(base: Duration, attempt: i32) -> Duration {
    let factor = 2i64.saturating_pow((attempt.max(1) - 1) as u32);
    let secs = base.num_seconds().saturating_mul(factor);
    Duration::seconds(secs.min(86_400))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles() {
        let b = Duration::seconds(30);
        assert_eq!(backoff(b, 1), Duration::seconds(30));
        assert_eq!(backoff(b, 2), Duration::seconds(60));
        assert_eq!(backoff(b, 4), Duration::seconds(240));
        assert_eq!(backoff(b, 40), Duration::seconds(86_400));
    }
}
