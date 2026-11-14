//! A company's own rules, which can only be stricter than the platform's
//! (`config/policy.toml`):
//! - a lower payout approval threshold (more payouts wait for a human);
//! - a shorter retention for data of candidates who weren't hired.
//!
//! The effective policy is the platform's with these applied. The runtime
//! and the signer both read it from the company's database, so the
//! signer's re-check of automatic payouts uses the same rules. Changing it is
//! the protected `ChangePolicy` action (MFA admin), audited before and after.

use axom_core::audit::{self, AuditEvent};
use axom_core::decision::Outcome;
use axom_core::domain::Actor;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

use crate::policy::{self, Action, Context, PolicyConfig};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct CompanyRules {
    /// Lower than the platform's, or `None` to use the platform's.
    pub approval_threshold_usd: Option<Decimal>,
    /// Shorter than the platform's, or `None` to use the platform's.
    pub candidate_retention_days: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum CompanyRulesError {
    #[error("not allowed: {0}")]
    Denied(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The company's stored rules (none set: all `None`).
pub async fn load(pool: &PgPool) -> Result<(CompanyRules, Option<(String, DateTime<Utc>)>), sqlx::Error> {
    type Row = (Option<Decimal>, Option<i32>, String, DateTime<Utc>);
    let r: Option<Row> =
        sqlx::query_as("SELECT approval_threshold_usd, candidate_retention_days, updated_by, updated_at FROM axom.company_policy WHERE id = 1").fetch_optional(pool).await?;
    Ok(match r {
        Some((t, d, by, at)) => (CompanyRules { approval_threshold_usd: t, candidate_retention_days: d }, Some((by, at))),
        None => (CompanyRules::default(), None),
    })
}

/// The platform policy with the company's stricter rules applied. Values
/// that aren't stricter are ignored, so a stored value can never loosen the
/// platform's (for example after the platform lowers its own threshold).
pub fn apply(base: &PolicyConfig, rules: &CompanyRules) -> PolicyConfig {
    let mut file = base.file.clone();
    if let Some(t) = rules.approval_threshold_usd {
        if t > Decimal::ZERO && t < file.payouts.approval_threshold_usd {
            file.payouts.approval_threshold_usd = t;
        }
    }
    if let Some(d) = rules.candidate_retention_days {
        if d >= 1 && i64::from(d) < file.privacy.candidate_retention_days {
            file.privacy.candidate_retention_days = i64::from(d);
        }
    }
    if file == base.file {
        return base.clone();
    }
    PolicyConfig::from_file(file).unwrap_or_else(|_| base.clone())
}

/// The effective policy for the company whose database this is. Falls back
/// to the platform's on a read error (never looser than the platform).
pub async fn effective(base: &PolicyConfig, pool: &PgPool) -> PolicyConfig {
    match load(pool).await {
        Ok((rules, _)) => apply(base, &rules),
        Err(e) => {
            tracing::warn!(error = %e, "company rules unreadable; using the platform's");
            base.clone()
        }
    }
}

/// Set the company's rules (MFA admin only). Each value must be stricter
/// than the platform's, or `None` to use the platform's.
pub async fn set(pool: &PgPool, base: &PolicyConfig, actor: &Actor, rules: &CompanyRules) -> Result<PolicyConfig, CompanyRulesError> {
    let d = policy::evaluate(base, &Action::ChangePolicy, &Context::new(actor.clone(), Utc::now()));
    if d.outcome != Outcome::Allow {
        return Err(CompanyRulesError::Denied(d.reason));
    }
    let platform_threshold = base.payouts().approval_threshold_usd;
    if let Some(t) = rules.approval_threshold_usd {
        if t <= Decimal::ZERO || t > platform_threshold {
            return Err(CompanyRulesError::Invalid(format!("the approval threshold must be above $0 and at most the platform's ${platform_threshold}")));
        }
    }
    let platform_retention = base.file.privacy.candidate_retention_days;
    if let Some(days) = rules.candidate_retention_days {
        if days < 1 || i64::from(days) > platform_retention {
            return Err(CompanyRulesError::Invalid(format!("candidate data retention must be 1 to {platform_retention} days")));
        }
    }
    let (before, _) = load(pool).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO axom.company_policy (id, approval_threshold_usd, candidate_retention_days, updated_by, updated_at) VALUES (1,$1,$2,$3,now())
         ON CONFLICT (id) DO UPDATE SET approval_threshold_usd = EXCLUDED.approval_threshold_usd,
           candidate_retention_days = EXCLUDED.candidate_retention_days, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(rules.approval_threshold_usd)
    .bind(rules.candidate_retention_days)
    .bind(actor.to_string())
    .execute(&mut *tx)
    .await?;
    let eff = apply(base, rules);
    let ev = AuditEvent::new(actor, "policy.company_rules", "changed")
        .inputs(json!({ "before": before, "after": rules, "effective_version": eff.version() }))
        .policy_version(&d.policy_version);
    audit::append(&mut *tx, &ev).await?;
    tx.commit().await?;
    Ok(eff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn base() -> PolicyConfig {
        PolicyConfig::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../config/policy.toml")).unwrap()).unwrap()
    }

    #[test]
    fn only_stricter_values_apply() {
        let b = base();
        assert_eq!(apply(&b, &CompanyRules::default()).version(), b.version(), "nothing set: the platform's policy, same version");
        let stricter = apply(&b, &CompanyRules { approval_threshold_usd: Some(dec!(20)), candidate_retention_days: Some(30) });
        assert_eq!(stricter.payouts().approval_threshold_usd, dec!(20));
        assert_eq!(stricter.file.privacy.candidate_retention_days, 30);
        assert_ne!(stricter.version(), b.version(), "decisions record that company rules applied");
        let looser = apply(&b, &CompanyRules { approval_threshold_usd: Some(dec!(5000)), candidate_retention_days: Some(9999) });
        assert_eq!(looser.payouts().approval_threshold_usd, dec!(50));
        assert_eq!(looser.file.privacy.candidate_retention_days, 180);
    }
}
