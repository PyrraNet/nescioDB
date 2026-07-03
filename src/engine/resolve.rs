//! RESOLVE: the database plans its own evidence procurement.
//!
//! Two public verbs share one planner. [`Query::resolve`] drives the target
//! slot's *entropy* under a bit budget — the classical form. [`Query::resolve_decision`]
//! generalizes it: instead of entropy, it drives down the **Bayes risk** of
//! a decision problem ([`Objective`]), so the plan optimizes the Value of
//! Information for the decision you actually face — of which entropy is the
//! log-loss special case.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::model::domain::Domain;
use crate::model::evidence::{Claim, Evidence};
use crate::rng::Rng;

use super::inference::cell_value;
use super::objective::Objective;
use super::types::{
    DecisionPlan, DecisionStep, ProcurementAction, ResolvePlan, ResolveStep, Value,
};
use super::Query;

/// One planned procurement step, in the planner's neutral terms (risk, not
/// entropy) — each public verb re-labels it for its result type.
struct RawStep {
    action: ProcurementAction,
    expected_risk: f64,
    expected_gain: f64,
}

/// The planner's neutral output; [`Query::resolve`] and
/// [`Query::resolve_decision`] format it into their public result types.
struct RawPlan {
    steps: Vec<RawStep>,
    start: f64,
    planned: f64,
    validated: Option<f64>,
    total_cost: f64,
    start_post: Vec<f64>,
    end_post: Vec<f64>,
}

impl Query<'_> {
    /// Plan the minimal-cost evidence that pushes the target slot's entropy
    /// under `target_entropy_bits`. See [`Query::resolve_decision`] for the
    /// general decision-theoretic form; this is the `Objective::Entropy`
    /// special case, kept API-stable.
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
        let raw = self.plan_procurement(
            entity,
            slot,
            &Objective::Entropy,
            target_entropy_bits,
            actions,
            max_steps,
            mc_samples,
            seed,
        )?;
        Ok(ResolvePlan {
            steps: raw
                .steps
                .iter()
                .map(|s| ResolveStep {
                    action: s.action.clone(),
                    expected_entropy_bits: s.expected_risk,
                    expected_gain_bits: s.expected_gain,
                })
                .collect(),
            start_entropy_bits: raw.start,
            planned_entropy_bits: raw.planned,
            validated_entropy_bits: raw.validated,
            total_cost: raw.total_cost,
        })
    }

    /// Plan backwards for a *decision*: which minimal-cost evidence pushes
    /// the Bayes risk of `objective` under `target_risk`? Where entropy asks
    /// "how uncertain am I", this asks "how much would better evidence
    /// improve the decision I have to make" — the Value of Information
    /// proper. Actions may target *other* slots; couplings carry the gain.
    ///
    /// The plan also reports what the DB would decide now versus after the
    /// plan ([`DecisionPlan::recommended_now`] / `recommended_after`), so the
    /// output is a decision, not just a number. Greedy selection is
    /// Monte-Carlo-validated exactly as in [`Query::resolve`].
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_decision(
        &self,
        entity: &str,
        slot: &str,
        objective: &Objective,
        target_risk: f64,
        actions: &[ProcurementAction],
        max_steps: usize,
        mc_samples: usize,
        seed: u64,
    ) -> Result<DecisionPlan> {
        let raw = self.plan_procurement(
            entity,
            slot,
            objective,
            target_risk,
            actions,
            max_steps,
            mc_samples,
            seed,
        )?;
        let domain = self.db.domain(slot)?;
        Ok(DecisionPlan {
            objective: objective.label().to_string(),
            units: objective.units().to_string(),
            recommended_now: objective.recommendation(domain, &raw.start_post),
            recommended_after: objective.recommendation(domain, &raw.end_post),
            steps: raw
                .steps
                .iter()
                .map(|s| DecisionStep {
                    action: s.action.clone(),
                    expected_risk: s.expected_risk,
                    expected_gain: s.expected_gain,
                })
                .collect(),
            start_risk: raw.start,
            planned_risk: raw.planned,
            validated_risk: raw.validated,
            total_cost: raw.total_cost,
        })
    }

    /// The shared planner, in terms of an arbitrary [`Objective`]'s Bayes
    /// risk. Selection is greedy on expected-gain-per-cost; per-action
    /// expected risk averages over where the answer could land (weighted by
    /// the current marginal). Information gain from conditionally
    /// independent observations is submodular, so greedy is within
    /// (1 - 1/e) of the optimal same-cost plan under the usual assumptions.
    /// Because the greedy roll-forward (answer at the running median)
    /// remains an approximation, the final plan is re-scored by seeded
    /// Monte-Carlo simulation over full worlds — the validated number is the
    /// one to trust. An empty plan means: nothing on offer helps.
    #[allow(clippy::too_many_arguments)]
    fn plan_procurement(
        &self,
        entity: &str,
        slot: &str,
        objective: &Objective,
        target: f64,
        actions: &[ProcurementAction],
        max_steps: usize,
        mc_samples: usize,
        seed: u64,
    ) -> Result<RawPlan> {
        let domain = self.db.domain(slot)?;
        objective.validate(domain)?;
        for a in actions {
            self.db.domain(&a.slot)?;
            a.source.validate()?;
        }
        let start_post = self.marginal(entity, slot, &BTreeMap::new(), &[])?;
        let start = objective.risk(domain, &start_post);
        let mut h = start;
        let mut hypo: Vec<Evidence> = Vec::new();
        let mut steps: Vec<RawStep> = Vec::new();
        let mut remaining: Vec<&ProcurementAction> = actions.iter().collect();
        while h > target && !remaining.is_empty() && steps.len() < max_steps {
            let mut best: Option<(f64, usize, f64)> = None; // (score, idx, expected_risk)
            for (i, a) in remaining.iter().enumerate() {
                let eh = self.expected_risk(entity, slot, objective, a, &hypo)?;
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
            steps.push(RawStep {
                action: action.clone(),
                expected_risk: eh,
                expected_gain: h - eh,
            });
            hypo.push(self.hypothetical_answer(entity, action, &hypo)?);
            h = objective.risk(
                domain,
                &self.marginal(entity, slot, &BTreeMap::new(), &hypo)?,
            );
        }
        let end_post = self.marginal(entity, slot, &BTreeMap::new(), &hypo)?;
        let validated = if !steps.is_empty() && mc_samples > 0 {
            Some(self.validate_plan(entity, slot, objective, &steps, mc_samples, seed)?)
        } else if steps.is_empty() {
            Some(start)
        } else {
            None
        };
        // `[].sum::<f64>()` seeds with -0.0; the `+ 0.0` normalizes an empty
        // plan's cost so it reports 0.0, not -0.0.
        let total_cost = steps.iter().map(|s| s.action.cost).sum::<f64>() + 0.0;
        Ok(RawPlan {
            total_cost,
            steps,
            start,
            planned: h,
            validated,
            start_post,
            end_post,
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

    /// E[risk(target | answer)]: average over where the answer could land,
    /// weighted by the current marginal of the action's slot. Subsampled on
    /// wide domains for speed.
    fn expected_risk(
        &self,
        entity: &str,
        target_slot: &str,
        objective: &Objective,
        action: &ProcurementAction,
        hypo: &[Evidence],
    ) -> Result<f64> {
        let action_domain = self.db.domain(&action.slot)?;
        let target_domain = self.db.domain(target_slot)?;
        let post = self.marginal(entity, &action.slot, &BTreeMap::new(), hypo)?;
        let n = action_domain.n();
        let step = (n / 12).max(1);
        let mut total_w = 0.0;
        let mut acc = 0.0;
        let mut extra: Vec<Evidence> = hypo.to_vec();
        for i in (0..n).step_by(step) {
            let w: f64 = post[i..(i + step).min(n)].iter().sum();
            if w <= 1e-9 {
                continue;
            }
            let center = cell_value(action_domain, (i + step / 2).min(n - 1));
            extra.push(self.answer_evidence(entity, action, &center)?);
            let marg = self.marginal(entity, target_slot, &BTreeMap::new(), &extra)?;
            extra.pop();
            acc += w * objective.risk(target_domain, &marg);
            total_w += w;
        }
        if total_w <= 0.0 {
            let marg = self.marginal(entity, target_slot, &BTreeMap::new(), hypo)?;
            return Ok(objective.risk(target_domain, &marg));
        }
        Ok(acc / total_w)
    }

    /// Roll-forward assumption for greedy planning: the answer lands at the
    /// running median of the action slot's marginal (less biased than the
    /// MAP; the final plan is Monte-Carlo-validated regardless).
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

    /// Honest E[risk] of the full plan: sample a true world, simulate every
    /// action's answer under the same mixture model the likelihoods assume
    /// (truthful with prob r, noise otherwise), score the target risk,
    /// average. Deterministic under the seed.
    fn validate_plan(
        &self,
        entity: &str,
        target_slot: &str,
        objective: &Objective,
        steps: &[RawStep],
        k: usize,
        seed: u64,
    ) -> Result<f64> {
        let target_domain = self.db.domain(target_slot)?;
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
            acc += objective.risk(
                target_domain,
                &self.marginal(entity, target_slot, &BTreeMap::new(), &hypo)?,
            );
        }
        Ok(acc / k as f64)
    }
}
