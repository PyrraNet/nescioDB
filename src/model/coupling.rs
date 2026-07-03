//! Coupling rules: explicit cross-slot correlations.
//!
//! A Coupling is a pairwise compatibility factor between two slots of the
//! same entity. It is schema-level (applies to every entity) and stored
//! once. This is the deliberate middle ground between "slots are
//! independent" and full joint inference (a probabilistic programming
//! language wearing a database costume).
//!
//! Because a database schema must be serializable, compatibility is not a
//! function pointer but a declarative form; each form compiles to an
//! explicit factor table at load time.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::domain::Domain;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Compat {
    /// Fully explicit: rows over slot_a's cells, columns over slot_b's.
    Table { rows: Vec<Vec<f64>> },
    /// slot_a categorical, slot_b continuous: each category pulls the
    /// continuous value toward a center, Gaussian-shaped with width sigma.
    /// Categories without a center are uninformative (weight 1).
    GaussianByCategory {
        centers: BTreeMap<String, f64>,
        sigma: f64,
    },
    /// Both categorical: `weights[a][b]`, missing entries fall back to `default`.
    Matrix {
        weights: BTreeMap<String, BTreeMap<String, f64>>,
        #[serde(default = "one")]
        default: f64,
    },
    /// slot_a continuous, slot_b categorical: category weights depend on
    /// whether the continuous value is below or above a threshold.
    /// Categories without an entry are uninformative (weight 1).
    StepThreshold {
        threshold: f64,
        below: BTreeMap<String, f64>,
        above: BTreeMap<String, f64>,
    },
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coupling {
    pub slot_a: String,
    pub slot_b: String,
    pub compat: Compat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Coupling {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{}~{}", self.slot_a, self.slot_b))
    }

    /// Compile to an explicit factor table (rows over slot_a's cells).
    pub fn build_table(&self, da: &Domain, db: &Domain) -> Result<Vec<Vec<f64>>> {
        let (na, nb) = (da.n(), db.n());
        let err = |m: &str| Error::Invalid(format!("coupling {}: {m}", self.label()));
        let rows: Vec<Vec<f64>> = match &self.compat {
            Compat::Table { rows } => {
                if rows.len() != na || rows.iter().any(|r| r.len() != nb) {
                    return Err(err(&format!("table must be {na}x{nb}")));
                }
                rows.clone()
            }
            Compat::GaussianByCategory { centers, sigma } => {
                let Domain::Categorical { values } = da else {
                    return Err(err("gaussian_by_category needs a categorical slot_a"));
                };
                let Domain::Continuous { .. } = db else {
                    return Err(err("gaussian_by_category needs a continuous slot_b"));
                };
                if !sigma.is_finite() || *sigma <= 0.0 {
                    return Err(err("sigma must be > 0"));
                }
                values
                    .iter()
                    .map(|v| match centers.get(v) {
                        Some(c) => (0..nb)
                            .map(|j| {
                                let x = (db.midpoint(j) - c) / sigma;
                                (-x * x).exp()
                            })
                            .collect(),
                        None => vec![1.0; nb],
                    })
                    .collect()
            }
            Compat::Matrix { weights, default } => {
                let (Domain::Categorical { values: va }, Domain::Categorical { values: vb }) =
                    (da, db)
                else {
                    return Err(err("matrix needs two categorical slots"));
                };
                va.iter()
                    .map(|a| {
                        vb.iter()
                            .map(|b| {
                                weights
                                    .get(a)
                                    .and_then(|row| row.get(b))
                                    .copied()
                                    .unwrap_or(*default)
                            })
                            .collect()
                    })
                    .collect()
            }
            Compat::StepThreshold {
                threshold,
                below,
                above,
            } => {
                let Domain::Continuous { .. } = da else {
                    return Err(err("step_threshold needs a continuous slot_a"));
                };
                let Domain::Categorical { values } = db else {
                    return Err(err("step_threshold needs a categorical slot_b"));
                };
                (0..na)
                    .map(|i| {
                        let side = if da.midpoint(i) < *threshold {
                            below
                        } else {
                            above
                        };
                        values
                            .iter()
                            .map(|v| side.get(v).copied().unwrap_or(1.0))
                            .collect()
                    })
                    .collect()
            }
        };
        for row in &rows {
            for w in row {
                if !w.is_finite() || *w < 0.0 {
                    return Err(err("weights must be finite and >= 0"));
                }
            }
        }
        Ok(rows)
    }
}
