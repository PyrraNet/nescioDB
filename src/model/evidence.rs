//! Evidence: append-only claims from sources, with decay as storage physics.
//!
//! An Evidence never asserts a value. It contributes a likelihood factor
//! over a slot's domain under the mixture model: with probability `r` the
//! claim is right (value uniform on what it asserts), with `1 - r` it is
//! noise (value uniform on the whole domain). Reliability erodes with a
//! source-specific half-life:
//!
//! ```text
//! r(t) = r0 * 0.5 ^ (age_days / half_life_days)
//! ```
//!
//! As `r -> 0` the factor flattens toward uniform, so old evidence loosens
//! its grip on the region automatically — erosion is not a TTL hack, it is
//! the same mechanism as everything else. Deleting a source's evidence
//! removes its factors and every derived region widens correctly.
//!
//! The classical-DB limit case: axiomatic evidence has no half-life and
//! reliability 1.0 — its region is a point forever.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::domain::Domain;
use crate::time::SECONDS_PER_DAY;

/// Non-axiomatic evidence never gets reliability 1.0: two contradicting
/// hard claims would annihilate the posterior. Only axioms may be absolute,
/// and contradicting axioms are a real conflict (surfaced by the engine).
pub const MAX_SOFT_RELIABILITY: f64 = 0.9999;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    /// r0 in (0, 1]; 1.0 is only meaningful together with `axiomatic`.
    pub reliability: f64,
    /// None = no decay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub half_life_days: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub axiomatic: bool,
}

impl Source {
    pub fn reliability_at(&self, age_days: f64) -> f64 {
        let mut r = self.reliability;
        if !self.axiomatic {
            r = r.min(MAX_SOFT_RELIABILITY);
        }
        if let Some(hl) = self.half_life_days {
            if age_days > 0.0 && hl > 0.0 {
                r *= 0.5_f64.powf(age_days / hl);
            }
        }
        r.max(0.0)
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(Error::Invalid("source needs a non-empty name".into()));
        }
        if !(self.reliability > 0.0 && self.reliability <= 1.0) {
            return Err(Error::Invalid(format!(
                "source {:?}: reliability must be in (0, 1]",
                self.name
            )));
        }
        if let Some(hl) = self.half_life_days {
            if !hl.is_finite() || hl <= 0.0 {
                return Err(Error::Invalid(format!(
                    "source {:?}: half_life_days must be > 0",
                    self.name
                )));
            }
        }
        Ok(())
    }
}

/// What an evidence says about one slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Claim {
    /// "The value lies in [lo, hi]."
    Interval { slot: String, lo: f64, hi: f64 },
    /// "The value is v" (categorical).
    Value { slot: String, value: String },
    /// "The value is NOT v" (categorical).
    NotValue { slot: String, value: String },
}

impl Claim {
    pub fn slot(&self) -> &str {
        match self {
            Claim::Interval { slot, .. }
            | Claim::Value { slot, .. }
            | Claim::NotValue { slot, .. } => slot,
        }
    }

    pub fn validate(&self, domain: &Domain) -> Result<()> {
        match (self, domain) {
            (
                Claim::Interval { slot, lo, hi },
                Domain::Continuous {
                    lo: dlo, hi: dhi, ..
                },
            ) => {
                if lo > hi {
                    return Err(Error::Invalid(format!("claim on {slot:?}: lo > hi")));
                }
                if hi < dlo || lo > dhi {
                    return Err(Error::Invalid(format!(
                        "claim on {slot:?}: [{lo}, {hi}] does not intersect domain [{dlo}, {dhi}]"
                    )));
                }
                Ok(())
            }
            (Claim::Interval { slot, .. }, Domain::Categorical { .. }) => Err(Error::Invalid(
                format!("interval claim on categorical slot {slot:?}"),
            )),
            (Claim::Value { value, .. }, d) | (Claim::NotValue { value, .. }, d) => {
                d.index_of_value(value).map(|_| ())
            }
        }
    }

    /// Per-cell likelihood factor under the mixture model. `r = 0` yields a
    /// uniform factor (no information); `r = 1` a hard constraint. A
    /// *narrow* claim is stronger evidence than a vague one — precision
    /// carries weight.
    pub fn likelihood(&self, domain: &Domain, r: f64) -> Vec<f64> {
        let n = domain.n();
        match (self, domain) {
            (
                Claim::Interval { lo, hi, .. },
                Domain::Continuous {
                    lo: dlo, hi: dhi, ..
                },
            ) => {
                // A point claim still occupies at least one cell.
                let min_w = domain.bin_width();
                let c_lo = *lo;
                let c_hi = hi.max(lo + min_w);
                let claim_w = c_hi - c_lo;
                let domain_w = dhi - dlo;
                (0..n)
                    .map(|i| {
                        let (a, b) = domain.cell_bounds(i);
                        let overlap = (b.min(c_hi) - a.max(c_lo)).max(0.0);
                        r * overlap / claim_w + (1.0 - r) * (b - a) / domain_w
                    })
                    .collect()
            }
            (Claim::Value { value, .. }, Domain::Categorical { values }) => {
                let noise = (1.0 - r) / n as f64;
                values
                    .iter()
                    .map(|v| if v == value { r + noise } else { noise })
                    .collect()
            }
            (Claim::NotValue { value, .. }, Domain::Categorical { values }) => {
                let noise = (1.0 - r) / n as f64;
                let spread = r / (n as f64 - 1.0).max(1.0);
                values
                    .iter()
                    .map(|v| if v == value { noise } else { spread + noise })
                    .collect()
            }
            // Mismatches are rejected at ingest; fall back to uniform.
            _ => vec![1.0 / n as f64; n],
        }
    }
}

/// The persisted form: one line in `log.jsonl`, source referenced by name
/// so re-calibration corrects decay physics in exactly one place.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub entity: String,
    pub claim: Claim,
    pub source: String,
    /// Unix seconds (the canonical, serialized form). Deserialization also
    /// accepts a readable date ("2026-06-25", "2026-06-25T14:30") and the
    /// field name `at` — hand-written JSONL for `import` should not need a
    /// unix-timestamp converter.
    #[serde(alias = "at", deserialize_with = "de_observed_at")]
    pub observed_at: i64,
}

fn de_observed_at<'de, D>(d: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct WhenVisitor;
    impl serde::de::Visitor<'_> for WhenVisitor {
        type Value = i64;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("unix seconds or a date string (YYYY-MM-DD[THH:MM[:SS]])")
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<i64, E> {
            Ok(v)
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<i64, E> {
            i64::try_from(v).map_err(|_| E::custom("timestamp out of range"))
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<i64, E> {
            crate::time::parse_when(v).map_err(|e| E::custom(e.to_string()))
        }
    }
    d.deserialize_any(WhenVisitor)
}

/// The resolved in-memory form (source looked up).
#[derive(Clone, Debug)]
pub struct Evidence {
    pub entity: String,
    pub claim: Claim,
    pub source: Source,
    pub observed_at: i64,
}

impl Evidence {
    pub fn reliability_at(&self, as_of: i64) -> f64 {
        let age_days = ((as_of - self.observed_at) as f64 / SECONDS_PER_DAY).max(0.0);
        self.source.reliability_at(age_days)
    }
}
