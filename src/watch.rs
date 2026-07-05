//! Watches: standing questions with knowledge horizons.
//!
//! A watch turns decay from a passive effect into an active signal. It
//! names an entity's slot and a threshold — "fire when the entropy of
//! `villa_1.price` exceeds 5 bits", or equivalently "when knowledge drops
//! below 40%". Because erosion is deterministic physics, a watch does not
//! just report its current state: absent new evidence, the exact moment it
//! will fire is *computable in advance*. That moment is the watch's
//! **knowledge horizon** — the date past which the database no longer
//! knows enough for the standing question.
//!
//! A watch also fires when its slot is in axiom conflict: a standing
//! question must surface contradiction, not step around it.
//!
//! Watches are stored in `watches.json` next to the schema and evaluated
//! on demand ([`check_watches`]) — by the CLI (`nescio watch`), by the
//! HTTP routes (`/watches`), and by the server's background evaluator,
//! which pushes `triggered` / `recovered` transitions to Server-Sent-Event
//! subscribers (`GET /watches/events`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{entropy_bits, Query};
use crate::error::{Error, Result};
use crate::store::Db;
use crate::time::format_unix;

/// How far ahead the horizon scan looks, in days (10 years). A watch that
/// decay cannot trigger within this window reports no horizon.
pub const DEFAULT_HORIZON_DAYS: u32 = 3650;

/// Floating-point guard so a watch does not flap on the threshold itself.
const TRIGGER_EPS: f64 = 1e-9;

const DAY: i64 = 86_400;

/// A standing question: fire when an entity's slot decays past a
/// threshold. Exactly one of `max_entropy_bits` / `min_knowledge` is set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Watch {
    pub name: String,
    pub entity: String,
    pub slot: String,
    /// Fire when entropy exceeds this many bits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entropy_bits: Option<f64>,
    /// Fire when knowledge (1 - entropy/max entropy) drops below this
    /// ratio in (0, 1].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_knowledge: Option<f64>,
}

impl Watch {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::Invalid("watch name must not be empty".into()));
        }
        match (self.max_entropy_bits, self.min_knowledge) {
            (Some(b), None) => {
                if !b.is_finite() || b < 0.0 {
                    return Err(Error::Invalid(format!(
                        "watch {:?}: max_entropy_bits must be finite and >= 0",
                        self.name
                    )));
                }
            }
            (None, Some(k)) => {
                if !k.is_finite() || k <= 0.0 || k > 1.0 {
                    return Err(Error::Invalid(format!(
                        "watch {:?}: min_knowledge must be in (0, 1]",
                        self.name
                    )));
                }
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "watch {:?}: set exactly one of max_entropy_bits / min_knowledge",
                    self.name
                )));
            }
        }
        Ok(())
    }

    /// The entropy threshold in bits, given the slot's maximal entropy.
    pub fn threshold_bits(&self, max_entropy_bits: f64) -> f64 {
        match (self.max_entropy_bits, self.min_knowledge) {
            (Some(b), _) => b,
            (None, Some(k)) => (1.0 - k) * max_entropy_bits,
            (None, None) => f64::INFINITY, // unreachable after validate()
        }
    }
}

/// One watch, evaluated at a point in time.
#[derive(Clone, Debug, Serialize)]
pub struct WatchState {
    #[serde(flatten)]
    pub watch: Watch,
    pub triggered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_bits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entropy_bits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<f64>,
    /// When decay alone will trigger this watch (unix seconds, day
    /// granularity). Already-triggered watches carry the evaluation time;
    /// absent when decay cannot trigger it within [`DEFAULT_HORIZON_DAYS`]
    /// (an axiomatic, non-decaying source pins knowledge forever).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizon: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizon_date: Option<String>,
    /// Set when evaluation itself failed — most importantly an axiom
    /// conflict on the watched slot, which triggers the watch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Evaluate every watch of the database at one point in time.
pub fn check_watches(db: &Db, at: i64, within_days: u32) -> Vec<WatchState> {
    db.watches
        .iter()
        .map(|w| evaluate_watch(db, w, at, within_days))
        .collect()
}

/// Evaluate one watch. Infallible by design: an evaluation error (axiom
/// conflict, dangling slot) becomes a triggered state with `error` set —
/// a standing question that cannot be answered has fired.
pub fn evaluate_watch(db: &Db, w: &Watch, at: i64, within_days: u32) -> WatchState {
    let mut st = WatchState {
        watch: w.clone(),
        triggered: false,
        threshold_bits: None,
        entropy_bits: None,
        knowledge: None,
        horizon: None,
        horizon_date: None,
        error: None,
    };
    let max_bits = match db.domain(&w.slot) {
        Ok(d) => d.max_entropy_bits(),
        Err(e) => {
            st.error = Some(e.to_string());
            st.triggered = true;
            return st;
        }
    };
    let threshold = w.threshold_bits(max_bits);
    st.threshold_bits = Some(threshold);
    let entropy = match slot_entropy(db, &w.entity, &w.slot, at) {
        Ok(e) => e,
        Err(e) => {
            st.error = Some(e.to_string());
            st.triggered = true;
            return st;
        }
    };
    st.entropy_bits = Some(entropy);
    st.knowledge = Some(if max_bits > 0.0 {
        1.0 - entropy / max_bits
    } else {
        1.0
    });
    st.triggered = entropy > threshold + TRIGGER_EPS;
    st.horizon = if st.triggered {
        Some(at)
    } else {
        horizon(db, w, at, threshold, within_days)
    };
    st.horizon_date = st.horizon.map(format_unix);
    st
}

fn slot_entropy(db: &Db, entity: &str, slot: &str, at: i64) -> Result<f64> {
    let q = Query::new(db, at);
    let post = q.marginal(entity, slot, &BTreeMap::new(), &[])?;
    Ok(entropy_bits(&post))
}

fn crossed(db: &Db, w: &Watch, at: i64, threshold: f64) -> bool {
    // An evaluation error at a future time counts as crossed: the watch
    // would fire there.
    slot_entropy(db, &w.entity, &w.slot, at)
        .map(|e| e > threshold + TRIGGER_EPS)
        .unwrap_or(true)
}

/// Find when decay alone pushes the slot's entropy over the threshold:
/// a coarse forward scan (daily for two weeks, weekly to half a year,
/// monthly to the cap), refined to day granularity. A plain binary search
/// would assume monotone entropy — contradicting claims with different
/// half-lives can make entropy dip before it rises, so scan first.
fn horizon(db: &Db, w: &Watch, from: i64, threshold: f64, within_days: u32) -> Option<i64> {
    let mut prev = 0i64;
    let mut day = 1i64;
    while day <= within_days as i64 {
        if crossed(db, w, from + day * DAY, threshold) {
            let (mut lo, mut hi) = (prev, day);
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if crossed(db, w, from + mid * DAY, threshold) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            return Some(from + hi * DAY);
        }
        prev = day;
        day += if day < 14 {
            1
        } else if day < 182 {
            7
        } else {
            30
        };
    }
    None
}
