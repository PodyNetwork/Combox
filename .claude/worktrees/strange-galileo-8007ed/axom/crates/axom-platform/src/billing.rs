//! Subscriptions paid in NEAR.
//!
//! One plan with a free trial. To pay, a company opens an invoice: the plan's
//! USD price converted to NEAR at the current rate, payable to the platform's
//! account within 24 hours. The customer sends the NEAR from any wallet and
//! submits the transaction hash with the sending account. ForeMan checks the
//! transaction on NEAR itself (receiver, sender, amount, success, time) and
//! that the hash hasn't paid another invoice, then extends the subscription.
//! Nothing a customer types is trusted.
//!
//! A NEAR transfer carries no memo, and every transaction's hash and sender
//! are public, so the amount is what ties a payment to one invoice: each
//! invoice asks for its own amount (the price plus a few random final
//! digits, unique among invoices that can still be paid), the payment must
//! be exactly that amount, and it must have been made after the invoice was
//! opened. Another company can't claim the payment: it can't open an invoice
//! for the same amount while this one is payable, and one opened after the
//! payment is too late.

use async_trait::async_trait;
use axom_core::near;
use axom_hr::PriceQuote;
use axom_hr::approvals::AdminSession;
use axom_tools::price::PriceFeed;
use chrono::{DateTime, Duration, TimeZone, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::tenants::{audit, Platform, TenantError};

#[derive(Debug, Clone, Serialize)]
pub struct PlanConfig {
    pub name: String,
    pub price_usd: Decimal,
    pub trial_days: i64,
    /// NEAR account subscription payments go to (`AXOM_BILLING_ACCOUNT`).
    pub pay_to: Option<String>,
    pub network: String,
    pub invoice_ttl_hours: i64,
}

impl PlanConfig {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            name: get("AXOM_PLAN_NAME").unwrap_or_else(|| "ForeMan".into()),
            price_usd: get("AXOM_PLAN_PRICE_USD").and_then(|v| v.parse().ok()).unwrap_or(Decimal::from(49)),
            trial_days: get("AXOM_TRIAL_DAYS").and_then(|v| v.parse().ok()).unwrap_or(14),
            pay_to: get("AXOM_BILLING_ACCOUNT"),
            network: if get("NEAR_NETWORK").as_deref() == Some("mainnet") { "mainnet".into() } else { "testnet".into() },
            invoice_ttl_hours: 24,
        }
    }
}

/// A NEAR transfer as reported by the chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub signer_id: String,
    pub receiver_id: String,
    pub deposit_yocto: u128,
    pub success: bool,
    pub is_final: bool,
    pub block_time: DateTime<Utc>,
}

#[async_trait]
pub trait PaymentLookup: Send + Sync {
    /// The transaction, or `None` if NEAR doesn't know it.
    async fn transfer(&self, tx_hash: &str, sender: &str) -> anyhow::Result<Option<Transfer>>;
}

/// Looks transactions up on a NEAR RPC node.
pub struct RpcPayments {
    client: reqwest::Client,
    url: String,
}

impl RpcPayments {
    pub fn new(url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().expect("http client");
        Self { client, url: url.into() }
    }

    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Result<Value, Value>> {
        let v: Value = self.client.post(&self.url).json(&json!({ "jsonrpc": "2.0", "id": "axom", "method": method, "params": params })).send().await?.json().await?;
        Ok(match v.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        })
    }
}

/// Parse NEAR's `tx` result (without the block time).
pub fn parse_tx(r: &Value) -> Option<(String, String, u128, bool, bool, String)> {
    let tx = r.get("transaction")?;
    let deposit: u128 = tx
        .get("actions")?
        .as_array()?
        .iter()
        .filter_map(|a| a.pointer("/Transfer/deposit").and_then(Value::as_str).and_then(|d| d.parse::<u128>().ok()))
        .sum();
    let success = r.pointer("/status/SuccessValue").is_some();
    let is_final = matches!(r.get("final_execution_status").and_then(Value::as_str), Some("FINAL") | None);
    Some((
        tx.get("signer_id")?.as_str()?.to_string(),
        tx.get("receiver_id")?.as_str()?.to_string(),
        deposit,
        success,
        is_final,
        r.pointer("/transaction_outcome/block_hash")?.as_str()?.to_string(),
    ))
}

#[async_trait]
impl PaymentLookup for RpcPayments {
    async fn transfer(&self, tx_hash: &str, sender: &str) -> anyhow::Result<Option<Transfer>> {
        let r = match self.call("tx", json!({ "tx_hash": tx_hash, "sender_account_id": sender, "wait_until": "FINAL" })).await? {
            Ok(r) => r,
            Err(e) if e.pointer("/cause/name").and_then(Value::as_str) == Some("UNKNOWN_TRANSACTION") => return Ok(None),
            Err(e) => anyhow::bail!("NEAR RPC error: {e}"),
        };
        let Some((signer_id, receiver_id, deposit_yocto, success, is_final, block_hash)) = parse_tx(&r) else {
            anyhow::bail!("unexpected transaction format from NEAR RPC");
        };
        let block = self.call("block", json!({ "block_id": block_hash })).await?.map_err(|e| anyhow::anyhow!("NEAR RPC error: {e}"))?;
        let ns = block.pointer("/header/timestamp").and_then(Value::as_u64).ok_or_else(|| anyhow::anyhow!("block without timestamp"))?;
        let block_time = Utc.timestamp_nanos(ns as i64);
        Ok(Some(Transfer { signer_id, receiver_id, deposit_yocto, success, is_final, block_time }))
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Invoice {
    pub id: Uuid,
    pub months: i32,
    pub amount_usd: Decimal,
    pub usd_per_near: Decimal,
    #[serde(serialize_with = "ser_yocto")]
    pub amount_yocto: Decimal,
    pub pay_to: String,
    pub network: String,
    pub status: String,
    pub tx_hash: Option<String>,
    pub payer: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
}

fn ser_yocto<S: serde::Serializer>(d: &Decimal, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&d.trunc().to_string())
}

impl Invoice {
    pub fn amount_near(&self) -> String {
        self.amount_yocto.trunc().to_string().parse::<u128>().map(near::yocto_to_near_string).unwrap_or_default()
    }
}

/// Invoice amounts are whole multiples of this: 8 decimal places of NEAR,
/// which wallets and exchanges can send.
const AMOUNT_STEP_YOCTO: u128 = 10u128.pow(16);
/// The price is rounded up to this (0.01 NEAR) before the tag is added, so
/// the tag fills decimal places 3 to 8 and never changes the price by a cent.
const PRICE_STEP_YOCTO: u128 = 10u128.pow(22);
/// Tags are 1..TAG_VALUES.
const TAG_VALUES: u32 = 1_000_000;

/// The amount an invoice asks for: the price rounded up to 0.01 NEAR, plus
/// `tag` in the last six of eight decimal places.
pub fn tagged_amount(price_yocto: u128, tag: u32) -> u128 {
    debug_assert!((1..TAG_VALUES).contains(&tag));
    price_yocto.div_ceil(PRICE_STEP_YOCTO) * PRICE_STEP_YOCTO + u128::from(tag) * AMOUNT_STEP_YOCTO
}

const INVOICE_COLS: &str = "id, months, amount_usd, usd_per_near, amount_yocto, pay_to, network, status, tx_hash, payer, created_at, expires_at, paid_at";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PaymentError {
    #[error("invoice not found")]
    NotFound,
    #[error("this invoice is {0}")]
    NotOpen(String),
    #[error("transaction not found on NEAR {0} for that sender; check the hash and the sending account")]
    UnknownTx(String),
    #[error("the transaction failed on NEAR")]
    Failed,
    #[error("the transaction is not final yet; try again in a minute")]
    NotFinal,
    #[error("the transaction was sent to {0}, not to {1}")]
    WrongReceiver(String, String),
    #[error("the transaction was signed by {0}, not by the account you entered")]
    WrongSender(String),
    #[error("the transaction sent {sent} NEAR; this invoice needs {needed} NEAR")]
    TooLittle { sent: String, needed: String },
    #[error("the transaction sent {sent} NEAR; this invoice needs exactly {needed} NEAR (the last digits identify your payment)")]
    NotExact { sent: String, needed: String },
    #[error("the transaction was made before this invoice was opened or after its payment window; confirm it against the invoice you paid")]
    OutsideWindow,
    #[error("this transaction already paid an invoice")]
    AlreadyUsed,
    #[error("that isn't a NEAR transaction hash; copy it from your wallet or the explorer")]
    BadHash,
    #[error("{0}")]
    Other(String),
}

fn valid_account(a: &str) -> bool {
    near::validate_account_id(a).is_ok()
}

impl Platform {
    pub async fn invoices(&self, tenant: Uuid) -> Result<Vec<Invoice>, sqlx::Error> {
        sqlx::query_as::<_, Invoice>(&format!("SELECT {INVOICE_COLS} FROM platform.invoices WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 50"))
            .bind(tenant)
            .fetch_all(&self.db)
            .await
    }

    /// Open an invoice for `months` of the plan at the current NEAR price.
    pub async fn create_invoice(&self, tenant: Uuid, s: &AdminSession, months: i32, price: &dyn PriceFeed, now: DateTime<Utc>) -> Result<Invoice, TenantError> {
        if self.role(tenant, s.user_id).await?.is_none() {
            return Err(TenantError::Forbidden("not a member of this company".into()));
        }
        if !(1..=12).contains(&months) {
            return Err(TenantError::Invalid("months: 1 to 12".into()));
        }
        let pay_to = self.plan.pay_to.clone().ok_or_else(|| TenantError::Invalid("payments are not set up on this platform yet (AXOM_BILLING_ACCOUNT)".into()))?;
        let q: PriceQuote = price.quote().await.map_err(|e| TenantError::Invalid(format!("no NEAR price right now ({e}); try again shortly")))?;
        if now - q.fetched_at > Duration::minutes(10) {
            return Err(TenantError::Invalid("the NEAR price is stale; try again shortly".into()));
        }
        let usd = self.plan.price_usd * Decimal::from(months);
        let price_yocto = near::usd_to_yocto(usd, q.usd_per_near).map_err(|e| TenantError::Invalid(e.to_string()))?;
        let id = Uuid::new_v4();
        let mut tx = self.db.begin().await?;
        // One invoice is opened at a time per billing account, so two can't
        // pick the same amount between the check below and the insert.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('platform.invoices:' || $1))").bind(&pay_to).execute(&mut *tx).await?;
        // The amount must be unique among invoices that can still be paid.
        // That includes ones marked expired because a newer one replaced
        // them: a payment made inside their window still counts.
        let mut yocto = None;
        for _ in 0..20 {
            let candidate = tagged_amount(price_yocto, rand::Rng::gen_range(&mut rand::rngs::OsRng, 1..TAG_VALUES));
            let (taken,): (bool,) = sqlx::query_as(
                "SELECT EXISTS (SELECT 1 FROM platform.invoices WHERE pay_to = $1 AND amount_yocto = $2::numeric AND status <> 'paid' AND expires_at + interval '5 minutes' > $3)",
            )
            .bind(&pay_to)
            .bind(candidate.to_string())
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
            if !taken {
                yocto = Some(candidate);
                break;
            }
        }
        let yocto = yocto.ok_or_else(|| TenantError::Invalid("couldn't open an invoice right now; try again shortly".into()))?;
        // Expire older open invoices: only the newest price is offered.
        sqlx::query("UPDATE platform.invoices SET status = 'expired' WHERE tenant_id = $1 AND status = 'open'").bind(tenant).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO platform.invoices (id, tenant_id, months, amount_usd, usd_per_near, amount_yocto, pay_to, network, created_by, created_at, expires_at)
             VALUES ($1,$2,$3,$4,$5,$6::numeric,$7,$8,$9,$10,$11)",
        )
        .bind(id)
        .bind(tenant)
        .bind(months)
        .bind(usd)
        .bind(q.usd_per_near)
        .bind(yocto.to_string())
        .bind(&pay_to)
        .bind(&self.plan.network)
        .bind(s.user_id)
        .bind(now)
        .bind(now + Duration::hours(self.plan.invoice_ttl_hours))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        audit(&self.db, Some(tenant), &format!("user:{}", s.user_id), "billing.invoice", json!({ "invoice_id": id, "months": months, "amount_usd": usd, "usd_per_near": q.usd_per_near })).await?;
        Ok(sqlx::query_as::<_, Invoice>(&format!("SELECT {INVOICE_COLS} FROM platform.invoices WHERE id = $1")).bind(id).fetch_one(&self.db).await?)
    }

    /// Check a payment on NEAR and, if it pays the invoice, extend the
    /// subscription by the invoice's months (30 days each).
    #[allow(clippy::too_many_arguments)]
    pub async fn verify_payment(
        &self,
        tenant: Uuid,
        s: &AdminSession,
        invoice_id: Uuid,
        tx_hash: &str,
        payer: &str,
        lookup: &dyn PaymentLookup,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, PaymentError> {
        if self.role(tenant, s.user_id).await.map_err(|e| PaymentError::Other(e.to_string()))?.is_none() {
            return Err(PaymentError::Other("not a member of this company".into()));
        }
        let tx_hash = tx_hash.trim();
        let payer = payer.trim().to_lowercase();
        // A NEAR transaction hash is 32 bytes, base58-encoded.
        if bs58::decode(tx_hash).into_vec().map(|b| b.len()) != Ok(32) {
            return Err(PaymentError::BadHash);
        }
        if !valid_account(&payer) {
            return Err(PaymentError::Other("the sending account looks wrong".into()));
        }
        let inv = sqlx::query_as::<_, Invoice>(&format!("SELECT {INVOICE_COLS} FROM platform.invoices WHERE id = $1 AND tenant_id = $2"))
            .bind(invoice_id)
            .bind(tenant)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| PaymentError::Other(e.to_string()))?
            .ok_or(PaymentError::NotFound)?;
        if inv.status == "paid" {
            return Err(PaymentError::NotOpen("already paid".into()));
        }
        let (used,): (i64,) = sqlx::query_as("SELECT count(*) FROM platform.invoices WHERE tx_hash = $1").bind(tx_hash).fetch_one(&self.db).await.map_err(|e| PaymentError::Other(e.to_string()))?;
        if used > 0 {
            return Err(PaymentError::AlreadyUsed);
        }
        let t = lookup.transfer(tx_hash, &payer).await.map_err(|e| PaymentError::Other(format!("couldn't check NEAR right now: {e}")))?.ok_or_else(|| PaymentError::UnknownTx(inv.network.clone()))?;
        if !t.success {
            return Err(PaymentError::Failed);
        }
        if !t.is_final {
            return Err(PaymentError::NotFinal);
        }
        if t.receiver_id != inv.pay_to {
            return Err(PaymentError::WrongReceiver(t.receiver_id, inv.pay_to));
        }
        if t.signer_id != payer {
            return Err(PaymentError::WrongSender(t.signer_id));
        }
        let needed: u128 = inv.amount_yocto.trunc().to_string().parse().map_err(|_| PaymentError::Other("bad invoice amount".into()))?;
        // Exactly the invoice's amount: the amount is what identifies the
        // payment as this invoice's (see the module comment).
        if t.deposit_yocto != needed {
            let too_little = t.deposit_yocto < needed;
            let (sent, needed) = (near::yocto_to_near_string(t.deposit_yocto), near::yocto_to_near_string(needed));
            return Err(if too_little { PaymentError::TooLittle { sent, needed } } else { PaymentError::NotExact { sent, needed } });
        }
        // Made after the invoice was opened, with no allowance: otherwise an
        // invoice could be opened to match a payment already on chain. A few
        // minutes are allowed at the end, and an expired invoice still
        // counts if the payment was made in time.
        if t.block_time < inv.created_at || t.block_time > inv.expires_at + Duration::minutes(5) {
            return Err(PaymentError::OutsideWindow);
        }
        let mut tx = self.db.begin().await.map_err(|e| PaymentError::Other(e.to_string()))?;
        let updated = sqlx::query("UPDATE platform.invoices SET status = 'paid', tx_hash = $2, payer = $3, paid_at = $4 WHERE id = $1 AND status <> 'paid'")
            .bind(invoice_id)
            .bind(tx_hash)
            .bind(&payer)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| if e.as_database_error().is_some_and(|d| d.is_unique_violation()) { PaymentError::AlreadyUsed } else { PaymentError::Other(e.to_string()) })?;
        if updated.rows_affected() != 1 {
            return Err(PaymentError::NotOpen("already paid".into()));
        }
        // Extend from whichever is later: now, the current paid period, or
        // the end of the trial (paying early doesn't lose trial days).
        let (paid_until,): (DateTime<Utc>,) = sqlx::query_as(
            "UPDATE platform.tenants SET paid_until = GREATEST($2, COALESCE(paid_until, $2), trial_ends_at) + make_interval(days => 30 * $3)
             WHERE id = $1 RETURNING paid_until",
        )
        .bind(tenant)
        .bind(now)
        .bind(inv.months)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| PaymentError::Other(e.to_string()))?;
        sqlx::query("INSERT INTO platform.audit_log (tenant_id, actor, action, details) VALUES ($1,$2,'billing.paid',$3)")
            .bind(tenant)
            .bind(format!("user:{}", s.user_id))
            .bind(json!({ "invoice_id": invoice_id, "tx_hash": tx_hash, "payer": payer, "deposit_yocto": t.deposit_yocto.to_string(), "paid_until": paid_until }))
            .execute(&mut *tx)
            .await
            .map_err(|e| PaymentError::Other(e.to_string()))?;
        tx.commit().await.map_err(|e| PaymentError::Other(e.to_string()))?;
        Ok(paid_until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoice_amounts_have_eight_decimals_and_carry_the_tag() {
        let near = 10u128.pow(24);
        // 28 NEAR exactly: nothing to round, the tag sits in decimals 3-8.
        assert_eq!(near::yocto_to_near_string(tagged_amount(28 * near, 1)), "28.00000001");
        assert_eq!(near::yocto_to_near_string(tagged_amount(28 * near, 999_999)), "28.00999999");
        // A price with more digits is rounded up to 0.01 NEAR first, so the
        // tag never makes the amount less than the price.
        let price = 16_333_333_000_000_000_000_000_000u128; // 16.333333 NEAR
        let amount = tagged_amount(price, 482_917);
        assert_eq!(near::yocto_to_near_string(amount), "16.34482917");
        assert!(amount > price && amount - price < 2 * PRICE_STEP_YOCTO);
        assert_eq!(amount % AMOUNT_STEP_YOCTO, 0);
        // Different tags, different amounts.
        assert_ne!(tagged_amount(price, 1), tagged_amount(price, 2));
    }

    #[test]
    fn parses_a_transfer_from_near_rpc() {
        let r = json!({
            "final_execution_status": "FINAL",
            "status": { "SuccessValue": "" },
            "transaction": { "signer_id": "alice.testnet", "receiver_id": "axom.testnet", "actions": [{ "Transfer": { "deposit": "16333000000000000000000000" } }] },
            "transaction_outcome": { "block_hash": "7d4ZbS" }
        });
        let (signer, receiver, deposit, ok, fin, block) = parse_tx(&r).unwrap();
        assert_eq!((signer.as_str(), receiver.as_str(), ok, fin, block.as_str()), ("alice.testnet", "axom.testnet", true, true, "7d4ZbS"));
        assert_eq!(deposit, 16_333_000_000_000_000_000_000_000);
        let failed = json!({ "status": { "Failure": {} }, "transaction": { "signer_id": "a.testnet", "receiver_id": "b.testnet", "actions": ["CreateAccount"] }, "transaction_outcome": { "block_hash": "x" } });
        let (_, _, deposit, ok, _, _) = parse_tx(&failed).unwrap();
        assert_eq!((deposit, ok), (0, false));
    }
}
