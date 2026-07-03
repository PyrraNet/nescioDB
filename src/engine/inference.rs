//! Posterior computation: unary factors and belief propagation.
//!
//! Each slot's unary factor = shared prior x evidence likelihoods. Coupled
//! slots exchange belief-propagation messages over the coupling graph —
//! exact on forests, damped-loopy (approximate, documented) on cycles.
//! Uncoupled slots take the fast path.

use std::collections::{BTreeMap, HashMap};

use crate::error::{Error, Result};
use crate::model::domain::Domain;
use crate::model::evidence::Evidence;
use crate::rng::Rng;

use super::types::{Region, Value};
use super::{Marginals, Query};

/// Cells outside the (1 - SUPPORT_EPS) credible set count as "impossible"
/// for the three-valued predicate logic and the support hulls.
pub const SUPPORT_EPS: f64 = 1e-3;

impl Query<'_> {
    pub fn marginal(
        &self,
        entity: &str,
        slot: &str,
        condition: &BTreeMap<String, usize>,
        extra: &[Evidence],
    ) -> Result<Vec<f64>> {
        self.db.domain(slot)?;
        // Fast path: a slot with no couplings is independent of everything
        // else — its marginal is just its own unary factor. This keeps
        // BOUND/FIND on uncoupled slots O(one slot), not O(schema).
        if !self.db.adjacency.contains_key(slot) {
            return self.unary(entity, slot, condition.get(slot).copied(), extra);
        }
        Ok(self.marginals(entity, condition, extra)?[slot].clone())
    }

    pub(super) fn marginals(
        &self,
        entity: &str,
        condition: &BTreeMap<String, usize>,
        extra: &[Evidence],
    ) -> Result<Marginals> {
        let cacheable = condition.is_empty() && extra.is_empty();
        if cacheable {
            if let Some(m) = self.memo.borrow().get(entity) {
                return Ok(m.clone());
            }
        }
        let mut unary: Marginals = BTreeMap::new();
        for slot in self.db.schema.slots.keys() {
            unary.insert(
                slot.clone(),
                self.unary(entity, slot, condition.get(slot).copied(), extra)?,
            );
        }
        let beliefs = if self.db.schema.couplings.is_empty() {
            unary
        } else {
            self.belief_propagation(entity, unary)?
        };
        if cacheable {
            self.memo
                .borrow_mut()
                .insert(entity.to_string(), beliefs.clone());
        }
        Ok(beliefs)
    }

    fn unary(
        &self,
        entity: &str,
        slot: &str,
        cond: Option<usize>,
        extra: &[Evidence],
    ) -> Result<Vec<f64>> {
        let domain = self.db.domain(slot)?;
        let n = domain.n();
        if let Some(c) = cond {
            let mut v = vec![0.0; n];
            v[c] = 1.0;
            return Ok(v);
        }
        // Uniform prior unless a shared prior is assigned; ignorance is
        // the default state.
        let mut logp: Vec<f64> = match self.db.prior_for(entity, slot) {
            Some(w) => w
                .iter()
                .map(|p| if *p > 0.0 { p.ln() } else { f64::NEG_INFINITY })
                .collect(),
            None => vec![0.0; n],
        };
        let indices = self.db.evidence_for(entity, slot);
        let stored = indices.iter().map(|&i| &self.db.evidence[i]);
        let extras = extra
            .iter()
            .filter(|e| e.entity == entity && e.claim.slot() == slot);
        for ev in stored.chain(extras) {
            if ev.observed_at > self.as_of {
                continue; // not yet observed in this world
            }
            let r = ev.reliability_at(self.as_of);
            if r <= 0.0 {
                continue;
            }
            for (lp, lik) in logp.iter_mut().zip(ev.claim.likelihood(domain, r)) {
                *lp += if lik > 0.0 {
                    lik.ln()
                } else {
                    f64::NEG_INFINITY
                };
            }
        }
        let m = logp.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if m == f64::NEG_INFINITY {
            return Err(Error::AxiomConflict(format!(
                "{entity}.{slot}: axiomatic evidence conflicts"
            )));
        }
        normalized(logp.iter().map(|v| (v - m).exp()).collect(), entity, slot)
    }

    /// (Loopy) belief propagation over the coupling graph. Exact on
    /// forests; on cyclic graphs runs damped and is approximate.
    fn belief_propagation(&self, entity: &str, unary: Marginals) -> Result<Marginals> {
        let couplings = &self.db.schema.couplings;
        // Directed edges: (src, dst, coupling index, src-is-slot_a).
        let mut edges: Vec<(&str, &str, usize, bool)> = Vec::new();
        for (ci, c) in couplings.iter().enumerate() {
            edges.push((&c.slot_a, &c.slot_b, ci, true));
            edges.push((&c.slot_b, &c.slot_a, ci, false));
        }
        let mut msgs: HashMap<(&str, &str), Vec<f64>> = HashMap::new();
        for &(src, dst, _, _) in &edges {
            let n = self.db.schema.slots[dst].n();
            msgs.insert((src, dst), vec![1.0 / n as f64; n]);
        }
        let damping = if self.db.loopy { 0.5 } else { 0.0 };
        let max_iters = if self.db.loopy {
            100
        } else {
            2 * couplings.len() + 2
        };
        for _ in 0..max_iters {
            let mut delta: f64 = 0.0;
            for &(src, dst, ci, src_is_a) in &edges {
                // Pre-message: unary(src) x all incoming messages except from dst.
                let mut pre = unary[src].clone();
                if let Some(nbrs) = self.db.adjacency.get(src) {
                    for (nbr, _) in nbrs {
                        if nbr == dst {
                            continue;
                        }
                        for (p, q) in pre.iter_mut().zip(&msgs[&(nbr.as_str(), src)]) {
                            *p *= q;
                        }
                    }
                }
                let table = &self.db.tables[ci];
                let nd = self.db.schema.slots[dst].n();
                let mut new = vec![0.0; nd];
                if src_is_a {
                    for (i, p) in pre.iter().enumerate() {
                        if *p > 0.0 {
                            for (nj, tij) in new.iter_mut().zip(&table[i]) {
                                *nj += p * tij;
                            }
                        }
                    }
                } else {
                    for (i, nj) in new.iter_mut().enumerate() {
                        *nj = table[i].iter().zip(&pre).map(|(t, p)| t * p).sum();
                    }
                }
                let total: f64 = new.iter().sum();
                if total <= 0.0 {
                    return Err(Error::AxiomConflict(format!(
                        "{entity}: coupling {} is incompatible with the evidence on {src}",
                        couplings[ci].label()
                    )));
                }
                for v in &mut new {
                    *v /= total;
                }
                let old = msgs.get_mut(&(src, dst)).unwrap();
                if damping > 0.0 {
                    for (n_, o) in new.iter_mut().zip(old.iter()) {
                        *n_ = damping * o + (1.0 - damping) * *n_;
                    }
                }
                for (o, n_) in old.iter().zip(&new) {
                    delta = delta.max((o - n_).abs());
                }
                *old = new;
            }
            if delta < 1e-10 {
                break;
            }
        }
        let mut beliefs = Marginals::new();
        for (slot, u) in &unary {
            let mut b = u.clone();
            if let Some(nbrs) = self.db.adjacency.get(slot) {
                for (nbr, _) in nbrs {
                    for (p, q) in b.iter_mut().zip(&msgs[&(nbr.as_str(), slot.as_str())]) {
                        *p *= q;
                    }
                }
            }
            beliefs.insert(slot.clone(), normalized(b, entity, slot)?);
        }
        Ok(beliefs)
    }
}

// ------------------------------------------------- posterior arithmetic

pub fn entropy_bits(post: &[f64]) -> f64 {
    let h: f64 = -post
        .iter()
        .filter(|p| **p > 0.0)
        .map(|p| p * p.log2())
        .sum::<f64>();
    h.max(0.0) // avoid -0.0
}

fn normalized(weights: Vec<f64>, entity: &str, slot: &str) -> Result<Vec<f64>> {
    let total: f64 = weights.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return Err(Error::AxiomConflict(format!(
            "{entity}.{slot}: all mass annihilated"
        )));
    }
    Ok(weights.into_iter().map(|w| w / total).collect())
}

pub(super) fn argmax(post: &[f64]) -> usize {
    let mut best = 0;
    for (i, p) in post.iter().enumerate() {
        if *p > post[best] {
            best = i;
        }
    }
    best
}

pub(super) fn cell_value(domain: &Domain, i: usize) -> Value {
    match domain {
        Domain::Continuous { .. } => Value::Num(domain.midpoint(i)),
        Domain::Categorical { values } => Value::Cat(values[i].clone()),
    }
}

pub(super) fn sample_index(post: &[f64], rng: &mut Rng) -> usize {
    let x = rng.next_f64();
    let mut acc = 0.0;
    for (i, p) in post.iter().enumerate() {
        acc += p;
        if x <= acc {
            return i;
        }
    }
    post.len() - 1
}

/// Indices of all cells inside the (1 - SUPPORT_EPS) credible set — what
/// the DB considers genuinely possible. Sorted ascending; never empty.
pub fn support_indices(post: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..post.len()).collect();
    order.sort_by(|a, b| post[*b].total_cmp(&post[*a]));
    let mut keep = Vec::new();
    let mut acc = 0.0;
    for i in order {
        keep.push(i);
        acc += post[i];
        if acc >= 1.0 - SUPPORT_EPS {
            break;
        }
    }
    keep.sort_unstable();
    keep
}

pub(super) fn credible_region(domain: &Domain, post: &[f64], level: f64) -> Region {
    let mut order: Vec<usize> = (0..post.len()).collect();
    order.sort_by(|a, b| post[*b].total_cmp(&post[*a]));
    let mut keep = Vec::new();
    let mut acc = 0.0;
    for i in order {
        keep.push(i);
        acc += post[i];
        if acc >= level {
            break;
        }
    }
    keep.sort_unstable();
    match domain {
        Domain::Categorical { values } => {
            Region::Values(keep.into_iter().map(|i| values[i].clone()).collect())
        }
        Domain::Continuous { .. } => {
            // Merge adjacent kept bins into intervals — the hyperrectangle view.
            let mut intervals: Vec<(f64, f64)> = Vec::new();
            let w = domain.bin_width();
            for i in keep {
                let (a, b) = domain.cell_bounds(i);
                match intervals.last_mut() {
                    Some(last) if (last.1 - a).abs() < w * 1e-9 => last.1 = b,
                    _ => intervals.push((a, b)),
                }
            }
            Region::Intervals(intervals)
        }
    }
}
