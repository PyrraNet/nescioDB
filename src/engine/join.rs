//! JOIN: relational predicates between entities, under uncertainty.
//!
//! A join compares a slot of one entity with a slot of another. Because
//! both sides are regions, the truth of the join is itself uncertain —
//! every match carries a graded `probability` AND a three-valued
//! `certainty` (the region-containment answer, consistent with the
//! `certainly` verb).
//!
//! Distinct entities carry independent posteriors, so the join
//! probability is an exact integral over their product measure — no
//! possible-worlds sampling, no lineage bookkeeping. That independence is
//! what makes `P(A.price > B.price)` a closed-form sum here, where
//! value-uncertain databases need Monte-Carlo or lineage tracking to join
//! over uncertainty.
//!
//! Pruning uses the same support hulls as FIND: a pair is examined only
//! when the predicate is regionally *possible*. An unselective join
//! (every region overlaps every other) is quadratic — fundamental to
//! joins, SQL included — so evaluation is capped at [`MAX_PAIRS`] and the
//! cap is reported, never silently applied.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::{Error, Result};
use crate::model::domain::Domain;

use super::inference::support_indices;
use super::types::{JoinMatch, JoinOptions, JoinPredicate, JoinResult, Tri};
use super::Query;

/// Ceiling on regionally-possible pairs evaluated in one join. A selective
/// join stays far below it; an unselective one hits it and is flagged
/// `truncated`.
const MAX_PAIRS: usize = 3_000_000;

struct Prep {
    id: String,
    post: Vec<f64>,
    support: Vec<usize>,
}

impl Query<'_> {
    /// Join entities on a relational predicate. See the module docs for the
    /// probability/certainty semantics.
    pub fn join(&self, pred: &JoinPredicate, opts: &JoinOptions) -> Result<JoinResult> {
        let (ls, rs) = pred.slots();
        let ld = self.db.domain(ls)?.clone();
        let rd = self.db.domain(rs)?.clone();
        if pred.is_numeric() {
            if !matches!(ld, Domain::Continuous { .. }) || !matches!(rd, Domain::Continuous { .. })
            {
                return Err(Error::Invalid(
                    "gt/lt/approx joins require continuous slots on both sides".into(),
                ));
            }
        } else if !matches!(ld, Domain::Categorical { .. })
            || !matches!(rd, Domain::Categorical { .. })
        {
            return Err(Error::Invalid(
                "same joins require categorical slots on both sides".into(),
            ));
        }

        let left = self.prepare(ls, opts.left_prefix.as_deref(), opts.require_evidence)?;
        let symmetric = ls == rs && opts.left_prefix == opts.right_prefix;
        let right_owned = if symmetric {
            Vec::new()
        } else {
            self.prepare(rs, opts.right_prefix.as_deref(), opts.require_evidence)?
        };
        let right: &[Prep] = if symmetric { &left } else { &right_owned };

        let mut matches = Vec::new();
        let mut examined = 0usize;
        let capped = if pred.is_numeric() {
            self.join_numeric(
                pred,
                opts,
                &ld,
                &rd,
                &left,
                right,
                symmetric,
                &mut matches,
                &mut examined,
            )
        } else {
            self.join_same(
                pred,
                opts,
                &ld,
                &rd,
                &left,
                right,
                symmetric,
                &mut matches,
                &mut examined,
            )
        };

        matches.sort_by(|a, b| {
            b.probability
                .total_cmp(&a.probability)
                .then(a.left.cmp(&b.left))
                .then(a.right.cmp(&b.right))
        });
        let truncated = capped || matches.len() > opts.limit;
        matches.truncate(opts.limit);
        Ok(JoinResult {
            matches,
            pairs_examined: examined,
            truncated,
        })
    }

    fn prepare(
        &self,
        slot: &str,
        prefix: Option<&str>,
        require_evidence: bool,
    ) -> Result<Vec<Prep>> {
        let mut out = Vec::new();
        for e in self.db.entities() {
            if let Some(p) = prefix {
                if !e.starts_with(p) {
                    continue;
                }
            }
            if require_evidence && self.db.evidence_for(e, slot).is_empty() {
                continue;
            }
            let post = self.marginal(e, slot, &BTreeMap::new(), &[])?;
            let support = support_indices(&post);
            out.push(Prep {
                id: e.to_string(),
                post,
                support,
            });
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn join_numeric(
        &self,
        pred: &JoinPredicate,
        opts: &JoinOptions,
        ld: &Domain,
        rd: &Domain,
        left: &[Prep],
        right: &[Prep],
        symmetric: bool,
        matches: &mut Vec<JoinMatch>,
        examined: &mut usize,
    ) -> bool {
        let lmid: Vec<f64> = (0..ld.n()).map(|i| ld.midpoint(i)).collect();
        let rmid: Vec<f64> = (0..rd.n()).map(|i| rd.midpoint(i)).collect();
        let rmin: Vec<f64> = right.iter().map(|p| rmid[p.support[0]]).collect();
        let rmax: Vec<f64> = right
            .iter()
            .map(|p| rmid[*p.support.last().unwrap()])
            .collect();
        let tol = match pred {
            JoinPredicate::Approx { tol, .. } => *tol,
            _ => 0.0,
        };
        // Order the right side so the pruning break is monotone.
        let mut order: Vec<usize> = (0..right.len()).collect();
        match pred {
            JoinPredicate::Lt { .. } => order.sort_by(|&a, &b| rmax[b].total_cmp(&rmax[a])),
            _ => order.sort_by(|&a, &b| rmin[a].total_cmp(&rmin[b])),
        }
        let dedup = symmetric && pred.is_symmetric();

        for a in left {
            let a_min = lmid[a.support[0]];
            let a_max = lmid[*a.support.last().unwrap()];
            for &ri in &order {
                let (b_min, b_max) = (rmin[ri], rmax[ri]);
                match pred {
                    JoinPredicate::Gt { .. } => {
                        if b_min >= a_max {
                            break; // possible iff a_max > b_min
                        }
                    }
                    JoinPredicate::Lt { .. } => {
                        if b_max <= a_min {
                            break; // possible iff a_min < b_max
                        }
                    }
                    JoinPredicate::Approx { .. } => {
                        if b_min > a_max + tol {
                            break;
                        }
                        if b_max < a_min - tol {
                            continue;
                        }
                    }
                    JoinPredicate::Same { .. } => unreachable!(),
                }
                let b = &right[ri];
                if a.id == b.id {
                    continue;
                }
                if dedup && a.id >= b.id {
                    continue;
                }
                *examined += 1;
                if *examined > MAX_PAIRS {
                    return true;
                }
                let tri = match pred {
                    JoinPredicate::Gt { .. } => tri_gt(a_min, a_max, b_min, b_max),
                    JoinPredicate::Lt { .. } => tri_gt(b_min, b_max, a_min, a_max),
                    JoinPredicate::Approx { .. } => tri_approx(a_min, a_max, b_min, b_max, tol),
                    JoinPredicate::Same { .. } => unreachable!(),
                };
                if tri == Tri::False {
                    continue;
                }
                if opts.certain_only && tri != Tri::True {
                    continue;
                }
                let p = match pred {
                    JoinPredicate::Gt { .. } => p_gt(&a.post, &lmid, &b.post, &rmid),
                    JoinPredicate::Lt { .. } => p_gt(&b.post, &rmid, &a.post, &lmid),
                    JoinPredicate::Approx { .. } => p_approx(&a.post, &lmid, &b.post, &rmid, tol),
                    JoinPredicate::Same { .. } => unreachable!(),
                };
                if p + 1e-12 < opts.min_probability {
                    continue;
                }
                matches.push(JoinMatch {
                    left: a.id.clone(),
                    right: b.id.clone(),
                    probability: p,
                    certainty: tri,
                });
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn join_same(
        &self,
        pred: &JoinPredicate,
        opts: &JoinOptions,
        ld: &Domain,
        rd: &Domain,
        left: &[Prep],
        right: &[Prep],
        symmetric: bool,
        matches: &mut Vec<JoinMatch>,
        examined: &mut usize,
    ) -> bool {
        let lvals = match ld {
            Domain::Categorical { values } => values,
            _ => unreachable!(),
        };
        let rvals = match rd {
            Domain::Categorical { values } => values,
            _ => unreachable!(),
        };
        // Each right entity's support as a set of value labels, plus buckets
        // label -> right positions. A "same" pair is only possible when the
        // supports intersect, so we only ever visit compatible candidates.
        let r_labels: Vec<HashSet<&str>> = right
            .iter()
            .map(|p| p.support.iter().map(|&i| rvals[i].as_str()).collect())
            .collect();
        let mut buckets: HashMap<&str, Vec<usize>> = HashMap::new();
        for (ri, labs) in r_labels.iter().enumerate() {
            for l in labs {
                buckets.entry(l).or_default().push(ri);
            }
        }
        let dedup = symmetric && pred.is_symmetric();

        for a in left {
            let a_labels: HashSet<&str> = a.support.iter().map(|&i| lvals[i].as_str()).collect();
            let mut seen: HashSet<usize> = HashSet::new();
            for l in &a_labels {
                let Some(cands) = buckets.get(l) else {
                    continue;
                };
                for &ri in cands {
                    if !seen.insert(ri) {
                        continue;
                    }
                    let b = &right[ri];
                    if a.id == b.id {
                        continue;
                    }
                    if dedup && a.id >= b.id {
                        continue;
                    }
                    *examined += 1;
                    if *examined > MAX_PAIRS {
                        return true;
                    }
                    let tri = tri_same(&a_labels, &r_labels[ri]);
                    if opts.certain_only && tri != Tri::True {
                        continue;
                    }
                    let p = p_same(&a.post, lvals, &b.post, rvals);
                    if p + 1e-12 < opts.min_probability {
                        continue;
                    }
                    matches.push(JoinMatch {
                        left: a.id.clone(),
                        right: b.id.clone(),
                        probability: p,
                        certainty: tri,
                    });
                }
            }
        }
        false
    }
}

// --------------------------------------------------- probability (graded)

/// P(A > B) under independent posteriors, midpoints ascending in both.
fn p_gt(a: &[f64], amid: &[f64], b: &[f64], bmid: &[f64]) -> f64 {
    let mut j = 0usize;
    let mut cum = 0.0;
    let mut p = 0.0;
    for i in 0..a.len() {
        while j < b.len() && bmid[j] < amid[i] {
            cum += b[j];
            j += 1;
        }
        p += a[i] * cum;
    }
    p
}

/// P(|A - B| <= tol): sliding window of B mass around each A midpoint.
fn p_approx(a: &[f64], amid: &[f64], b: &[f64], bmid: &[f64], tol: f64) -> f64 {
    let mut lo = 0usize;
    let mut hi = 0usize;
    let mut win = 0.0;
    let mut p = 0.0;
    for i in 0..a.len() {
        let ub = amid[i] + tol;
        let lb = amid[i] - tol;
        while hi < b.len() && bmid[hi] <= ub {
            win += b[hi];
            hi += 1;
        }
        while lo < hi && bmid[lo] < lb {
            win -= b[lo];
            lo += 1;
        }
        p += a[i] * win;
    }
    p
}

/// P(A == B) for categorical slots, matched by value label.
fn p_same(a: &[f64], avals: &[String], b: &[f64], bvals: &[String]) -> f64 {
    let bmap: HashMap<&str, f64> = bvals
        .iter()
        .enumerate()
        .map(|(j, l)| (l.as_str(), b[j]))
        .collect();
    let mut p = 0.0;
    for (i, l) in avals.iter().enumerate() {
        if let Some(bp) = bmap.get(l.as_str()) {
            p += a[i] * bp;
        }
    }
    p
}

// --------------------------------------------- certainty (region-valued)

fn tri_gt(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> Tri {
    if a_min > b_max {
        Tri::True
    } else if a_max > b_min {
        Tri::Possible
    } else {
        Tri::False
    }
}

fn tri_approx(a_min: f64, a_max: f64, b_min: f64, b_max: f64, tol: f64) -> Tri {
    let farthest = (a_max - b_min).max(b_max - a_min);
    let gap = 0f64.max(a_min - b_max).max(b_min - a_max);
    if farthest <= tol {
        Tri::True
    } else if gap <= tol {
        Tri::Possible
    } else {
        Tri::False
    }
}

fn tri_same(a: &HashSet<&str>, b: &HashSet<&str>) -> Tri {
    let intersect = a.iter().any(|l| b.contains(l));
    if a.len() == 1 && b.len() == 1 && intersect {
        Tri::True
    } else if intersect {
        Tri::Possible
    } else {
        Tri::False
    }
}
