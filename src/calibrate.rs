//! Half-life calibration: learn a source's decay physics from the log.
//!
//! Half-lives are not configuration — they are empirical claims about how
//! fast a source's statements go stale. When the log later receives
//! ground truth (axiomatic or near-axiomatic evidence) about a slot a
//! soft source made a claim about, that pair (claim age at truth time,
//! was the claim right?) is a calibration observation. Maximum likelihood
//! over the decay model
//!
//! ```text
//! P(correct | age) = r0 * 0.5 ^ (age / half_life)
//! ```
//!
//! recovers r0 and the half-life per source. Grid search: deterministic,
//! dependency-free, and honest about its resolution.

use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::evidence::{Claim, Evidence};
use crate::time::SECONDS_PER_DAY;

const EPS: f64 = 1e-9;

pub const HALF_LIFE_GRID: [Option<f64>; 15] = [
    Some(7.0),
    Some(14.0),
    Some(30.0),
    Some(45.0),
    Some(60.0),
    Some(90.0),
    Some(120.0),
    Some(180.0),
    Some(270.0),
    Some(365.0),
    Some(540.0),
    Some(730.0),
    Some(1460.0),
    Some(2920.0),
    None,
];

#[derive(Clone, Debug, Serialize)]
pub struct FittedDecay {
    pub source_name: String,
    pub r0: f64,
    pub half_life_days: Option<f64>,
    pub log_likelihood: f64,
    pub n_observations: usize,
}

/// pairs: (age in days when checked, claim was correct). ML over the grid.
pub fn fit_decay(source_name: &str, pairs: &[(f64, bool)]) -> Result<FittedDecay> {
    if pairs.is_empty() {
        return Err(Error::Invalid(format!(
            "cannot calibrate source {source_name:?}: no ground-truth pairs in the log"
        )));
    }
    let r0_grid: Vec<f64> = (1..20).map(|i| i as f64 / 20.0).chain([0.99]).collect();
    let mut best: Option<(f64, f64, Option<f64>)> = None;
    for &r0 in &r0_grid {
        for &hl in &HALF_LIFE_GRID {
            let mut ll = 0.0;
            for &(age, correct) in pairs {
                let p = match hl {
                    Some(h) => r0 * 0.5_f64.powf(age / h),
                    None => r0,
                };
                let p = p.clamp(EPS, 1.0 - EPS);
                ll += if correct { p.ln() } else { (1.0 - p).ln() };
            }
            if best.map_or(true, |(b, _, _)| ll > b) {
                best = Some((ll, r0, hl));
            }
        }
    }
    let (ll, r0, hl) = best.unwrap();
    Ok(FittedDecay {
        source_name: source_name.to_string(),
        r0,
        half_life_days: hl,
        log_likelihood: ll,
        n_observations: pairs.len(),
    })
}

/// Did a claim turn out to be right about a concrete value?
/// None = not checkable across claim types.
fn claim_covers(claim: &Claim, truth: &Claim) -> Option<bool> {
    match truth {
        Claim::Interval { lo, hi, .. } => {
            let v = (lo + hi) / 2.0;
            match claim {
                Claim::Interval { lo: a, hi: b, .. } => Some(*a <= v && v <= *b),
                _ => None,
            }
        }
        Claim::Value { value, .. } => match claim {
            Claim::Value { value: c, .. } => Some(c == value),
            Claim::NotValue { value: c, .. } => Some(c != value),
            _ => None,
        },
        Claim::NotValue { .. } => None,
    }
}

/// Scan the log: for each claim by `source_name`, find the earliest later
/// ground-truth evidence on the same (entity, slot) and score the claim.
pub fn calibration_pairs(
    log: &[Evidence],
    source_name: &str,
    min_truth_reliability: f64,
) -> Vec<(f64, bool)> {
    let mut pairs = Vec::new();
    for ev in log.iter().filter(|e| e.source.name == source_name) {
        let truth = log
            .iter()
            .filter(|t| {
                t.entity == ev.entity
                    && t.claim.slot() == ev.claim.slot()
                    && t.observed_at >= ev.observed_at
                    && t.source.name != source_name
                    && (t.source.axiomatic || t.source.reliability >= min_truth_reliability)
            })
            .min_by_key(|t| t.observed_at);
        let Some(t) = truth else { continue };
        let Some(correct) = claim_covers(&ev.claim, &t.claim) else {
            continue;
        };
        let age_days = (t.observed_at - ev.observed_at) as f64 / SECONDS_PER_DAY;
        pairs.push((age_days, correct));
    }
    pairs
}
