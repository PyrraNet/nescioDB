//! The Aporia engine: BOUND, SAMPLE, RESOLVE, FIND over the evidence log.
//!
//! Ignorance is the primary object. A slot with no evidence (and no shared
//! prior) has a uniform posterior and maximal entropy; evidence narrows
//! it, erosion widens it back, couplings let knowledge flow between slots.
//! Queries are evaluated `as_of` a point in time — time travel is a
//! parameter, not a feature.
//!
//! - [`types`] — results and query types (regions, plans, predicates)
//! - [`inference`] — unary factors and belief propagation
//! - [`resolve`] — procurement planning and Monte-Carlo validation

pub mod inference;
pub mod join;
pub mod resolve;
pub mod types;

pub use inference::{entropy_bits, support_indices, SUPPORT_EPS};
pub use types::{
    Bound, FindMode, JoinMatch, JoinOptions, JoinPredicate, JoinResult, Predicate,
    ProcurementAction, Region, ResolvePlan, ResolveStep, Tri, Value,
};

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use crate::error::{Error, Result};
use crate::model::domain::Domain;
use crate::rng::Rng;
use crate::store::Db;

use inference::{argmax, cell_value, credible_region, sample_index, support_indices as support};

pub(crate) type Marginals = BTreeMap<String, Vec<f64>>;

/// A read view of the database at one point in time. Marginals are
/// memoized per entity for the lifetime of the query.
pub struct Query<'a> {
    pub(crate) db: &'a Db,
    pub(crate) as_of: i64,
    pub(crate) memo: RefCell<HashMap<String, Marginals>>,
}

impl<'a> Query<'a> {
    pub fn new(db: &'a Db, as_of: i64) -> Self {
        Query {
            db,
            as_of,
            memo: RefCell::new(HashMap::new()),
        }
    }

    pub fn as_of(&self) -> i64 {
        self.as_of
    }

    // ---------------------------------------------------------------- BOUND

    pub fn bound(&self, entity: &str, slot: &str, credible: f64) -> Result<Bound> {
        let domain = self.db.domain(slot)?;
        let post = self.marginal(entity, slot, &BTreeMap::new(), &[])?;
        let entropy = entropy_bits(&post);
        let region = credible_region(domain, &post, credible);
        let map_i = argmax(&post);
        let map_estimate = cell_value(domain, map_i);
        Ok(Bound {
            entity: entity.to_string(),
            slot: slot.to_string(),
            region,
            entropy_bits: entropy,
            max_entropy_bits: domain.max_entropy_bits(),
            map_estimate,
            posterior: post,
        })
    }

    // --------------------------------------------------------------- SAMPLE

    /// Draw one consistent world via the chain rule: sample a slot,
    /// condition on it, sample the next (slots in schema order, which is
    /// deterministic). Couplings are respected — a world never violates a
    /// hard coupling. Deterministic under the seed, across platforms.
    pub fn sample(&self, entity: &str, seed: u64) -> Result<BTreeMap<String, Value>> {
        self.sample_with(entity, &["sample", entity, &seed.to_string()])
    }

    pub(crate) fn sample_with(
        &self,
        entity: &str,
        seed_parts: &[&str],
    ) -> Result<BTreeMap<String, Value>> {
        let mut rng = Rng::from_parts(seed_parts);
        let mut world = BTreeMap::new();
        let mut condition: BTreeMap<String, usize> = BTreeMap::new();
        for (slot, domain) in &self.db.schema.slots {
            let post = self.marginal(entity, slot, &condition, &[])?;
            let i = sample_index(&post, &mut rng);
            condition.insert(slot.clone(), i);
            let value = match domain {
                Domain::Continuous { .. } => {
                    let (a, b) = domain.cell_bounds(i);
                    Value::Num(a + rng.next_f64() * (b - a))
                }
                Domain::Categorical { values } => Value::Cat(values[i].clone()),
            };
            world.insert(slot.clone(), value);
        }
        Ok(world)
    }

    // ------------------------------------------------- three-valued queries

    /// Region-containment check, not a value comparison.
    pub fn certainly(&self, entity: &str, slot: &str, pred: &Predicate) -> Result<Tri> {
        let domain = self.db.domain(slot)?;
        let post = self.marginal(entity, slot, &BTreeMap::new(), &[])?;
        let mut any = false;
        let mut all = true;
        for i in support(&post) {
            if pred.matches(&cell_value(domain, i))? {
                any = true;
            } else {
                all = false;
            }
        }
        Ok(if all {
            Tri::True
        } else if any {
            Tri::Possible
        } else {
            Tri::False
        })
    }

    // ----------------------------------------------------------------- FIND

    /// Region query across entities: which entities' regions certainly lie
    /// in / possibly intersect [lo, hi]? Support hulls are sorted in state
    /// space and used as a filter with exact refinement.
    pub fn find(&self, slot: &str, lo: f64, hi: f64, mode: FindMode) -> Result<Vec<String>> {
        let domain = self.db.domain(slot)?;
        if !matches!(domain, Domain::Continuous { .. }) {
            return Err(Error::Invalid(
                "find() ranges require a continuous slot".into(),
            ));
        }
        let mut hulls: Vec<(f64, f64, String)> = Vec::new();
        for entity in self.db.entities() {
            let post = self.marginal(entity, slot, &BTreeMap::new(), &[])?;
            let idx = support(&post);
            let sup_lo = domain.cell_bounds(idx[0]).0;
            let sup_hi = domain.cell_bounds(*idx.last().unwrap()).1;
            hulls.push((sup_lo, sup_hi, entity.to_string()));
        }
        hulls.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        let mut out = Vec::new();
        for (sup_lo, sup_hi, entity) in hulls {
            if sup_lo > hi {
                break; // sorted by lower edge: nothing further can intersect
            }
            if sup_hi < lo {
                continue;
            }
            match mode {
                FindMode::Certain => {
                    if lo <= sup_lo && sup_hi <= hi {
                        out.push(entity);
                    }
                }
                FindMode::Possible => {
                    // Refine: the hull may bridge a gap in a multi-modal support.
                    let post = self.marginal(&entity, slot, &BTreeMap::new(), &[])?;
                    let intersects = support(&post).into_iter().any(|i| {
                        let (a, b) = domain.cell_bounds(i);
                        b >= lo && a <= hi
                    });
                    if intersects {
                        out.push(entity);
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }
}
