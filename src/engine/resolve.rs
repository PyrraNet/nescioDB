//! RESOLVE: the database plans its own evidence procurement.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::model::domain::Domain;
use crate::model::evidence::{Claim, Evidence};
use crate::rng::Rng;

use super::inference::{cell_value, entropy_bits};
use super::types::{ProcurementAction, ResolvePlan, ResolveStep, Value};
use super::Query;

impl Query<'_> {
    /// Plan backwards: which minimal-cost evidence pushes the target
    /// slot's entropy under the goal? Actions may target *other* slots —
    /// couplings carry the information.
    ///
    /// Selection is greedy on expected-gain-per-cost; per-action expected
    /// entropy averages over where the answer could land (weighted by the
    /// current marginal). Information gain from conditionally independent
    /// observations is submodular, so greedy is within (1 - 1/e) of the
    /// optimal same-cost plan under the usual assumptions. Because the
    /// greedy roll-forward (answer at the running median) remains an
    /// approximation, the final plan is re-scored by seeded Monte-Carlo
    /// simulation over full worlds — `validated_entropy_bits` is the
    /// number to trust. An empty plan means: nothing on offer helps.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        entity: &str,
        slot: &str,
        target_entropy_bits: f64,
        actions: &[ProcurementAction],
        max_steps: usize,
        mc_samples: usize,
        seed: u64,
    ) -> Result<ResolvePlan> {
        for a in actions {
            self.db.domain(&a.slot)?;
            a.source.validate()?;
        }
        let start = entropy_bits(&self.marginal(entity, slot, &BTreeMap::new(), &[])?);
        let mut h = start;
        let mut hypo: Vec<Evidence> = Vec::new();
        let mut steps: Vec<ResolveStep> = Vec::new();
        let mut remaining: Vec<&ProcurementAction> = actions.iter().collect();
        while h > target_entropy_bits && !remaining.is_empty() && steps.len() < max_steps {
            let mut best: Option<(f64, usize, f64)> = None; // (score, idx, expected_h)
            for (i, a) in remaining.iter().enumerate() {
                let eh = self.expected_entropy(entity, slot, a, &hypo)?;
                let gain = h - eh;
                if gain <= 1e-9 {
                    continue;
                }
                let score = gain / a.cost.max(1e-9);
                if best.map_or(true, |(s, _, _)| score > s) {
                    best = Some((score, i, eh));
                }
            }
            let Some((_, idx, eh)) = best else {
                break; // nothing helps; the DB knows it cannot know more
            };
            let action = remaining.remove(idx);
            steps.push(ResolveStep {
                action: action.clone(),
                expected_entropy_bits: eh,
                expected_gain_bits: h - eh,
            });
            hypo.push(self.hypothetical_answer(entity, action, &hypo)?);
            h = entropy_bits(&self.marginal(entity, slot, &BTreeMap::new(), &hypo)?);
        }
        let validated = if !steps.is_empty() && mc_samples > 0 {
            Some(self.validate_plan(entity, slot, &steps, mc_samples, seed)?)
        } else if steps.is_empty() {
            Some(start)
        } else {
            None
        };
        Ok(ResolvePlan {
            total_cost: steps.iter().map(|s| s.action.cost).sum(),
            steps,
            start_entropy_bits: start,
            planned_entropy_bits: h,
            validated_entropy_bits: validated,
        })
    }

    fn answer_evidence(
        &self,
        entity: &str,
        action: &ProcurementAction,
        center: &Value,
    ) -> Result<Evidence> {
        let domain = self.db.domain(&action.slot)?;
        let claim = match (domain, center) {
            (
                Domain::Continuous {
                    lo: dlo, hi: dhi, ..
                },
                Value::Num(c),
            ) => {
                let w = action.answer_width.unwrap_or((dhi - dlo) / 10.0);
                Claim::Interval {
                    slot: action.slot.clone(),
                    lo: (c - w / 2.0).max(*dlo),
                    hi: (c + w / 2.0).min(*dhi),
                }
            }
            (Domain::Categorical { .. }, Value::Cat(v)) => Claim::Value {
                slot: action.slot.clone(),
                value: v.clone(),
            },
            _ => return Err(Error::Invalid("action/answer type mismatch".into())),
        };
        Ok(Evidence {
            entity: entity.to_string(),
            claim,
            source: action.source.clone(),
            observed_at: self.as_of,
        })
    }

    /// E[H(target | answer)]: average over where the answer could land,
    /// weighted by the current marginal of the action's slot. Subsampled
    /// on wide domains for speed.
    fn expected_entropy(
        &self,
        entity: &str,
        target_slot: &str,
        action: &ProcurementAction,
        hypo: &[Evidence],
    ) -> Result<f64> {
        let domain = self.db.domain(&action.slot)?;
        let post = self.marginal(entity, &action.slot, &BTreeMap::new(), hypo)?;
        let n = domain.n();
        let step = (n / 12).max(1);
        let mut total_w = 0.0;
        let mut acc = 0.0;
        let mut extra: Vec<Evidence> = hypo.to_vec();
        for i in (0..n).step_by(step) {
            let w: f64 = post[i..(i + step).min(n)].iter().sum();
            if w <= 1e-9 {
                continue;
            }
            let center = cell_value(domain, (i + step / 2).min(n - 1));
            extra.push(self.answer_evidence(entity, action, &center)?);
            let marg = self.marginal(entity, target_slot, &BTreeMap::new(), &extra)?;
            extra.pop();
            acc += w * entropy_bits(&marg);
            total_w += w;
        }
        if total_w <= 0.0 {
            let marg = self.marginal(entity, target_slot, &BTreeMap::new(), hypo)?;
            return Ok(entropy_bits(&marg));
        }
        Ok(acc / total_w)
    }

    /// Roll-forward assumption for greedy planning: the answer lands at
    /// the running median of the action slot's marginal (less biased than
    /// the MAP; the final plan is Monte-Carlo-validated regardless).
    fn hypothetical_answer(
        &self,
        entity: &str,
        action: &ProcurementAction,
        hypo: &[Evidence],
    ) -> Result<Evidence> {
        let domain = self.db.domain(&action.slot)?;
        let post = self.marginal(entity, &action.slot, &BTreeMap::new(), hypo)?;
        let mut acc = 0.0;
        let mut med = post.len() - 1;
        for (i, p) in post.iter().enumerate() {
            acc += p;
            if acc >= 0.5 {
                med = i;
                break;
            }
        }
        self.answer_evidence(entity, action, &cell_value(domain, med))
    }

    /// Honest E[H] of the full plan: sample a true world, simulate every
    /// action's answer under the same mixture model the likelihoods assume
    /// (truthful with prob r, noise otherwise), score the target entropy,
    /// average. Deterministic under the seed.
    fn validate_plan(
        &self,
        entity: &str,
        target_slot: &str,
        steps: &[ResolveStep],
        k: usize,
        seed: u64,
    ) -> Result<f64> {
        let seed_s = seed.to_string();
        let mut acc = 0.0;
        for s in 0..k {
            let s_s = s.to_string();
            let mut rng = Rng::from_parts(&["validate", entity, &seed_s, &s_s]);
            let world = self.sample_with(entity, &["validate-world", entity, &seed_s, &s_s])?;
            let mut hypo: Vec<Evidence> = Vec::new();
            for step in steps {
                let a = &step.action;
                let domain = self.db.domain(&a.slot)?;
                let r = a.source.reliability_at(0.0);
                let truthful = rng.next_f64() < r;
                let center = match domain {
                    Domain::Continuous { lo, hi, .. } => {
                        if truthful {
                            world[&a.slot].clone()
                        } else {
                            Value::Num(lo + rng.next_f64() * (hi - lo))
                        }
                    }
                    Domain::Categorical { values } => {
                        if truthful {
                            world[&a.slot].clone()
                        } else {
                            Value::Cat(values[rng.choice_index(values.len())].clone())
                        }
                    }
                };
                hypo.push(self.answer_evidence(entity, a, &center)?);
            }
            acc += entropy_bits(&self.marginal(entity, target_slot, &BTreeMap::new(), &hypo)?);
        }
        Ok(acc / k as f64)
    }
}
