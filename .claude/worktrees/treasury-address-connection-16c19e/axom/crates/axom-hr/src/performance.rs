//! Performance metrics, review thresholds, the firing rule, and spot-check
//! sampling. Pure functions; the database side lives in [`crate::reviews`].
//!
//! Metrics come from task records, never from LLM summaries. Only objective
//! metrics (completion, on-time, cost, objective quality checks) decide
//! whether a review is below threshold; the LLM quality score is recorded for
//! information and can never trigger a firing.

use std::path::Path;

use chrono::Duration;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewRules {
    pub every_finished_tasks: i64,
    pub period_days: i64,
    pub spot_check_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub min_completion_rate: f64,
    pub min_on_time_rate: f64,
    pub max_cost_per_completed_usd: Decimal,
    pub min_objective_quality: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiringRules {
    pub min_completed_tasks: i64,
    pub min_data_days: i64,
    pub consecutive_below_periods: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerformanceFile {
    pub version: String,
    pub review: ReviewRules,
    pub thresholds: Thresholds,
    pub firing: FiringRules,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PerformanceConfig {
    pub file: PerformanceFile,
    version: String,
}

impl PerformanceConfig {
    pub fn from_file(file: PerformanceFile) -> anyhow::Result<Self> {
        let r = &file.review;
        anyhow::ensure!(r.every_finished_tasks > 0 && r.period_days > 0, "review cadence must be positive");
        anyhow::ensure!((0.0..=1.0).contains(&r.spot_check_rate), "spot_check_rate must be in 0..=1");
        anyhow::ensure!(file.firing.consecutive_below_periods >= 1, "consecutive_below_periods must be >= 1");
        let hash = hex::encode(Sha256::digest(serde_json::to_vec(&file)?));
        let version = format!("{}+{}", file.version, &hash[..8]);
        Ok(Self { file, version })
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Self::from_file(toml::from_str(text)?)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn period(&self) -> Duration {
        Duration::days(self.file.review.period_days)
    }
}

/// Metrics for one worker over one window, computed from task records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub completed: i64,
    pub failed: i64,
    /// Completed tasks finished on or before their deadline (no deadline = on time).
    pub on_time: i64,
    pub total_cost_usd: Decimal,
    pub objective_checks: i64,
    pub objective_passed: i64,
    /// Mean LLM quality score over judged tasks, if any.
    pub llm_quality: Option<f64>,
}

impl Metrics {
    pub fn finished(&self) -> i64 {
        self.completed + self.failed
    }

    pub fn completion_rate(&self) -> Option<f64> {
        (self.finished() > 0).then(|| self.completed as f64 / self.finished() as f64)
    }

    pub fn on_time_rate(&self) -> Option<f64> {
        (self.completed > 0).then(|| self.on_time as f64 / self.completed as f64)
    }

    pub fn cost_per_completed_usd(&self) -> Option<Decimal> {
        (self.completed > 0).then(|| (self.total_cost_usd / Decimal::from(self.completed)).round_dp(6))
    }

    pub fn objective_quality(&self) -> Option<f64> {
        (self.objective_checks > 0).then(|| self.objective_passed as f64 / self.objective_checks as f64)
    }
}

/// Result of checking metrics against thresholds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assessment {
    pub objective_below: bool,
    pub reasons: Vec<String>,
}

/// Compare objective metrics to thresholds. Metrics without data are skipped;
/// a window with no finished tasks is never below threshold.
pub fn assess(m: &Metrics, t: &Thresholds) -> Assessment {
    let mut reasons = Vec::new();
    if let Some(r) = m.completion_rate() {
        if r < t.min_completion_rate {
            reasons.push(format!("completion rate {r:.2} < {:.2}", t.min_completion_rate));
        }
    }
    if let Some(r) = m.on_time_rate() {
        if r < t.min_on_time_rate {
            reasons.push(format!("on-time rate {r:.2} < {:.2}", t.min_on_time_rate));
        }
    }
    if let Some(c) = m.cost_per_completed_usd() {
        if c > t.max_cost_per_completed_usd {
            reasons.push(format!("cost per completed task {c} USD > {} USD", t.max_cost_per_completed_usd));
        }
    }
    if let Some(q) = m.objective_quality() {
        if q < t.min_objective_quality {
            reasons.push(format!("objective quality {q:.2} < {:.2}", t.min_objective_quality));
        }
    }
    // A completed-zero window with failures is caught by completion rate;
    // the LLM score is deliberately not consulted.
    Assessment { objective_below: !reasons.is_empty(), reasons }
}

/// Inputs to the firing rule.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FiringInputs {
    /// Completed tasks over the worker's whole history.
    pub completed_total: i64,
    /// Days from the worker's first finished task to now.
    pub data_days: f64,
    /// `objective_below` of the most recent periodic reviews, newest first.
    pub recent_below: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FiringVerdict {
    pub fire: bool,
    pub reason: String,
}

/// Fire only if there are enough completed tasks AND enough days of data AND
/// the last N periodic reviews were all below threshold on objective metrics.
pub fn firing_verdict(inputs: &FiringInputs, rules: &FiringRules) -> FiringVerdict {
    let n = rules.consecutive_below_periods;
    let no = |reason: String| FiringVerdict { fire: false, reason };
    if inputs.completed_total < rules.min_completed_tasks {
        return no(format!("only {} completed tasks (need {})", inputs.completed_total, rules.min_completed_tasks));
    }
    if inputs.data_days < rules.min_data_days as f64 {
        return no(format!("only {:.1} days of data (need {})", inputs.data_days, rules.min_data_days));
    }
    if inputs.recent_below.len() < n || !inputs.recent_below.iter().take(n).all(|b| *b) {
        return no(format!("not below threshold for {n} consecutive review periods"));
    }
    FiringVerdict {
        fire: true,
        reason: format!(
            "objective metrics below threshold for {n} consecutive review periods, with {} completed tasks over {:.1} days",
            inputs.completed_total, inputs.data_days
        ),
    }
}

/// Deterministic sampling from the review id (a random v4 UUID): selects a
/// review for the owner spot-check queue with probability `rate`.
pub fn spot_check_selected(review_id: Uuid, rate: f64) -> bool {
    let digest = Sha256::digest(review_id.as_bytes());
    let x = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    (x as f64 / u64::MAX as f64) < rate
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    pub(crate) fn rules() -> PerformanceConfig {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/performance.toml");
        PerformanceConfig::load(&path).unwrap()
    }

    #[test]
    fn metric_math() {
        let m = Metrics { completed: 8, failed: 2, on_time: 6, total_cost_usd: dec!(12), objective_checks: 4, objective_passed: 3, llm_quality: Some(0.9) };
        assert_eq!(m.completion_rate(), Some(0.8));
        assert_eq!(m.on_time_rate(), Some(0.75));
        assert_eq!(m.cost_per_completed_usd(), Some(dec!(1.5)));
        assert_eq!(m.objective_quality(), Some(0.75));
        assert_eq!(Metrics::default().completion_rate(), None);
    }

    #[test]
    fn assess_uses_objective_metrics_only() {
        let t = rules().file.thresholds;
        let good = Metrics { completed: 10, on_time: 10, total_cost_usd: dec!(10), llm_quality: Some(0.05), ..Default::default() };
        assert!(!assess(&good, &t).objective_below, "a terrible LLM score alone is not below threshold");
        let slow = Metrics { completed: 10, on_time: 2, ..Default::default() };
        let a = assess(&slow, &t);
        assert!(a.objective_below);
        assert!(a.reasons[0].contains("on-time"));
        let costly = Metrics { completed: 2, on_time: 2, total_cost_usd: dec!(20), ..Default::default() };
        assert!(assess(&costly, &t).objective_below);
        let failing_tests = Metrics { completed: 5, on_time: 5, objective_checks: 5, objective_passed: 1, ..Default::default() };
        assert!(assess(&failing_tests, &t).objective_below);
        assert!(!assess(&Metrics::default(), &t).objective_below, "no data is not below");
    }

    #[test]
    fn firing_rule() {
        let r = rules().file.firing;
        let base = FiringInputs { completed_total: 12, data_days: 15.0, recent_below: vec![true, true] };
        assert!(firing_verdict(&base, &r).fire);
        // 10 completed over 3 days: no.
        assert!(!firing_verdict(&FiringInputs { completed_total: 10, data_days: 3.0, ..base.clone() }, &r).fire);
        // 14 days with 5 completed: no.
        assert!(!firing_verdict(&FiringInputs { completed_total: 5, data_days: 14.0, ..base.clone() }, &r).fire);
        // One below period: no.
        assert!(!firing_verdict(&FiringInputs { recent_below: vec![true], ..base.clone() }, &r).fire);
        assert!(!firing_verdict(&FiringInputs { recent_below: vec![true, false, true], ..base.clone() }, &r).fire);
        // Boundary: exactly 10 completed and 14 days.
        assert!(firing_verdict(&FiringInputs { completed_total: 10, data_days: 14.0, ..base }, &r).fire);
    }

    #[test]
    fn spot_check_rate_is_about_ten_percent() {
        let n = 20_000;
        let picked = (0..n).filter(|_| spot_check_selected(Uuid::new_v4(), 0.10)).count();
        let frac = picked as f64 / n as f64;
        assert!((0.09..=0.11).contains(&frac), "selected {frac}");
        assert!(!spot_check_selected(Uuid::new_v4(), 0.0));
        assert!(spot_check_selected(Uuid::new_v4(), 1.0));
    }
}
