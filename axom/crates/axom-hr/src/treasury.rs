//! Treasury yield: the payroll float earns in Rhea lending while payroll
//! doesn't need it.
//!
//! The rule is deterministic and tied to the monthly cap. Everything payroll
//! may still pay out this month stays liquid:
//!
//!   floor  = NEAR owed on payouts not yet sent (pending approval, approved,
//!            retrying), plus what the cap still allows this month at the
//!            current NEAR price, plus `min_liquid_near`;
//!   target = floor × (1 + liquidity_margin_pct).
//!
//! Only NEAR above the target is supplied to Rhea (up to `max_deployed_pct`
//! of the treasury); the moment liquid NEAR drops below the floor (a new
//! month resets the cap's headroom, a payout gets approved, the price falls)
//! enough is withdrawn to get back to the target. Without a fresh price the
//! planner never supplies, but still withdraws to cover payouts already
//! owed. Moves keep value inside the treasury's own accounts; see
//! `axom_core::rhea`.

use axom_core::near::YOCTO_PER_NEAR;
use axom_core::rhea::{Move, MAX_LENDING_STORAGE_YOCTO};
use axom_tools::rhea::TreasuryPosition;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::PgPool;

use crate::cap;
use crate::policy::{PolicyConfig, PriceQuote, TreasuryRules};

/// Kept liquid on top of a supply's attached deposits, for gas.
pub const GAS_RESERVE_YOCTO: u128 = 50_000_000_000_000_000_000_000; // 0.05 NEAR
/// wNEAR below this is left alone (unwrapping dust costs more than it returns).
pub const UNWRAP_DUST_YOCTO: u128 = 1_000_000_000_000_000_000_000; // 0.001 NEAR
/// A Rhea balance below this is never withdrawn: interest keeps accruing on
/// what a full withdrawal leaves behind, and pulling back a speck every
/// minute would cost more gas than it returns.
pub const WITHDRAW_DUST_YOCTO: u128 = UNWRAP_DUST_YOCTO;

/// What payroll may still need, as the planner sees it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Obligations {
    /// NEAR (yocto) on payouts created but not yet sent.
    #[serde(with = "axom_core::near::yocto_str")]
    pub owed_yocto: u128,
    /// USD the monthly cap still allows this month (not yet reserved or committed).
    pub cap_headroom_usd: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    /// The one move to make now, if any.
    pub action: Option<Move>,
    pub reason: String,
    /// Liquid NEAR that must always be there (yocto). `None` without a price.
    #[serde(with = "axom_core::near::yocto_str::opt")]
    pub floor_yocto: Option<u128>,
    /// Where supplying stops (yocto). `None` without a price.
    #[serde(with = "axom_core::near::yocto_str::opt")]
    pub target_yocto: Option<u128>,
    pub position: TreasuryPosition,
    pub obligations: Obligations,
    /// The fresh NEAR price the floor was computed at (USD per NEAR), if any.
    pub usd_per_near: Option<Decimal>,
}

fn near_to_yocto(d: Decimal) -> u128 {
    if d <= Decimal::ZERO {
        return 0;
    }
    axom_core::near::near_to_yocto(d.round_dp(24)).unwrap_or(u128::MAX)
}

fn usd_to_yocto(usd: Decimal, usd_per_near: Decimal) -> u128 {
    if usd <= Decimal::ZERO || usd_per_near <= Decimal::ZERO {
        return 0;
    }
    near_to_yocto(usd / usd_per_near)
}

/// Exact amount, for what a move actually sends.
fn fmt(y: u128) -> String {
    axom_core::near::yocto_to_near_string(y)
}

const MILLI_NEAR: u128 = YOCTO_PER_NEAR / 1000;

/// A balance in a reason, rounded down to 0.001 NEAR.
fn fmt_down(y: u128) -> String {
    fmt(y / MILLI_NEAR * MILLI_NEAR)
}

/// A floor or target in a reason, rounded up to 0.001 NEAR, so a balance
/// below it never reads as equal to it.
fn fmt_up(y: u128) -> String {
    fmt(y.div_ceil(MILLI_NEAR).saturating_mul(MILLI_NEAR))
}

/// Round down to 0.001 NEAR so moves are readable on an explorer.
fn tidy(y: u128) -> u128 {
    let step = YOCTO_PER_NEAR / 1000;
    y / step * step
}

/// Decide the next move. Pure: same inputs, same answer.
///
/// `price` must already be fresh (the caller drops stale quotes).
pub fn plan(rules: &TreasuryRules, pos: &TreasuryPosition, ob: Obligations, price: Option<PriceQuote>) -> Plan {
    let mk = |action: Option<Move>, reason: String, floor: Option<u128>, target: Option<u128>| Plan {
        action,
        reason,
        floor_yocto: floor,
        target_yocto: target,
        position: pos.clone(),
        obligations: ob,
        usd_per_near: price.as_ref().map(|q| q.usd_per_near),
    };
    let min_liquid = near_to_yocto(rules.min_liquid_near);
    let min_move = near_to_yocto(rules.min_move_near);

    // The floor. Without a price, only payouts already owed are known.
    let floor_known = ob.owed_yocto.saturating_add(min_liquid);
    let (floor, target) = match price {
        Some(q) => {
            let f = floor_known.saturating_add(usd_to_yocto(ob.cap_headroom_usd, q.usd_per_near));
            let t = f.saturating_add(f / 100 * rules.liquidity_margin_pct as u128);
            (Some(f), Some(t))
        }
        None => (None, None),
    };

    // wNEAR sitting in the treasury (withdrawn from Rhea, or an intent refund) goes back to NEAR first.
    if pos.wnear_yocto >= UNWRAP_DUST_YOCTO {
        return mk(Some(Move::Unwrap { amount_yocto: pos.wnear_yocto }), format!("unwrap {} wNEAR back to liquid NEAR", fmt_down(pos.wnear_yocto)), floor, target);
    }
    if pos.extra_decimals != 0 {
        return mk(None, format!("Rhea reports extra_decimals {} for wNEAR; refusing to move (amounts would be mis-scaled)", pos.extra_decimals), floor, target);
    }

    // What can usefully come back from Rhea (dust stays).
    let withdrawable = if pos.supplied_yocto >= WITHDRAW_DUST_YOCTO { pos.supplied_yocto } else { 0 };

    // Yield turned off: bring everything home.
    if !rules.yield_enabled && withdrawable > 0 {
        return if pos.can_withdraw {
            mk(Some(Move::Withdraw { amount_yocto: withdrawable }), "treasury yield is off: withdrawing everything from Rhea".into(), floor, target)
        } else {
            mk(None, "treasury yield is off but Rhea has withdrawals paused for wNEAR".into(), floor, target)
        };
    }

    // Below the floor: withdraw back up to the target.
    let must_hold = floor.unwrap_or(floor_known);
    if pos.liquid_yocto < must_hold {
        if withdrawable == 0 {
            return mk(None, format!("liquid {} NEAR is below the {} NEAR floor and nothing is in Rhea: fund the treasury", fmt_down(pos.liquid_yocto), fmt_up(must_hold)), floor, target);
        }
        if !pos.can_withdraw {
            return mk(None, "below the liquidity floor, but Rhea has withdrawals paused for wNEAR".into(), floor, target);
        }
        let want = target.unwrap_or(must_hold).saturating_sub(pos.liquid_yocto).max(min_move);
        let amount = want.min(withdrawable);
        return mk(
            Some(Move::Withdraw { amount_yocto: amount }),
            format!("liquid {} NEAR is below the {} NEAR payroll floor: withdrawing {} NEAR from Rhea", fmt_down(pos.liquid_yocto), fmt_up(must_hold), fmt(amount)),
            floor,
            target,
        );
    }

    if !rules.yield_enabled {
        return mk(None, "treasury yield is off".into(), floor, target);
    }
    let (Some(_), Some(target)) = (floor, target) else {
        return mk(None, "no fresh NEAR price: not supplying (payouts already owed are covered)".into(), floor, target);
    };
    if !pos.can_deposit {
        return mk(None, "Rhea has deposits paused for wNEAR".into(), Some(must_hold), Some(target));
    }

    // Above the target: supply the surplus, within the deployment limit.
    let surplus = pos.liquid_yocto.saturating_sub(target).saturating_sub(GAS_RESERVE_YOCTO);
    let total = pos.liquid_yocto.saturating_add(pos.supplied_yocto);
    let room = (total / 100 * rules.max_deployed_pct as u128).saturating_sub(pos.supplied_yocto);
    let amount = tidy(surplus.min(room));
    if amount < min_move {
        return mk(
            None,
            format!("holding: {} NEAR liquid against a {} NEAR target; {} NEAR in Rhea", fmt_down(pos.liquid_yocto), fmt_up(target), fmt_down(pos.supplied_yocto)),
            Some(must_hold),
            Some(target),
        );
    }
    if !pos.registered {
        if pos.storage_min_yocto == 0 || pos.storage_min_yocto > MAX_LENDING_STORAGE_YOCTO {
            return mk(None, format!("Rhea storage minimum {} is outside the accepted bound", pos.storage_min_yocto), Some(must_hold), Some(target));
        }
        return mk(
            Some(Move::Register { storage_yocto: pos.storage_min_yocto }),
            format!("registering the treasury on Rhea lending ({} NEAR storage) before supplying", fmt(pos.storage_min_yocto)),
            Some(must_hold),
            Some(target),
        );
    }
    mk(
        Some(Move::Supply { amount_yocto: amount }),
        format!("supplying {} NEAR to Rhea: liquid {} NEAR stays above the {} NEAR target (floor {} NEAR)", fmt(amount), fmt_down(pos.liquid_yocto - amount), fmt_up(target), fmt_up(must_hold)),
        Some(must_hold),
        Some(target),
    )
}

/// Read what payroll still owes and may still spend this month.
pub async fn obligations(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<Obligations> {
    let (owed,): (Option<String>,) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount_yocto), 0)::text FROM axom.payouts
         WHERE amount_yocto IS NOT NULL
           AND (status IN ('pending_approval', 'approved') OR (status = 'failed' AND next_attempt_at IS NOT NULL))",
    )
    .fetch_one(pool)
    .await?;
    let usage = cap::usage(pool, cap::month_of(now)).await?;
    Ok(Obligations { owed_yocto: owed.and_then(|s| s.parse().ok()).unwrap_or(0), cap_headroom_usd: usage.remaining_usd() })
}

/// Drop a quote older than the policy allows.
pub fn fresh(cfg: &PolicyConfig, price: Option<PriceQuote>, now: DateTime<Utc>) -> Option<PriceQuote> {
    price.filter(|q| {
        let age = now - q.fetched_at;
        age >= chrono::Duration::zero() && age <= chrono::Duration::seconds(cfg.payouts().price_max_age_secs)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    const N: u128 = YOCTO_PER_NEAR;

    fn rules() -> TreasuryRules {
        TreasuryRules { yield_enabled: true, ..Default::default() }
    }
    fn pos(liquid: u128, supplied: u128) -> TreasuryPosition {
        TreasuryPosition {
            liquid_yocto: liquid,
            wnear_yocto: 0,
            supplied_yocto: supplied,
            registered: true,
            storage_min_yocto: N / 10,
            supply_apr: dec!(0.0032),
            can_deposit: true,
            can_withdraw: true,
            extra_decimals: 0,
        }
    }
    fn price(p: Decimal) -> Option<PriceQuote> {
        Some(PriceQuote { usd_per_near: p, fetched_at: Utc::now() })
    }
    fn ob(owed_near: u128, headroom: Decimal) -> Obligations {
        Obligations { owed_yocto: owed_near * N, cap_headroom_usd: headroom }
    }

    #[test]
    fn supplies_only_what_the_cap_cannot_spend() {
        // 1000 NEAR liquid; cap headroom 300 USD at 3 USD/NEAR = 100 NEAR; 10 NEAR owed; 1 NEAR minimum.
        let p = plan(&rules(), &pos(1000 * N, 0), ob(10, dec!(300)), price(dec!(3)));
        assert_eq!(p.floor_yocto, Some(111 * N));
        // target = floor + 25%.
        assert_eq!(p.target_yocto, Some(111 * N + 111 * N / 100 * 25));
        let Some(Move::Supply { amount_yocto }) = p.action else { panic!("{p:?}") };
        // Limited by 90% deployment: 900 NEAR max; surplus (1000 - 138.75 - 0.05) is smaller.
        assert_eq!(amount_yocto, tidy(1000 * N - (111 * N + 111 * N / 100 * 25) - GAS_RESERVE_YOCTO));
        assert!(1000 * N - amount_yocto >= p.target_yocto.unwrap(), "liquid stays at or above the target");
    }

    #[test]
    fn deployment_limit_caps_the_supply() {
        let r = TreasuryRules { max_deployed_pct: 50, ..rules() };
        let p = plan(&r, &pos(1000 * N, 0), ob(0, dec!(0)), price(dec!(3)));
        assert_eq!(p.action, Some(Move::Supply { amount_yocto: 500 * N }));
        // Already at the limit: hold.
        let p = plan(&r, &pos(500 * N, 500 * N), ob(0, dec!(0)), price(dec!(3)));
        assert_eq!(p.action, None, "{}", p.reason);
    }

    #[test]
    fn withdraws_when_payroll_needs_it_with_hysteresis() {
        // New month: headroom back to 500 USD = 166.67 NEAR at 3 USD; only 50 NEAR liquid.
        let p = plan(&rules(), &pos(50 * N, 800 * N), ob(0, dec!(500)), price(dec!(3)));
        let Some(Move::Withdraw { amount_yocto }) = p.action else { panic!("{p:?}") };
        assert_eq!(50 * N + amount_yocto, p.target_yocto.unwrap(), "withdraws back up to the target, not just the floor");
        // Between floor and target: neither supplies nor withdraws.
        let f = p.floor_yocto.unwrap();
        let mid = plan(&rules(), &pos(f + N, 800 * N), ob(0, dec!(500)), price(dec!(3)));
        assert_eq!(mid.action, None, "{}", mid.reason);
    }

    #[test]
    fn no_price_never_supplies_but_covers_owed_payouts() {
        assert_eq!(plan(&rules(), &pos(1000 * N, 0), ob(0, dec!(500)), None).action, None);
        // 60 NEAR owed, 20 liquid: withdraw at least the gap to the known floor (61).
        let p = plan(&rules(), &pos(20 * N, 300 * N), ob(60, dec!(500)), None);
        assert_eq!(p.action, Some(Move::Withdraw { amount_yocto: 41 * N }));
    }

    #[test]
    fn unwraps_first_registers_before_supplying_and_exits_when_disabled() {
        let mut p0 = pos(1000 * N, 0);
        p0.wnear_yocto = 5 * N;
        assert_eq!(plan(&rules(), &p0, ob(0, dec!(0)), price(dec!(3))).action, Some(Move::Unwrap { amount_yocto: 5 * N }));
        let mut p1 = pos(1000 * N, 0);
        p1.registered = false;
        assert_eq!(plan(&rules(), &p1, ob(0, dec!(0)), price(dec!(3))).action, Some(Move::Register { storage_yocto: N / 10 }));
        let off = TreasuryRules { yield_enabled: false, ..rules() };
        assert_eq!(plan(&off, &pos(1000 * N, 40 * N), ob(0, dec!(0)), price(dec!(3))).action, Some(Move::Withdraw { amount_yocto: 40 * N }));
        assert_eq!(plan(&off, &pos(1000 * N, 0), ob(0, dec!(0)), price(dec!(3))).action, None, "off and nothing supplied: nothing to do");
    }

    #[test]
    fn refuses_odd_contract_states() {
        let mut p = pos(1000 * N, 0);
        p.extra_decimals = 12;
        assert_eq!(plan(&rules(), &p, ob(0, dec!(0)), price(dec!(3))).action, None);
        let mut p = pos(1000 * N, 0);
        p.can_deposit = false;
        assert_eq!(plan(&rules(), &p, ob(0, dec!(0)), price(dec!(3))).action, None);
        let mut p = pos(1000 * N, 0);
        p.registered = false;
        p.storage_min_yocto = MAX_LENDING_STORAGE_YOCTO + 1;
        assert_eq!(plan(&rules(), &p, ob(0, dec!(0)), price(dec!(3))).action, None);
        // Tiny surpluses are not moved.
        assert_eq!(plan(&rules(), &pos(16 * N / 10, 0), ob(0, dec!(0)), price(dec!(3))).action, None, "0.3 NEAR surplus is under min_move");
    }

    #[test]
    fn reasons_round_to_a_thousandth_without_hiding_a_shortfall() {
        // 500 USD / 3.12 = 160.2564102… NEAR, + 1 NEAR minimum.
        let p = plan(&rules(), &pos(199_818 * N / 100_000, 0), ob(0, dec!(500)), price(dec!(3.12)));
        assert_eq!(p.reason, "liquid 1.998 NEAR is below the 161.257 NEAR floor and nothing is in Rhea: fund the treasury");
        // A balance a hair under the floor still reads as below it.
        let floor = p.floor_yocto.unwrap();
        let p = plan(&rules(), &pos(floor - 1, 0), ob(0, dec!(500)), price(dec!(3.12)));
        assert!(p.reason.starts_with("liquid 161.256 NEAR is below the 161.257 NEAR floor"), "{}", p.reason);
    }

    #[test]
    fn interest_dust_left_in_rhea_is_never_withdrawn() {
        // A full withdrawal leaves a speck of accrued interest behind.
        let dust = 528_136_906_772_004_469u128; // ~0.0000005 NEAR, seen on testnet
        let off = TreasuryRules { yield_enabled: false, ..Default::default() };
        let p = plan(&off, &pos(5 * N, dust), ob(0, dec!(2)), price(dec!(3)));
        assert_eq!(p.action, None, "{}", p.reason);
        // Below the floor with only dust in Rhea: ask for funding, don't withdraw the dust.
        let p = plan(&rules(), &pos(N, dust), ob(0, dec!(500)), price(dec!(3)));
        assert_eq!(p.action, None, "{}", p.reason);
        assert!(p.reason.contains("fund the treasury"), "{}", p.reason);
        // Anything real still comes home when yield is turned off.
        let p = plan(&off, &pos(5 * N, WITHDRAW_DUST_YOCTO), ob(0, dec!(2)), price(dec!(3)));
        assert_eq!(p.action, Some(Move::Withdraw { amount_yocto: WITHDRAW_DUST_YOCTO }));
    }
}
