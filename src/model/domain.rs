//! Slot domains: the discretized state spaces over which regions live.
//!
//! A slot's "region" is internally a posterior over a finite domain. For
//! continuous slots the domain is binned; hyperrectangles are only the
//! *output format* of BOUND (credible region), never the internal truth.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Domain {
    /// A bounded continuous domain discretized into equal-width bins.
    Continuous { lo: f64, hi: f64, n_bins: usize },
    /// A finite set of labelled values. Booleans are `["true", "false"]`.
    Categorical { values: Vec<String> },
}

impl Domain {
    pub fn boolean() -> Self {
        Domain::Categorical {
            values: vec!["true".into(), "false".into()],
        }
    }

    pub fn n(&self) -> usize {
        match self {
            Domain::Continuous { n_bins, .. } => *n_bins,
            Domain::Categorical { values } => values.len(),
        }
    }

    pub fn max_entropy_bits(&self) -> f64 {
        (self.n() as f64).log2()
    }

    pub fn bin_width(&self) -> f64 {
        match self {
            Domain::Continuous { lo, hi, n_bins } => (hi - lo) / *n_bins as f64,
            Domain::Categorical { .. } => 1.0,
        }
    }

    /// (lo, hi) of cell `i` — continuous domains only.
    pub fn cell_bounds(&self, i: usize) -> (f64, f64) {
        match self {
            Domain::Continuous { lo, .. } => {
                let w = self.bin_width();
                (lo + i as f64 * w, lo + (i as f64 + 1.0) * w)
            }
            Domain::Categorical { .. } => (i as f64, i as f64 + 1.0),
        }
    }

    pub fn midpoint(&self, i: usize) -> f64 {
        let (a, b) = self.cell_bounds(i);
        (a + b) / 2.0
    }

    pub fn index_of_value(&self, v: &str) -> Result<usize> {
        match self {
            Domain::Categorical { values } => values
                .iter()
                .position(|x| x == v)
                .ok_or_else(|| Error::Invalid(format!("value {v:?} not in domain {values:?}"))),
            Domain::Continuous { .. } => Err(Error::Invalid(
                "categorical value used on a continuous domain".into(),
            )),
        }
    }

    pub fn validate(&self, slot: &str) -> Result<()> {
        match self {
            Domain::Continuous { lo, hi, n_bins } => {
                if !hi.is_finite() || !lo.is_finite() || hi <= lo || *n_bins < 2 {
                    return Err(Error::Invalid(format!(
                        "slot {slot:?}: continuous domain needs hi > lo and n_bins >= 2"
                    )));
                }
            }
            Domain::Categorical { values } => {
                if values.len() < 2 {
                    return Err(Error::Invalid(format!(
                        "slot {slot:?}: categorical domain needs >= 2 values"
                    )));
                }
                let mut seen = std::collections::BTreeSet::new();
                for v in values {
                    if !seen.insert(v) {
                        return Err(Error::Invalid(format!(
                            "slot {slot:?}: duplicate value {v:?}"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}
