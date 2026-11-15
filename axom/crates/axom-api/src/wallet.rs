//! The treasury's transaction history, paged and filtered in SQL: NEAR sent
//! to it (deposits), payouts from it, and the signer's Rhea moves, which
//! keep value inside the treasury's own accounts. Read-only.
//!
//! Payouts and moves are recorded here when they are made. Deposits come
//! from the chain: each request first asks FastNEAR's transactions API for
//! inbound transactions not seen yet (at most every `SYNC_EVERY` per
//! account) and stores them in `axom.treasury_deposits`. If that fails, the
//! history still answers from what is stored and says so.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axom_core::near;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::{admin::mfa_admin, reply, AppState};

pub const DIRECTIONS: &[&str] = &["in", "out", "internal"];
pub const KINDS: &[&str] = &["deposit", "payout", "register", "supply", "withdraw", "unwrap"];
pub const STATUSES: &[&str] = &["submitted", "succeeded", "failed", "refunded"];
pub const DEFAULT_LIMIT: i64 = 25;
pub const MAX_LIMIT: i64 = 100;

#[derive(Deserialize, Default)]
pub struct TxQuery {
    pub limit: Option<i64>,
    /// `next_cursor` from the previous page.
    pub cursor: Option<String>,
    /// Comma-separated: in, out, internal.
    pub direction: Option<String>,
    /// Comma-separated: deposit, payout, register, supply, withdraw, unwrap.
    pub kind: Option<String>,
    /// Comma-separated: submitted, succeeded, failed, refunded.
    pub status: Option<String>,
    /// From this instant (RFC 3339) or day (YYYY-MM-DD, UTC), inclusive.
    pub from: Option<String>,
    /// Before this instant or day, exclusive.
    pub to: Option<String>,
}

/// The query, checked.
#[derive(Debug, PartialEq)]
pub struct Filter {
    pub limit: i64,
    pub after: Option<(DateTime<Utc>, String)>,
    pub direction: Option<Vec<String>>,
    pub kind: Option<Vec<String>>,
    pub status: Option<Vec<String>>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

fn list(name: &str, v: &Option<String>, allowed: &[&str]) -> Result<Option<Vec<String>>, String> {
    let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    let items: Vec<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if let Some(bad) = items.iter().find(|i| !allowed.contains(&i.as_str())) {
        return Err(format!("unknown {name} '{bad}' (use {})", allowed.join(", ")));
    }
    Ok(Some(items))
}

fn instant(name: &str, v: &Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    if let Ok(t) = DateTime::parse_from_rfc3339(v) {
        return Ok(Some(t.with_timezone(&Utc)));
    }
    NaiveDate::parse_from_str(v, "%Y-%m-%d")
        .map(|d| Some(d.and_hms_opt(0, 0, 0).unwrap().and_utc()))
        .map_err(|_| format!("{name} must be a date (YYYY-MM-DD) or an RFC 3339 time"))
}

/// A page boundary: the last row's time and id, opaque to the client.
pub fn encode_cursor(at: DateTime<Utc>, id: &str) -> String {
    hex::encode(format!("{}|{id}", at.timestamp_micros()))
}

pub fn decode_cursor(c: &str) -> Option<(DateTime<Utc>, String)> {
    let raw = String::from_utf8(hex::decode(c).ok()?).ok()?;
    let (micros, id) = raw.split_once('|')?;
    let at = DateTime::from_timestamp_micros(micros.parse().ok()?)?;
    (!id.is_empty()).then(|| (at, id.to_string()))
}

impl Filter {
    pub fn parse(q: &TxQuery) -> Result<Self, String> {
        let limit = q.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(format!("limit must be 1 to {MAX_LIMIT}"));
        }
        let after = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
            None => None,
            Some(c) => Some(decode_cursor(c).ok_or("invalid cursor")?),
        };
        let (from, to) = (instant("from", &q.from)?, instant("to", &q.to)?);
        if let (Some(f), Some(t)) = (from, to) {
            if t <= f {
                return Err("'to' must be after 'from'".into());
            }
        }
        Ok(Self {
            limit,
            after,
            direction: list("direction", &q.direction, DIRECTIONS)?,
            kind: list("kind", &q.kind, KINDS)?,
            status: list("status", &q.status, STATUSES)?,
            from,
            to,
        })
    }
}

/// Every transaction of the treasury on `$1` (deposits to account `$2`, or
/// any account when `$2` is null), one row shape, filtered by `$3`..`$7`.
const LEDGER: &str = r#"
WITH ledger AS (
    SELECT 'deposit:' || d.tx_hash AS id, 'deposit' AS kind, 'in' AS direction,
           d.sender AS counterparty, NULL::text AS label, NULL::text AS note, NULL::text AS error,
           d.amount_yocto, d.status, d.tx_hash, d.created_at AS at
    FROM axom.treasury_deposits d
    WHERE d.network = $1 AND ($2::text IS NULL OR d.account = $2) AND d.amount_yocto > 0
    UNION ALL
    SELECT 'payout:' || p.id, 'payout', 'out',
           p.recipient, w.name,
           CASE WHEN p.dest_chain IS NOT NULL THEN 'to ' || p.dest_asset || ' on ' || p.dest_chain END,
           NULLIF(p.last_error, ''),
           COALESCE(p.amount_yocto, 0),
           CASE p.status WHEN 'finalized' THEN 'succeeded' WHEN 'failed' THEN 'failed' WHEN 'refunded' THEN 'refunded' ELSE 'submitted' END,
           p.tx_hash, COALESCE(p.sent_at, p.created_at)
    FROM axom.payouts p LEFT JOIN axom.workers w ON w.id = p.worker_id
    WHERE p.network = $1 AND p.tx_hash IS NOT NULL
    UNION ALL
    SELECT 'move:' || m.id, m.kind, 'internal', m.contract, NULL, m.reason, m.error,
           m.amount_yocto, m.status, m.tx_hash, m.created_at
    FROM axom.treasury_moves m
    WHERE m.network = $1
)
SELECT * FROM ledger
WHERE ($3::text[] IS NULL OR direction = ANY($3))
  AND ($4::text[] IS NULL OR kind = ANY($4))
  AND ($5::text[] IS NULL OR status = ANY($5))
  AND ($6::timestamptz IS NULL OR at >= $6)
  AND ($7::timestamptz IS NULL OR at < $7)
"#;

type Row = (String, String, String, String, Option<String>, Option<String>, Option<String>, String, String, String, DateTime<Utc>);

/// One page of the ledger, newest first, plus the filtered count and the
/// succeeded totals in and out.
pub async fn page(pool: &PgPool, network: &str, account: Option<&str>, f: &Filter) -> Result<Value, sqlx::Error> {
    // The page: keyset on (at, id), one extra row to know there is more.
    static PAGE: OnceLock<String> = OnceLock::new();
    let sql = PAGE.get_or_init(|| {
        LEDGER.replace("SELECT * FROM ledger", "SELECT id, kind, direction, counterparty, label, note, error, amount_yocto::text, status, tx_hash, at FROM ledger")
            + " AND ($8::timestamptz IS NULL OR (at, id) < ($8, $9)) ORDER BY at DESC, id DESC LIMIT $10"
    });
    let mut rows: Vec<Row> = sqlx::query_as(sql)
        .bind(network)
        .bind(account)
        .bind(&f.direction)
        .bind(&f.kind)
        .bind(&f.status)
        .bind(f.from)
        .bind(f.to)
        .bind(f.after.as_ref().map(|a| a.0))
        .bind(f.after.as_ref().map(|a| a.1.clone()))
        .bind(f.limit + 1)
        .fetch_all(pool)
        .await?;
    let more = rows.len() as i64 > f.limit;
    rows.truncate(f.limit as usize);
    let next = more.then(|| rows.last().map(|r| encode_cursor(r.10, &r.0))).flatten();

    // The count and totals: the same filters, without the cursor.
    static TOTALS: OnceLock<String> = OnceLock::new();
    let totals_sql = TOTALS.get_or_init(|| {
        LEDGER.replace(
            "SELECT * FROM ledger",
            "SELECT count(*), COALESCE(SUM(amount_yocto) FILTER (WHERE direction = 'in' AND status = 'succeeded'), 0)::text,
                    COALESCE(SUM(amount_yocto) FILTER (WHERE direction = 'out' AND status = 'succeeded'), 0)::text FROM ledger",
        )
    });
    let (total, inn, out): (i64, String, String) = sqlx::query_as(totals_sql)
        .bind(network)
        .bind(account)
        .bind(&f.direction)
        .bind(&f.kind)
        .bind(&f.status)
        .bind(f.from)
        .bind(f.to)
        .fetch_one(pool)
        .await?;
    let y = |s: &str| s.parse::<u128>().unwrap_or(0);
    let explorer = |hash: &str| format!("https://{}nearblocks.io/txns/{hash}", if network == "mainnet" { "" } else { "testnet." });
    Ok(json!({
        "transactions": rows.into_iter().map(|r| json!({
            "id": r.0, "kind": r.1, "direction": r.2, "counterparty": r.3, "label": r.4, "note": r.5, "error": r.6,
            "amount_near": near::yocto_to_near_string(y(&r.7)), "amount_yocto": r.7,
            "status": r.8, "explorer_url": explorer(&r.9), "tx_hash": r.9, "at": r.10,
        })).collect::<Vec<_>>(),
        "next_cursor": next,
        "total": total,
        "totals": { "in_near": near::yocto_to_near_string(y(&inn)), "out_near": near::yocto_to_near_string(y(&out)) },
    }))
}

// ---------------------------------------------------------------- deposits

/// A transfer to the treasury found in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deposit {
    pub tx_hash: String,
    pub sender: String,
    /// 0 when the transaction moved no NEAR to the treasury.
    pub amount_yocto: u128,
    pub succeeded: bool,
    pub block_height: i64,
    pub at: DateTime<Utc>,
}

/// What one transaction (FastNEAR's `/v0/transactions` shape) sent to
/// `account`: native transfers received by it from anyone but itself and the
/// protocol (`system` receipts are gas refunds). A failed transfer was never
/// received, so it counts only when nothing succeeded.
pub fn parse_deposit(tx: &Value, account: &str) -> Option<Deposit> {
    let hash = tx.pointer("/transaction/hash").and_then(Value::as_str)?.to_string();
    let signer = tx.pointer("/transaction/signer_id").and_then(Value::as_str).unwrap_or_default();
    let (mut ok, mut failed, mut sender, mut first) = (0u128, 0u128, None::<String>, None::<(i64, i64)>);
    for r in tx.get("receipts").and_then(Value::as_array).into_iter().flatten() {
        let rc = &r["receipt"];
        let pred = rc["predecessor_id"].as_str().unwrap_or_default();
        if rc["receiver_id"].as_str() != Some(account) || pred == "system" || pred == account {
            continue;
        }
        let amount: u128 = rc
            .pointer("/receipt/Action/actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| a.pointer("/Transfer/deposit").and_then(Value::as_str)?.parse::<u128>().ok())
            .sum();
        if amount == 0 {
            continue;
        }
        let failure = r.pointer("/execution_outcome/outcome/status/Failure").is_some();
        if failure { failed += amount } else { ok += amount }
        sender.get_or_insert_with(|| pred.to_string());
        let (h, ts) = (rc["block_height"].as_i64().unwrap_or(0), rc["block_timestamp"].as_i64().unwrap_or(0));
        first = Some(first.map_or((h, ts), |f| f.min((h, ts))));
    }
    let (height, ts) = first.unwrap_or_else(|| {
        let o = &tx["execution_outcome"];
        (o["block_height"].as_i64().unwrap_or(0), o["block_timestamp"].as_i64().unwrap_or(0))
    });
    Some(Deposit {
        tx_hash: hash,
        sender: sender.unwrap_or_else(|| signer.to_string()),
        amount_yocto: if ok > 0 { ok } else { failed },
        succeeded: ok > 0 || failed == 0,
        block_height: height,
        at: DateTime::from_timestamp_nanos(ts),
    })
}

/// How often the chain is asked, per account; and how far back one sync
/// reads (pages of 200 transactions, newest first).
const SYNC_EVERY: Duration = Duration::from_secs(30);
const MAX_PAGES: usize = 5;

async fn post(client: &reqwest::Client, url: String, body: Value) -> Result<Value, String> {
    let r = client.post(&url).json(&body).send().await.map_err(|e| format!("can't reach the NEAR transactions API: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the NEAR transactions API answered HTTP {}", r.status().as_u16()));
    }
    r.json().await.map_err(|_| "the NEAR transactions API gave an unexpected answer".to_string())
}

/// Store deposits to `account` that aren't stored yet. Returns how many
/// transactions were added.
pub async fn sync_deposits(pool: &PgPool, tx_api: &str, network: &str, account: &str) -> Result<usize, String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().map_err(|e| e.to_string())?;
    let base = tx_api.trim_end_matches('/');
    let db = |e: sqlx::Error| e.to_string();
    // Inbound transactions (signed by someone else), newest first, until a
    // page reaches one already stored.
    let mut new: Vec<String> = Vec::new();
    let mut resume: Option<Value> = None;
    for _ in 0..MAX_PAGES {
        let mut body = json!({ "account_id": account, "desc": true, "limit": 200 });
        if let Some(r) = &resume {
            body["resume_token"] = r.clone();
        }
        let v = post(&client, format!("{base}/v0/account"), body).await?;
        let inbound: Vec<String> = v["account_txs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["is_signer"] == json!(false))
            .filter_map(|t| t["transaction_hash"].as_str().map(str::to_string))
            .collect();
        let known: Vec<String> =
            sqlx::query_scalar("SELECT tx_hash FROM axom.treasury_deposits WHERE tx_hash = ANY($1)").bind(&inbound).fetch_all(pool).await.map_err(db)?;
        let known: HashSet<String> = known.into_iter().collect();
        let reached_known = !known.is_empty();
        new.extend(inbound.into_iter().filter(|h| !known.contains(h)));
        resume = v.get("resume_token").filter(|r| !r.is_null()).cloned();
        if reached_known || resume.is_none() {
            break;
        }
    }
    let mut found = Vec::new();
    for chunk in new.chunks(20) {
        let v = post(&client, format!("{base}/v0/transactions"), json!({ "tx_hashes": chunk })).await?;
        found.extend(v["transactions"].as_array().into_iter().flatten().filter_map(|t| parse_deposit(t, account)));
    }
    // All at once, so a sync that fails part way leaves no gap behind the
    // newest stored deposit.
    let mut tx = pool.begin().await.map_err(db)?;
    for d in &found {
        sqlx::query(
            "INSERT INTO axom.treasury_deposits (tx_hash, network, account, sender, amount_yocto, status, block_height, created_at)
             VALUES ($1, $2, $3, $4, $5::numeric, $6, $7, $8) ON CONFLICT (tx_hash) DO NOTHING",
        )
        .bind(&d.tx_hash)
        .bind(network)
        .bind(account)
        .bind(&d.sender)
        .bind(d.amount_yocto.to_string())
        .bind(if d.succeeded { "succeeded" } else { "failed" })
        .bind(d.block_height)
        .bind(d.at)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    }
    tx.commit().await.map_err(db)?;
    Ok(found.len())
}

/// Sync unless this account was synced in the last `SYNC_EVERY`; one sync
/// at a time per account. Returns when it last succeeded and any error.
async fn sync_throttled(pool: &PgPool, tx_api: &str, network: &str, account: &str) -> (Option<DateTime<Utc>>, Option<String>) {
    #[derive(Default)]
    struct Entry {
        ok_at: Option<(DateTime<Utc>, Instant)>,
        tried: Option<Instant>,
        error: Option<String>,
    }
    static STATE: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<Entry>>>>> = OnceLock::new();
    let key = format!("{tx_api} {network} {account}");
    let entry = STATE.get_or_init(Default::default).lock().unwrap().entry(key).or_default().clone();
    let mut e = entry.lock().await;
    if e.tried.is_some_and(|t| t.elapsed() < SYNC_EVERY) {
        return (e.ok_at.map(|o| o.0), e.error.clone());
    }
    e.tried = Some(Instant::now());
    match sync_deposits(pool, tx_api, network, account).await {
        Ok(_) => {
            e.ok_at = Some((Utc::now(), Instant::now()));
            e.error = None;
        }
        Err(err) => {
            tracing::warn!(error = %err, "wallet: deposit sync failed");
            e.error = Some(err);
        }
    }
    (e.ok_at.map(|o| o.0), e.error.clone())
}

/// `GET /admin/treasury/transactions`: MFA-verified admins only. Query:
/// `limit` (1-100, default 25), `cursor`, `direction`, `kind`, `status`
/// (comma-separated), `from`, `to`.
pub async fn transactions(State(s): State<AppState>, headers: HeaderMap, Query(q): Query<TxQuery>) -> Response {
    if let Err(r) = mfa_admin(&s, &headers).await {
        return *r;
    }
    let f = match Filter::parse(&q) {
        Ok(f) => f,
        Err(e) => return reply(StatusCode::BAD_REQUEST, json!({ "error": e })),
    };
    let t = &s.treasury;
    let (account, account_error) = match crate::treasury::account_of(t).await {
        Ok(a) => (a, None),
        Err(e) => (None, Some(e)),
    };
    let (synced_at, sync_error) = match &account {
        Some(a) => sync_throttled(&s.pool, &t.tx_api_url, &t.network, a).await,
        None => (None, account_error.map(|e| format!("deposits can't be read: {e}"))),
    };
    match page(&s.pool, &t.network, account.as_deref(), &f).await {
        Ok(mut v) => {
            v["network"] = json!(t.network);
            v["account"] = json!(account);
            v["deposits"] = json!({ "synced_at": synced_at, "error": sync_error });
            reply(StatusCode::OK, v)
        }
        Err(e) => crate::ApiError(e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> TxQuery {
        let mut q = TxQuery::default();
        for (k, v) in pairs {
            let v = Some(v.to_string());
            match *k {
                "limit" => q.limit = v.and_then(|s| s.parse().ok()),
                "cursor" => q.cursor = v,
                "direction" => q.direction = v,
                "kind" => q.kind = v,
                "status" => q.status = v,
                "from" => q.from = v,
                "to" => q.to = v,
                _ => unreachable!(),
            }
        }
        q
    }

    #[test]
    fn cursors_round_trip_and_reject_junk() {
        let at = DateTime::from_timestamp_micros(1_794_719_666_594_549).unwrap();
        let c = encode_cursor(at, "move:abc");
        assert_eq!(decode_cursor(&c), Some((at, "move:abc".to_string())));
        assert_eq!(decode_cursor("zz"), None);
        assert_eq!(decode_cursor(&hex::encode("12|")), None);
        assert_eq!(decode_cursor(&hex::encode("x|id")), None);
    }

    #[test]
    fn filters_are_checked() {
        let f = Filter::parse(&q(&[])).unwrap();
        assert_eq!((f.limit, f.direction, f.after), (25, None, None));
        let f = Filter::parse(&q(&[("direction", "in, out"), ("kind", "deposit"), ("status", "succeeded"), ("from", "2026-11-01"), ("to", "2026-11-15T10:00:00+01:00")])).unwrap();
        assert_eq!(f.direction, Some(vec!["in".into(), "out".into()]));
        assert_eq!(f.kind, Some(vec!["deposit".into()]));
        assert_eq!(f.from.unwrap().to_rfc3339(), "2026-11-01T00:00:00+00:00");
        assert_eq!(f.to.unwrap().to_rfc3339(), "2026-11-15T09:00:00+00:00");
        // Empty values mean no filter.
        assert_eq!(Filter::parse(&q(&[("direction", ""), ("cursor", "")])).unwrap().direction, None);
        for bad in [[("direction", "sideways")], [("kind", "deposit,mint")], [("status", "done")], [("limit", "0")], [("limit", "101")], [("cursor", "nothex")], [("from", "yesterday")]] {
            assert!(Filter::parse(&q(&bad)).is_err(), "{bad:?}");
        }
        assert!(Filter::parse(&q(&[("from", "2026-11-02"), ("to", "2026-11-01")])).is_err());
    }

    fn receipt(pred: &str, recv: &str, deposit: &str, failed: bool, height: i64) -> Value {
        let status = if failed { json!({ "Failure": { "ActionError": {} } }) } else { json!({ "SuccessValue": "" }) };
        json!({
            "execution_outcome": { "outcome": { "status": status } },
            "receipt": { "predecessor_id": pred, "receiver_id": recv, "block_height": height, "block_timestamp": 1_794_719_667_140_866_528i64,
                         "receipt": { "Action": { "actions": [{ "Transfer": { "deposit": deposit } }] } } }
        })
    }

    #[test]
    fn deposits_are_read_from_transactions() {
        let tx = |receipts: Vec<Value>| json!({ "transaction": { "hash": "H", "signer_id": "alice.testnet" }, "execution_outcome": { "block_height": 9, "block_timestamp": 1 }, "receipts": receipts });
        // A plain transfer, as the chain reported it for the local treasury.
        let d = parse_deposit(&tx(vec![receipt("alice.testnet", "t", "2000000000000000000000000", false, 10)]), "t").unwrap();
        assert_eq!((d.sender.as_str(), d.amount_yocto, d.succeeded, d.block_height), ("alice.testnet", 2 * 10u128.pow(24), true, 10));
        assert_eq!(d.at.timestamp(), 1_794_719_667);
        // Gas refunds, self-receipts and transfers to others don't count.
        let d = parse_deposit(&tx(vec![receipt("system", "t", "5", false, 10), receipt("t", "t", "6", false, 10), receipt("alice.testnet", "bob", "7", false, 10)]), "t").unwrap();
        assert_eq!(d.amount_yocto, 0);
        // A failed transfer is recorded as failed, with what was attempted.
        let d = parse_deposit(&tx(vec![receipt("alice.testnet", "t", "8", true, 10)]), "t").unwrap();
        assert_eq!((d.amount_yocto, d.succeeded), (8, false));
        // A contract forwarding NEAR: the sender is whoever sent the receipt.
        let d = parse_deposit(&tx(vec![receipt("wrap.testnet", "t", "3", false, 11), receipt("wrap.testnet", "t", "4", false, 12)]), "t").unwrap();
        assert_eq!((d.sender.as_str(), d.amount_yocto, d.block_height), ("wrap.testnet", 7, 11));
        assert!(parse_deposit(&json!({}), "t").is_none());
    }
}
