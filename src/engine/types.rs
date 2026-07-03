//! Result and query types for the engine's verbs.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::evidence::Source;

/// A concrete cell value: numeric for continuous slots, labelled for
/// categorical ones.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    Num(f64),
    Cat(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Num(x) => write!(f, "{x}"),
            Value::Cat(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum Region {
    /// Credible region of a continuous slot: merged bin intervals — the
    /// hyperrectangle *view* of the posterior.
    Intervals(Vec<(f64, f64)>),
    Values(Vec<String>),
}

/// Result of BOUND: the region plus how ignorant the DB really is.
#[derive(Clone, Debug, Serialize)]
pub struct Bound {
    pub entity: String,
    pub slot: String,
    pub region: Region,
    pub entropy_bits: f64,
    pub max_entropy_bits: f64,
    pub map_estimate: Value,
    #[serde(skip)]
    pub posterior: Vec<f64>,
}

impl Bound {
    /// 0 = knows nothing, 1 = fully collapsed.
    pub fn knowledge_ratio(&self) -> f64 {
        if self.max_entropy_bits == 0.0 {
            1.0
        } else {
            1.0 - self.entropy_bits / self.max_entropy_bits
        }
    }
}

/// Three-valued truth: region containment, not value comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tri {
    True,
    Possible,
    False,
}

impl std::fmt::Display for Tri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tri::True => write!(f, "true"),
            Tri::Possible => write!(f, "possible"),
            Tri::False => write!(f, "false"),
        }
    }
}

/// Predicates for three-valued queries — declarative so the CLI and
/// serialized queries can express them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Predicate {
    Gt { value: f64 },
    Lt { value: f64 },
    Between { lo: f64, hi: f64 },
    Is { value: String },
    IsNot { value: String },
}

impl Predicate {
    pub(crate) fn matches(&self, v: &Value) -> Result<bool> {
        match (self, v) {
            (Predicate::Gt { value }, Value::Num(x)) => Ok(x > value),
            (Predicate::Lt { value }, Value::Num(x)) => Ok(x < value),
            (Predicate::Between { lo, hi }, Value::Num(x)) => Ok(lo <= x && x <= hi),
            (Predicate::Is { value }, Value::Cat(s)) => Ok(s == value),
            (Predicate::IsNot { value }, Value::Cat(s)) => Ok(s != value),
            _ => Err(Error::Invalid(
                "predicate type does not match slot domain".into(),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindMode {
    Possible,
    Certain,
}

impl std::str::FromStr for FindMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "possible" => Ok(FindMode::Possible),
            "certain" => Ok(FindMode::Certain),
            _ => Err(Error::Invalid(
                "mode must be 'possible' or 'certain'".into(),
            )),
        }
    }
}

/// Something you *could* do to gain evidence: ask a source about a slot.
/// The slot need not be the RESOLVE target — couplings carry the gain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcurementAction {
    pub name: String,
    pub slot: String,
    pub cost: f64,
    pub source: Source,
    /// For continuous slots: the answer arrives as an interval of this width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_width: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolveStep {
    pub action: ProcurementAction,
    pub expected_entropy_bits: f64,
    pub expected_gain_bits: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvePlan {
    pub steps: Vec<ResolveStep>,
    pub start_entropy_bits: f64,
    /// Greedy estimate for the full plan (median roll-forward).
    pub planned_entropy_bits: f64,
    /// Seeded Monte-Carlo estimate over full worlds — the number to trust.
    pub validated_entropy_bits: Option<f64>,
    pub total_cost: f64,
}

// ------------------------------------------------------- decision RESOLVE

/// One step of a decision-theoretic plan: risk is in the objective's units
/// (variance, absolute error, loss, or bits), not necessarily entropy.
#[derive(Clone, Debug, Serialize)]
pub struct DecisionStep {
    pub action: ProcurementAction,
    pub expected_risk: f64,
    pub expected_gain: f64,
}

/// Result of [`crate::engine::Query::resolve_decision`]: the evidence to
/// acquire to make a *decision* better, plus the decision itself — what the
/// DB would choose now versus after the plan runs. Where [`ResolvePlan`]
/// speaks in bits, this speaks in the objective's own risk units.
#[derive(Clone, Debug, Serialize)]
pub struct DecisionPlan {
    /// Objective name, e.g. `"squared_error"` or `"decision"`.
    pub objective: String,
    /// The unit `*_risk` is measured in (`variance` / `abs. error` /
    /// `loss` / `bits`).
    pub units: String,
    pub steps: Vec<DecisionStep>,
    pub start_risk: f64,
    /// Greedy estimate for the full plan (median roll-forward).
    pub planned_risk: f64,
    /// Seeded Monte-Carlo estimate over full worlds — the number to trust.
    pub validated_risk: Option<f64>,
    pub total_cost: f64,
    /// The decision the DB would make right now.
    pub recommended_now: String,
    /// The decision it would make after executing the plan (greedy
    /// roll-forward).
    pub recommended_after: String,
}

// -------------------------------------------------------------------- join

/// A relational predicate between a slot of the left entity and a slot of
/// the right entity. Because both sides are regions, the truth of a join
/// is itself uncertain — every match carries a graded probability AND a
/// three-valued certainty.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum JoinPredicate {
    /// `left.<a> > right.<b>`  (numeric)
    Gt { left: String, right: String },
    /// `left.<a> < right.<b>`  (numeric)
    Lt { left: String, right: String },
    /// `|left.<a> - right.<b>| <= tol`  (numeric similarity / fuzzy match)
    Approx {
        left: String,
        right: String,
        tol: f64,
    },
    /// `left.<a> == right.<b>`  (categorical; entity resolution)
    Same { left: String, right: String },
}

impl JoinPredicate {
    pub fn slots(&self) -> (&str, &str) {
        match self {
            JoinPredicate::Gt { left, right }
            | JoinPredicate::Lt { left, right }
            | JoinPredicate::Approx { left, right, .. }
            | JoinPredicate::Same { left, right } => (left, right),
        }
    }

    pub(crate) fn is_numeric(&self) -> bool {
        !matches!(self, JoinPredicate::Same { .. })
    }

    /// Symmetric predicates deduplicate (a,b)/(b,a) on a self-join.
    pub(crate) fn is_symmetric(&self) -> bool {
        matches!(
            self,
            JoinPredicate::Approx { .. } | JoinPredicate::Same { .. }
        )
    }
}

fn default_true() -> bool {
    true
}

fn default_join_limit() -> usize {
    1000
}

#[derive(Clone, Debug, Deserialize)]
pub struct JoinOptions {
    /// Restrict the left side to entities whose id starts with this prefix
    /// (a lightweight way to express entity "kinds" by naming convention).
    #[serde(default)]
    pub left_prefix: Option<String>,
    #[serde(default)]
    pub right_prefix: Option<String>,
    /// Keep only matches with at least this join probability.
    #[serde(default)]
    pub min_probability: f64,
    /// Keep only regionally-certain matches (certainty == true).
    #[serde(default)]
    pub certain_only: bool,
    /// Join only entities that actually have evidence on the compared slot
    /// (default true) — entities with none carry the uniform prior and
    /// would match almost everything as "possible".
    #[serde(default = "default_true")]
    pub require_evidence: bool,
    /// Cap on returned matches (ranked by probability). Truncation is
    /// reported, never silent.
    #[serde(default = "default_join_limit")]
    pub limit: usize,
}

impl Default for JoinOptions {
    fn default() -> Self {
        JoinOptions {
            left_prefix: None,
            right_prefix: None,
            min_probability: 0.0,
            certain_only: false,
            require_evidence: true,
            limit: default_join_limit(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JoinMatch {
    pub left: String,
    pub right: String,
    /// P(predicate holds) under the two entities' independent posteriors.
    pub probability: f64,
    /// Region-containment truth, matching the `certainly` verb.
    pub certainty: Tri,
}

#[derive(Clone, Debug, Serialize)]
pub struct JoinResult {
    pub matches: Vec<JoinMatch>,
    /// Regionally-possible pairs actually evaluated (post-pruning) — the
    /// real cost of the join.
    pub pairs_examined: usize,
    pub truncated: bool,
}
