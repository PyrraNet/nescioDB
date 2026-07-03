//! # nescioDB
//!
//! *nescio* (lat.): "I do not know". A working implementation of
//! **Aporia** — a database whose primary object is ignorance, not values.
//!
//! A slot without evidence is not `NULL`; it is a region of maximal
//! entropy. Evidence narrows regions, time widens them again (erosion as
//! storage physics), couplings let knowledge flow between slots. A
//! classical relational database is the limit case in which every
//! evidence is axiomatic and every region is a point.
//!
//! ## The verbs
//!
//! - **BOUND** — [`engine::Query::bound`]: credible region + entropy in
//!   bits + MAP estimate. How ignorant is the DB, really?
//! - **SAMPLE** — [`engine::Query::sample`]: one consistent world,
//!   deterministic under a seed, couplings respected.
//! - **RESOLVE** — [`engine::Query::resolve`]: which minimal-cost evidence
//!   would push a slot's entropy under a target? The DB plans its own
//!   data procurement, across slot boundaries, Monte-Carlo-validated.
//! - **FIND** — [`engine::Query::find`]: region queries across entities
//!   ("all objects whose price certainly lies below 600k").
//! - **JOIN** — [`engine::Query::join`]: entity pairs matching a relation,
//!   each with a probability *and* a three-valued certainty — joining two
//!   regions is itself uncertain.
//! - **certainly** — [`engine::Query::certainly`]: three-valued predicates
//!   as region containment: `true` / `possible` / `false`.
//!
//! ## Quick start
//!
//! ```
//! use nescio::prelude::*;
//! use std::collections::BTreeMap;
//!
//! let mut slots = BTreeMap::new();
//! slots.insert("price".into(), Domain::Continuous { lo: 0.0, hi: 1e6, n_bins: 200 });
//! let schema = Schema { slots, couplings: vec![] };
//! let broker = Source { name: "broker".into(), reliability: 0.85,
//!                       half_life_days: Some(90.0), axiomatic: false };
//! let mut db = Db::in_memory(schema, vec![broker]).unwrap();
//!
//! db.ingest(EvidenceRecord {
//!     entity: "house_1".into(),
//!     claim: Claim::Interval { slot: "price".into(), lo: 400_000.0, hi: 500_000.0 },
//!     source: "broker".into(),
//!     observed_at: 0,
//! }).unwrap();
//!
//! let q = Query::new(&db, 86_400); // one day later
//! let b = q.bound("house_1", "price", 0.95).unwrap();
//! assert!(b.entropy_bits < b.max_entropy_bits);
//! ```

pub mod binlog;
pub mod calibrate;
pub mod engine;
pub mod error;
pub mod model;
pub mod rng;
pub mod server;
pub mod store;
pub mod time;

pub mod prelude {
    pub use crate::calibrate::{calibration_pairs, fit_decay, FittedDecay};
    pub use crate::engine::{
        Bound, DecisionPlan, DecisionStep, FindMode, JoinMatch, JoinOptions, JoinPredicate,
        JoinResult, Objective, Predicate, ProcurementAction, Query, Region, ResolvePlan,
        ResolveStep, Tri, Value,
    };
    pub use crate::error::{Error, Result};
    pub use crate::model::coupling::{Compat, Coupling};
    pub use crate::model::domain::Domain;
    pub use crate::model::evidence::{Claim, Evidence, EvidenceRecord, Source};
    pub use crate::store::{Db, PriorDef, Priors, Schema};
}
