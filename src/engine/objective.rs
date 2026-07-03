//! Decision objectives for RESOLVE: what "risk" a plan should drive down.
//!
//! Classical RESOLVE pushes *entropy* under a target. But entropy is only
//! the Bayes risk under log-loss — the cost of having to report the whole
//! posterior. An agent rarely wants the distribution; it wants to *decide*
//! something, and different decisions value information differently. A
//! cheap observation that halves the entropy is worthless if it never
//! changes the decision you would make; a tiny observation that flips a
//! high-stakes call is worth a lot.
//!
//! This module generalizes the reduced quantity from entropy to the
//! **Bayes risk** of a decision problem, of which entropy is one instance.
//! Given a posterior `p` over a slot's cells, the Bayes risk is
//!
//! ```text
//! R(p) = min_d  E_{θ~p}[ L(d, θ) ]
//! ```
//!
//! — the loss of the best decision available *right now*. RESOLVE then
//! acquires the evidence that most reduces `R`: the true Value of
//! Information, not a proxy for it.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::domain::Domain;

use super::inference::{argmax, entropy_bits};

/// The decision problem a RESOLVE plan optimizes for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Objective {
    /// Report the full posterior; loss is negative log-likelihood, so the
    /// Bayes risk is the Shannon entropy in bits. This is exactly the
    /// classical RESOLVE target — kept as the special case it always was.
    Entropy,
    /// Commit to a point estimate under squared error. The Bayes action is
    /// the posterior mean and the Bayes risk is the posterior *variance* —
    /// natural for valuation ("how wrong, squared, will my price be?").
    SquaredError,
    /// Commit to a point estimate under absolute error. The Bayes action is
    /// the posterior median and the Bayes risk is the mean absolute
    /// deviation — robust to the heavy tails real evidence produces.
    AbsoluteError,
    /// A finite decision with an explicit loss for every (decision, cell)
    /// pair: `loss[d][cell]`. The Bayes risk is the smallest expected loss
    /// over the decisions. This is where the decision-optimal action can
    /// diverge from the entropy-optimal one: asymmetric stakes change what
    /// is worth finding out. `labels` name the decisions for reporting.
    Decision {
        loss: Vec<Vec<f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        labels: Option<Vec<String>>,
    },
}

impl Objective {
    pub fn label(&self) -> &'static str {
        match self {
            Objective::Entropy => "entropy",
            Objective::SquaredError => "squared_error",
            Objective::AbsoluteError => "absolute_error",
            Objective::Decision { .. } => "decision",
        }
    }

    /// The unit the risk is measured in — shown next to the numbers so a
    /// "risk of 4.2" is never ambiguous.
    pub fn units(&self) -> &'static str {
        match self {
            Objective::Entropy => "bits",
            Objective::SquaredError => "variance",
            Objective::AbsoluteError => "abs. error",
            Objective::Decision { .. } => "loss",
        }
    }

    /// Check the objective is well-formed for the target slot's domain.
    /// Point objectives need a continuous slot; a loss matrix must have one
    /// entry per cell for every decision.
    pub fn validate(&self, domain: &Domain) -> Result<()> {
        match self {
            Objective::Entropy => Ok(()),
            Objective::SquaredError | Objective::AbsoluteError => match domain {
                Domain::Continuous { .. } => Ok(()),
                Domain::Categorical { .. } => Err(Error::Invalid(format!(
                    "{} objective needs a continuous slot",
                    self.label()
                ))),
            },
            Objective::Decision { loss, labels } => {
                if loss.is_empty() {
                    return Err(Error::Invalid(
                        "decision objective needs at least one decision".into(),
                    ));
                }
                let n = domain.n();
                for (d, row) in loss.iter().enumerate() {
                    if row.len() != n {
                        return Err(Error::Invalid(format!(
                            "decision {d}: loss has {} entries, slot has {n} cells",
                            row.len()
                        )));
                    }
                    if row.iter().any(|x| !x.is_finite()) {
                        return Err(Error::Invalid(format!(
                            "decision {d}: loss entries must be finite"
                        )));
                    }
                }
                if let Some(l) = labels {
                    if l.len() != loss.len() {
                        return Err(Error::Invalid(format!(
                            "decision objective: {} labels for {} decisions",
                            l.len(),
                            loss.len()
                        )));
                    }
                }
                Ok(())
            }
        }
    }

    /// Bayes risk of the posterior — the loss of the best decision available
    /// now. Lower is better; evidence drives it down. Assumes [`validate`]
    /// has already passed for this domain.
    ///
    /// [`validate`]: Objective::validate
    pub fn risk(&self, domain: &Domain, post: &[f64]) -> f64 {
        match self {
            Objective::Entropy => entropy_bits(post),
            Objective::SquaredError => mean_variance(domain, post).1,
            Objective::AbsoluteError => {
                let center = domain.midpoint(median_cell(post));
                post.iter()
                    .enumerate()
                    .map(|(i, p)| p * (domain.midpoint(i) - center).abs())
                    .sum()
            }
            Objective::Decision { loss, .. } => decision_risk(loss, post).0,
        }
    }

    /// What the DB would decide right now under this objective, as a human
    /// string — the actionable half of a plan ("I would decide X").
    pub fn recommendation(&self, domain: &Domain, post: &[f64]) -> String {
        match self {
            Objective::Entropy => {
                format!("report posterior (MAP {})", value_str(domain, argmax(post)))
            }
            Objective::SquaredError => format!("estimate {:.0}", mean_variance(domain, post).0),
            Objective::AbsoluteError => {
                format!("estimate {:.0}", domain.midpoint(median_cell(post)))
            }
            Objective::Decision { loss, labels } => {
                let d = decision_risk(loss, post).1;
                match labels {
                    Some(l) => l[d].clone(),
                    None => format!("decision #{d}"),
                }
            }
        }
    }
}

/// Posterior (mean, variance) over cell midpoints — one pass.
fn mean_variance(domain: &Domain, post: &[f64]) -> (f64, f64) {
    let mut mean = 0.0;
    let mut m2 = 0.0;
    for (i, p) in post.iter().enumerate() {
        let x = domain.midpoint(i);
        mean += p * x;
        m2 += p * x * x;
    }
    (mean, (m2 - mean * mean).max(0.0))
}

/// First cell whose cumulative mass reaches one half.
fn median_cell(post: &[f64]) -> usize {
    let mut acc = 0.0;
    for (i, p) in post.iter().enumerate() {
        acc += p;
        if acc >= 0.5 {
            return i;
        }
    }
    post.len() - 1
}

/// (Bayes risk, argmin decision) for a loss matrix `loss[d][cell]`.
fn decision_risk(loss: &[Vec<f64>], post: &[f64]) -> (f64, usize) {
    let mut best = (f64::INFINITY, 0);
    for (d, row) in loss.iter().enumerate() {
        let r: f64 = row.iter().zip(post).map(|(l, p)| l * p).sum();
        if r < best.0 {
            best = (r, d);
        }
    }
    best
}

fn value_str(domain: &Domain, i: usize) -> String {
    match domain {
        Domain::Continuous { .. } => format!("{:.0}", domain.midpoint(i)),
        Domain::Categorical { values } => values[i].clone(),
    }
}
