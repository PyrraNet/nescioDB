//! Integration tests: every core claim of the Aporia concept, plus
//! persistence. Ported from the Python prototype's 19-test suite.

use std::collections::BTreeMap;

use nescio::prelude::*;
use nescio::rng::Rng;

const DAY: i64 = 86_400;

fn day(d: f64) -> i64 {
    (d * DAY as f64) as i64
}

fn price_domain() -> Domain {
    Domain::Continuous {
        lo: 0.0,
        hi: 1_000_000.0,
        n_bins: 200,
    }
}

fn source(name: &str, reliability: f64, half_life_days: Option<f64>) -> Source {
    Source {
        name: name.into(),
        reliability,
        half_life_days,
        axiomatic: false,
    }
}

fn axiom(name: &str) -> Source {
    Source {
        name: name.into(),
        reliability: 1.0,
        half_life_days: None,
        axiomatic: true,
    }
}

fn interval(slot: &str, lo: f64, hi: f64) -> Claim {
    Claim::Interval {
        slot: slot.into(),
        lo,
        hi,
    }
}

fn value(slot: &str, v: &str) -> Claim {
    Claim::Value {
        slot: slot.into(),
        value: v.into(),
    }
}

fn make_db(sources: Vec<Source>) -> Db {
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    Db::in_memory(
        Schema {
            slots,
            couplings: vec![],
        },
        sources,
    )
    .unwrap()
}

fn ingest(db: &mut Db, entity: &str, claim: Claim, source_name: &str, at_day: f64) {
    db.ingest(EvidenceRecord {
        entity: entity.into(),
        claim,
        source: source_name.into(),
        observed_at: day(at_day),
    })
    .unwrap();
}

fn region_width(b: &Bound) -> f64 {
    match &b.region {
        Region::Intervals(ivs) => ivs.iter().map(|(a, c)| c - a).sum(),
        Region::Values(_) => panic!("expected continuous region"),
    }
}

// ------------------------------------------------------------- core claims

#[test]
fn no_evidence_is_maximal_ignorance() {
    let mut db = make_db(vec![source("x", 0.5, Some(1.0))]);
    ingest(&mut db, "other", interval("price", 1.0, 2.0), "x", 0.0);
    let q = Query::new(&db, 0);
    let b = q.bound("obj1", "price", 0.95).unwrap();
    assert!((b.entropy_bits - b.max_entropy_bits).abs() < 1e-9);
    assert!(b.knowledge_ratio().abs() < 1e-9);
    assert!(region_width(&b) >= 0.94 * 1_000_000.0);
}

#[test]
fn evidence_narrows_region_and_entropy() {
    let mut db = make_db(vec![source("broker", 0.9, Some(90.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 400_000.0, 500_000.0),
        "broker",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let b = q.bound("obj1", "price", 0.95).unwrap();
    assert!(b.entropy_bits < b.max_entropy_bits);
    match b.map_estimate {
        Value::Num(m) => assert!((400_000.0..=500_000.0).contains(&m)),
        _ => panic!(),
    }
}

#[test]
fn erosion_widens_region_over_time() {
    let mut db = make_db(vec![source("broker", 0.9, Some(30.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 400_000.0, 500_000.0),
        "broker",
        0.0,
    );
    let fresh = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    let stale = Query::new(&db, day(365.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!(stale.entropy_bits > fresh.entropy_bits);
    assert!(region_width(&stale) > region_width(&fresh));
}

#[test]
fn axioms_do_not_erode() {
    let mut db = make_db(vec![axiom("land_registry")]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 500_000.0, 505_000.0),
        "land_registry",
        0.0,
    );
    let late = Query::new(&db, day(10_000.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!(region_width(&late) <= 10_000.0); // still (nearly) a point
}

#[test]
fn conflicting_axioms_raise() {
    let mut db = make_db(vec![axiom("registry_a"), axiom("registry_b")]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 100_000.0, 200_000.0),
        "registry_a",
        0.0,
    );
    ingest(
        &mut db,
        "obj1",
        interval("price", 700_000.0, 800_000.0),
        "registry_b",
        0.0,
    );
    let err = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap_err();
    assert!(matches!(err, Error::AxiomConflict(_)));
}

#[test]
fn soft_contradiction_is_uncertainty_not_conflict() {
    let mut db = make_db(vec![
        source("scraper_a", 0.8, Some(60.0)),
        source("scraper_b", 0.8, Some(60.0)),
    ]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 100_000.0, 200_000.0),
        "scraper_a",
        0.0,
    );
    ingest(
        &mut db,
        "obj1",
        interval("price", 700_000.0, 800_000.0),
        "scraper_b",
        0.0,
    );
    let b = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!(b.entropy_bits > 0.0);
}

#[test]
fn forget_source_widens_region() {
    let mut db = make_db(vec![source("shady_broker", 0.9, Some(90.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 400_000.0, 450_000.0),
        "shady_broker",
        0.0,
    );
    let before = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    let removed = db.forget_source("shady_broker").unwrap();
    assert_eq!(removed, 1);
    let after = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!(after.entropy_bits > before.entropy_bits);
    assert!((after.entropy_bits - after.max_entropy_bits).abs() < 1e-9);
}

#[test]
fn sample_is_deterministic_and_in_support() {
    let mut db = make_db(vec![source("broker", 0.95, Some(90.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 400_000.0, 500_000.0),
        "broker",
        0.0,
    );
    ingest(
        &mut db,
        "obj1",
        value("wants_to_sell", "true"),
        "broker",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let w1 = q.sample("obj1", 42).unwrap();
    let w2 = q.sample("obj1", 42).unwrap();
    let w3 = q.sample("obj1", 43).unwrap();
    assert_eq!(w1, w2);
    assert_ne!(w1, w3);
    match w1["price"] {
        Value::Num(p) => assert!((0.0..=1_000_000.0).contains(&p)),
        _ => panic!(),
    }
}

#[test]
fn three_valued_predicates() {
    // One soft source leaves residual mass outside -> only "possible".
    // Certainty requires corroboration by independent sources.
    let mut db = make_db(vec![
        source("registry", 0.999, None),
        source("notary_s", 0.999, None),
    ]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 600_000.0, 700_000.0),
        "registry",
        0.0,
    );
    ingest(
        &mut db,
        "obj1",
        interval("price", 600_000.0, 700_000.0),
        "notary_s",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    assert_eq!(
        q.certainly("obj1", "price", &Predicate::Gt { value: 500_000.0 })
            .unwrap(),
        Tri::True
    );
    assert_eq!(
        q.certainly("obj1", "price", &Predicate::Gt { value: 650_000.0 })
            .unwrap(),
        Tri::Possible
    );
    assert_eq!(
        q.certainly("obj1", "price", &Predicate::Gt { value: 900_000.0 })
            .unwrap(),
        Tri::False
    );
}

// ----------------------------------------------------------------- RESOLVE

fn act(name: &str, slot: &str, cost: f64, src: Source, width: Option<f64>) -> ProcurementAction {
    ProcurementAction {
        name: name.into(),
        slot: slot.into(),
        cost,
        source: src,
        answer_width: width,
    }
}

#[test]
fn resolve_plans_cheapest_informative_evidence() {
    let mut db = make_db(vec![source("web_scrape", 0.7, Some(30.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 300_000.0, 800_000.0),
        "web_scrape",
        0.0,
    );
    let actions = vec![
        act(
            "ask_owner",
            "price",
            100.0,
            source("owner", 0.95, Some(60.0)),
            Some(50_000.0),
        ),
        act(
            "buy_market_report",
            "price",
            500.0,
            source("report", 0.9, Some(180.0)),
            Some(100_000.0),
        ),
        act(
            "useless_forum_poll",
            "price",
            10.0,
            source("forum", 0.1, Some(7.0)),
            Some(900_000.0),
        ),
    ];
    let q = Query::new(&db, day(1.0));
    let plan = q.resolve("obj1", "price", 3.0, &actions, 10, 8, 0).unwrap();
    assert!(!plan.steps.is_empty());
    assert_eq!(plan.steps[0].action.name, "ask_owner"); // best gain per cost
    assert!(plan.planned_entropy_bits < plan.start_entropy_bits);
    let cost: f64 = plan.steps.iter().map(|s| s.action.cost).sum();
    assert_eq!(plan.total_cost, cost);
    for s in &plan.steps {
        assert!(s.expected_gain_bits > 0.0);
    }
}

#[test]
fn resolve_admits_when_nothing_helps() {
    let mut db = make_db(vec![axiom("registry")]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 500_000.0, 505_000.0),
        "registry",
        0.0,
    );
    let useless = act(
        "forum_poll",
        "price",
        10.0,
        source("forum", 0.05, Some(7.0)),
        Some(900_000.0),
    );
    let q = Query::new(&db, day(1.0));
    let plan = q
        .resolve("obj1", "price", 0.0, &[useless], 10, 8, 0)
        .unwrap();
    assert!(plan.steps.is_empty()); // the DB knows it cannot know more this way
}

// ------------------------------------------------------------ coupled slots

// ------------------------------------------------- decision-theoretic RESOLVE

#[test]
fn squared_error_resolve_reduces_variance_and_recommends_an_estimate() {
    // With a point-estimate decision, "risk" is the posterior variance and
    // the recommendation is the estimate itself — not a distribution.
    let mut db = make_db(vec![source("scrape", 0.7, Some(30.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 200_000.0, 800_000.0),
        "scrape",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let survey = act(
        "survey",
        "price",
        100.0,
        source("surveyor", 0.95, Some(60.0)),
        Some(40_000.0),
    );
    let plan = q
        .resolve_decision(
            "obj1",
            "price",
            &Objective::SquaredError,
            5e9,
            &[survey],
            10,
            8,
            0,
        )
        .unwrap();
    assert_eq!(plan.units, "variance");
    assert!(plan.start_risk > 0.0);
    assert!(!plan.steps.is_empty());
    assert!(plan.planned_risk < plan.start_risk);
    assert!(plan.recommended_now.starts_with("estimate"));
}

#[test]
fn decision_resolve_stops_when_the_decision_is_already_clear() {
    // A "buy vs. pass" call at a 500k budget. The assessor already places the
    // price well above budget, so the decision (pass) is settled — even
    // though plenty of entropy remains. Entropy-RESOLVE still spends to
    // narrow the region; decision-RESOLVE spends nothing, because no evidence
    // on offer would change the call. That gap is the Value of Information.
    let mut db = make_db(vec![source("assessor", 0.9, Some(180.0))]);
    ingest(
        &mut db,
        "deal",
        interval("price", 650_000.0, 750_000.0),
        "assessor",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let survey = act(
        "survey",
        "price",
        100.0,
        source("surveyor", 0.9, Some(180.0)),
        Some(50_000.0),
    );

    // Entropy still has an appetite for this evidence.
    let entropy_plan = q
        .resolve(
            "deal",
            "price",
            2.0,
            std::slice::from_ref(&survey),
            10,
            8,
            0,
        )
        .unwrap();
    assert!(!entropy_plan.steps.is_empty());

    // The decision does not.
    let n = 200;
    let budget = 500_000.0;
    let price = |i: usize| (i as f64 + 0.5) * (1_000_000.0 / n as f64);
    let scale = 1e-6;
    let buy: Vec<f64> = (0..n)
        .map(|i| (price(i) - budget).max(0.0) * scale)
        .collect();
    let pass: Vec<f64> = (0..n)
        .map(|i| (budget - price(i)).max(0.0) * scale)
        .collect();
    let objective = Objective::Decision {
        loss: vec![buy, pass],
        labels: Some(vec!["buy".into(), "pass".into()]),
    };
    let plan = q
        .resolve_decision(
            "deal",
            "price",
            &objective,
            0.03,
            std::slice::from_ref(&survey),
            10,
            8,
            0,
        )
        .unwrap();
    assert_eq!(plan.units, "loss");
    assert_eq!(plan.recommended_now, "pass"); // above budget → don't buy
    assert!(plan.start_risk < 0.03);
    assert!(
        plan.steps.is_empty(),
        "the decision is settled; no evidence is worth buying"
    );
}

#[test]
fn decision_objective_rejects_ill_formed_inputs() {
    let mut db = make_db(vec![source("s", 0.8, Some(90.0))]);
    ingest(
        &mut db,
        "e",
        interval("price", 100_000.0, 200_000.0),
        "s",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let a = act(
        "survey",
        "price",
        100.0,
        source("surv", 0.9, Some(60.0)),
        Some(40_000.0),
    );

    // A point-estimate objective on a categorical slot makes no sense.
    assert!(q
        .resolve_decision(
            "e",
            "wants_to_sell",
            &Objective::SquaredError,
            0.1,
            std::slice::from_ref(&a),
            5,
            4,
            0
        )
        .is_err());

    // A loss row must have one entry per cell.
    let bad = Objective::Decision {
        loss: vec![vec![0.0, 1.0, 2.0]],
        labels: None,
    };
    assert!(q
        .resolve_decision("e", "price", &bad, 0.1, &[a], 5, 4, 0)
        .is_err());
}

fn coupled_db(sources: Vec<Source>) -> Db {
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    slots.insert(
        "condition".to_string(),
        Domain::Categorical {
            values: vec!["renovated".into(), "original".into(), "derelict".into()],
        },
    );
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    let coupling = Coupling {
        slot_a: "condition".into(),
        slot_b: "price".into(),
        compat: Compat::GaussianByCategory {
            centers: [
                ("renovated".to_string(), 800_000.0),
                ("original".to_string(), 500_000.0),
                ("derelict".to_string(), 200_000.0),
            ]
            .into_iter()
            .collect(),
            sigma: 250_000.0,
        },
        name: Some("cond~price".into()),
    };
    Db::in_memory(
        Schema {
            slots,
            couplings: vec![coupling],
        },
        sources,
    )
    .unwrap()
}

#[test]
fn coupling_lets_knowledge_flow_between_slots() {
    let mut db = coupled_db(vec![source("visit", 0.95, Some(365.0))]);
    ingest(&mut db, "e1", value("condition", "derelict"), "visit", 0.0);
    let b = Query::new(&db, day(1.0))
        .bound("e1", "price", 0.95)
        .unwrap();
    // No direct price evidence, yet the price region is informed via coupling.
    assert!(b.entropy_bits < b.max_entropy_bits - 0.3);
    match b.map_estimate {
        Value::Num(m) => assert!(m < 500_000.0),
        _ => panic!(),
    }
}

#[test]
fn sample_respects_hard_coupling() {
    // A hard coupling: price > 500k is incompatible with 'derelict'.
    let n_bins = 200;
    let dom = price_domain();
    let mut rows = vec![vec![1.0; n_bins]; 3]; // renovated, original, derelict
    for (j, w) in rows[2].iter_mut().enumerate() {
        if dom.midpoint(j) > 500_000.0 {
            *w = 0.0;
        }
    }
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    slots.insert(
        "condition".to_string(),
        Domain::Categorical {
            values: vec!["renovated".into(), "original".into(), "derelict".into()],
        },
    );
    let coupling = Coupling {
        slot_a: "condition".into(),
        slot_b: "price".into(),
        compat: Compat::Table { rows },
        name: None,
    };
    let mut db = Db::in_memory(
        Schema {
            slots,
            couplings: vec![coupling],
        },
        vec![source("scraper", 0.6, Some(60.0))],
    )
    .unwrap();
    ingest(
        &mut db,
        "e1",
        interval("price", 400_000.0, 900_000.0),
        "scraper",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    for seed in 0..25 {
        let w = q.sample("e1", seed).unwrap();
        if w["condition"] == Value::Cat("derelict".into()) {
            match w["price"] {
                Value::Num(p) => assert!(p <= 505_000.0), // one bin of slack
                _ => panic!(),
            }
        }
    }
}

#[test]
fn resolve_plans_across_coupled_slots() {
    let mut db = coupled_db(vec![source("scrape", 0.6, Some(45.0))]);
    ingest(
        &mut db,
        "e1",
        interval("price", 100_000.0, 900_000.0),
        "scrape",
        0.0,
    );
    let visit = act(
        "site_visit",
        "condition",
        200.0,
        source("inspector", 0.95, Some(365.0)),
        None,
    );
    let q = Query::new(&db, day(1.0));
    let plan = q.resolve("e1", "price", 6.0, &[visit], 10, 6, 0).unwrap();
    // An action on ANOTHER slot reduces the price entropy through the coupling.
    assert!(!plan.steps.is_empty());
    assert_eq!(plan.steps[0].action.name, "site_visit");
    assert!(plan.steps[0].expected_gain_bits > 0.1);
    assert!(plan.validated_entropy_bits.is_some());
}

#[test]
fn resolve_mc_validation_is_deterministic() {
    let mut db = coupled_db(vec![source("s", 0.6, Some(45.0))]);
    ingest(
        &mut db,
        "e1",
        interval("price", 100_000.0, 900_000.0),
        "s",
        0.0,
    );
    let visit = act(
        "site_visit",
        "condition",
        200.0,
        source("inspector", 0.95, Some(365.0)),
        None,
    );
    let q = Query::new(&db, day(1.0));
    let p1 = q
        .resolve("e1", "price", 6.0, std::slice::from_ref(&visit), 10, 8, 5)
        .unwrap();
    let p2 = q.resolve("e1", "price", 6.0, &[visit], 10, 8, 5).unwrap();
    assert_eq!(p1.validated_entropy_bits, p2.validated_entropy_bits);
}

// ------------------------------------------------------------ shared priors

#[test]
fn shared_prior_is_knowledge_without_evidence() {
    let mut db = make_db(vec![]);
    let dom = price_domain();
    // Market prior: mass concentrated in 200k-600k, stored once...
    let weights: Vec<f64> = (0..200)
        .map(|i| {
            let m = dom.midpoint(i);
            if (200_000.0..=600_000.0).contains(&m) {
                1.0
            } else {
                0.05
            }
        })
        .collect();
    db.register_prior("market_2026", "price", weights).unwrap();
    // ...referenced by many entities (factorized shared assumption).
    for e in ["a", "b", "c"] {
        db.use_prior(e, "price", "market_2026").unwrap();
    }
    let q = Query::new(&db, 0);
    let b = q.bound("a", "price", 0.95).unwrap();
    assert!(b.knowledge_ratio() > 0.0 && b.knowledge_ratio() < 1.0);
    match b.map_estimate {
        Value::Num(m) => assert!((200_000.0..=600_000.0).contains(&m)),
        _ => panic!(),
    }
    let b2 = q.bound("b", "price", 0.95).unwrap();
    assert_eq!(b.entropy_bits, b2.entropy_bits);
}

// -------------------------------------------------------------------- FIND

#[test]
fn find_certain_and_possible() {
    let mut db = make_db(vec![axiom("notary"), source("scraper", 0.7, Some(60.0))]);
    ingest(
        &mut db,
        "cheap",
        interval("price", 200_000.0, 220_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "mid",
        interval("price", 450_000.0, 470_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "vague",
        interval("price", 100_000.0, 300_000.0),
        "scraper",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let certain = q.find("price", 0.0, 300_000.0, FindMode::Certain).unwrap();
    assert_eq!(certain, vec!["cheap".to_string()]); // 'vague' has soft support everywhere
    let possible = q.find("price", 0.0, 300_000.0, FindMode::Possible).unwrap();
    assert!(possible.contains(&"cheap".to_string()));
    assert!(possible.contains(&"vague".to_string()));
    // 'mid' is axiomatically outside [0, 300k] -> not even possible.
    assert!(!possible.contains(&"mid".to_string()));
}

// ------------------------------------------------------------- calibration

#[test]
fn fit_decay_recovers_half_life() {
    let mut rng = Rng::from_parts(&["calibration-test", "7"]);
    let (true_r0, true_hl) = (0.85, 60.0);
    // Ground truth must land within ~2 half-lives to identify r0 AND the
    // half-life; checks far beyond that see P(correct) ~ 0 for any r0.
    let pairs: Vec<(f64, bool)> = (0..600)
        .map(|_| {
            let age = 1.0 + rng.next_f64() * 119.0;
            let p = true_r0 * 0.5_f64.powf(age / true_hl);
            (age, rng.next_f64() < p)
        })
        .collect();
    let fit = fit_decay("broker", &pairs).unwrap();
    let hl = fit.half_life_days.expect("should find a finite half-life");
    assert!((30.0..=120.0).contains(&hl), "learned half-life {hl}");
    // r0 and half-life trade off along a likelihood ridge when most
    // observations are old; the identifiable object is the decay CURVE.
    // Require the learned P(correct | age) to track the true curve.
    for age in [10.0, 30.0, 60.0, 120.0, 240.0] {
        let p_true = true_r0 * 0.5_f64.powf(age / true_hl);
        let p_fit = fit.r0 * 0.5_f64.powf(age / hl);
        assert!(
            (p_fit - p_true).abs() <= 0.12,
            "curve off at age {age}: fit {p_fit:.2} vs true {p_true:.2}"
        );
    }
}

#[test]
fn calibration_pairs_and_apply_source() {
    // Broker believed near-immortal; ground truth says otherwise.
    let mut db = make_db(vec![source("broker", 0.9, Some(3650.0)), axiom("notary")]);
    let cases: [(&str, f64, f64); 4] = [
        ("e1", 450_000.0, 10.0),  // fresh, correct
        ("e2", 450_000.0, 20.0),  // fresh, correct
        ("e3", 800_000.0, 300.0), // stale, wrong
        ("e4", 900_000.0, 400.0), // stale, wrong
    ];
    for (ent, truth, age) in cases {
        ingest(
            &mut db,
            ent,
            interval("price", 400_000.0, 500_000.0),
            "broker",
            0.0,
        );
        ingest(
            &mut db,
            ent,
            interval("price", truth - 500.0, truth + 500.0),
            "notary",
            age,
        );
    }
    let pairs = calibration_pairs(&db.evidence, "broker", 0.99);
    assert_eq!(pairs.len(), 4);
    let mut sorted = pairs.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(
        sorted.iter().map(|p| p.1).collect::<Vec<_>>(),
        vec![true, true, false, false]
    );
    let fit = db.recalibrate_source("broker", 0.99).unwrap();
    assert!(fit.half_life_days.unwrap_or(f64::INFINITY) < 3650.0);
    // Applying the corrected physics changes derived regions.
    let before = Query::new(&db, day(200.0))
        .bound("e3", "price", 0.95)
        .unwrap()
        .entropy_bits;
    let n = db
        .put_source(Source {
            name: "broker".into(),
            reliability: fit.r0,
            half_life_days: fit.half_life_days,
            axiomatic: false,
        })
        .unwrap();
    assert_eq!(n, 4);
    let after = Query::new(&db, day(200.0))
        .bound("e3", "price", 0.95)
        .unwrap()
        .entropy_bits;
    assert_ne!(before, after);
}

// ------------------------------------------------------------- persistence

#[test]
fn persistence_roundtrip() {
    let dir = std::env::temp_dir().join(format!("nescio-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let mut slots = BTreeMap::new();
        slots.insert("price".to_string(), price_domain());
        slots.insert("wants_to_sell".to_string(), Domain::boolean());
        let mut db = Db::init(
            &dir,
            Schema {
                slots,
                couplings: vec![],
            },
            vec![source("broker", 0.9, Some(90.0)), axiom("notary")],
        )
        .unwrap();
        ingest(
            &mut db,
            "obj1",
            interval("price", 400_000.0, 500_000.0),
            "broker",
            0.0,
        );
        ingest(
            &mut db,
            "obj1",
            value("wants_to_sell", "true"),
            "broker",
            0.0,
        );
        ingest(
            &mut db,
            "obj2",
            interval("price", 700_000.0, 710_000.0),
            "notary",
            5.0,
        );
        let dom = price_domain();
        db.register_prior("market", "price", (0..dom.n()).map(|_| 1.0).collect())
            .unwrap();
        db.use_prior("obj3", "price", "market").unwrap();
    }
    // Reopen: everything must survive.
    let mut db = Db::open(&dir).unwrap();
    assert_eq!(db.evidence.len(), 3);
    assert_eq!(db.entities().count(), 3);
    let b = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!(b.entropy_bits < b.max_entropy_bits);

    // GDPR erasure survives reopen too — and is physical.
    let removed = db.forget_source("broker").unwrap();
    assert_eq!(removed, 2);
    drop(db);
    let db = Db::open(&dir).unwrap();
    assert_eq!(db.evidence.len(), 1);
    // Physical erasure: the source's name must be gone from the binary log.
    let log = std::fs::read(dir.join("log.bin")).unwrap();
    let needle = b"broker";
    assert!(
        !log.windows(needle.len()).any(|w| w == needle),
        "physically erased from the binary log"
    );
    let b = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!((b.entropy_bits - b.max_entropy_bits).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn binary_log_roundtrips_and_export_matches() {
    let dir = std::env::temp_dir().join(format!("nescio-binlog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    let mut db = Db::init(
        &dir,
        Schema {
            slots,
            couplings: vec![],
        },
        vec![source("broker", 0.9, Some(90.0))],
    )
    .unwrap();
    ingest(
        &mut db,
        "e1",
        interval("price", 400_000.0, 500_000.0),
        "broker",
        0.0,
    );
    ingest(&mut db, "e1", value("wants_to_sell", "true"), "broker", 0.0);

    // The on-disk file starts with the binary magic, not JSON.
    let bytes = std::fs::read(dir.join("log.bin")).unwrap();
    assert_eq!(&bytes[..8], nescio::binlog::MAGIC);

    // export reconstructs valid JSONL that itself re-parses to the same set.
    let jsonl = db.export_jsonl().unwrap();
    let lines: Vec<&str> = jsonl.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2);
    for line in lines {
        let _rec: EvidenceRecord = serde_json::from_str(line).unwrap();
    }
    drop(db);
    // Reopen from the binary log: state survives.
    let db = Db::open(&dir).unwrap();
    assert_eq!(db.evidence.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_jsonl_is_migrated_on_open() {
    let dir = std::env::temp_dir().join(format!("nescio-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Build a database, then downgrade it to the legacy layout by hand:
    // write log.jsonl and remove log.bin, as an old nescioDB would have.
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    {
        let mut db = Db::init(
            &dir,
            Schema {
                slots,
                couplings: vec![],
            },
            vec![source("broker", 0.9, Some(90.0))],
        )
        .unwrap();
        ingest(
            &mut db,
            "e1",
            interval("price", 400_000.0, 500_000.0),
            "broker",
            0.0,
        );
        ingest(
            &mut db,
            "e2",
            interval("price", 700_000.0, 710_000.0),
            "broker",
            0.0,
        );
    }
    let jsonl = Db::open(&dir).unwrap().export_jsonl().unwrap();
    std::fs::write(dir.join("log.jsonl"), &jsonl).unwrap();
    std::fs::remove_file(dir.join("log.bin")).unwrap();

    // Opening the legacy database migrates it to binary transparently.
    let db = Db::open(&dir).unwrap();
    assert_eq!(db.evidence.len(), 2);
    assert!(dir.join("log.bin").exists(), "migrated to binary");
    assert!(
        dir.join("log.jsonl.migrated").exists(),
        "old log kept as backup"
    );
    assert!(!dir.join("log.jsonl").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_source_and_slot_are_rejected() {
    let mut db = make_db(vec![]);
    let err = db
        .ingest(EvidenceRecord {
            entity: "e".into(),
            claim: interval("price", 0.0, 1.0),
            source: "ghost".into(),
            observed_at: 0,
        })
        .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)));
    let err = db
        .ingest(EvidenceRecord {
            entity: "e".into(),
            claim: interval("no_such_slot", 0.0, 1.0),
            source: "ghost".into(),
            observed_at: 0,
        })
        .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)));
}

#[test]
fn ingest_batch_is_atomic_and_durable() {
    let dir = std::env::temp_dir().join(format!("nescio-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    let mut db = Db::init(
        &dir,
        Schema {
            slots,
            couplings: vec![],
        },
        vec![source("broker", 0.9, Some(90.0))],
    )
    .unwrap();

    // A batch containing one invalid record must not land at all.
    let bad = vec![
        EvidenceRecord {
            entity: "a".into(),
            claim: interval("price", 100.0, 200.0),
            source: "broker".into(),
            observed_at: 0,
        },
        EvidenceRecord {
            entity: "b".into(),
            claim: interval("price", 100.0, 200.0),
            source: "ghost".into(), // unknown source
            observed_at: 0,
        },
    ];
    assert!(db.ingest_batch(bad).is_err());
    assert_eq!(db.evidence.len(), 0);
    // The log holds only its magic header — nothing from the rejected batch.
    assert_eq!(
        std::fs::read(dir.join("log.bin")).unwrap(),
        nescio::binlog::header()
    );

    // A valid batch lands completely and survives reopen.
    let good: Vec<EvidenceRecord> = (0..500)
        .map(|i| EvidenceRecord {
            entity: format!("e{i}"),
            claim: interval("price", 1000.0 * i as f64, 1000.0 * i as f64 + 500.0),
            source: "broker".into(),
            observed_at: 0,
        })
        .collect();
    assert_eq!(db.ingest_batch(good).unwrap(), 500);
    drop(db);
    let db = Db::open(&dir).unwrap();
    assert_eq!(db.evidence.len(), 500);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn uncoupled_fast_path_matches_full_inference() {
    // The fast path must be an optimization, never a semantic change:
    // an uncoupled slot's bound is identical whether or not a coupling
    // exists elsewhere in the schema.
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    slots.insert(
        "condition".to_string(),
        Domain::Categorical {
            values: vec!["renovated".into(), "original".into(), "derelict".into()],
        },
    );
    let coupling = Coupling {
        slot_a: "condition".into(),
        slot_b: "price".into(),
        compat: Compat::GaussianByCategory {
            centers: [("derelict".to_string(), 200_000.0)].into_iter().collect(),
            sigma: 250_000.0,
        },
        name: None,
    };
    let mut db = Db::in_memory(
        Schema {
            slots,
            couplings: vec![coupling],
        },
        vec![source("broker", 0.9, Some(90.0))],
    )
    .unwrap();
    // wants_to_sell is uncoupled -> fast path; its posterior must be pure
    // unary regardless of the coupled subgraph next to it.
    ingest(&mut db, "e1", value("wants_to_sell", "true"), "broker", 0.0);
    ingest(&mut db, "e1", value("condition", "derelict"), "broker", 0.0);
    let q = Query::new(&db, day(1.0));
    let b = q.bound("e1", "wants_to_sell", 0.95).unwrap();
    assert!(b.entropy_bits < 1.0);
    // And the coupled slot still feels the coupling.
    let bp = q.bound("e1", "price", 0.95).unwrap();
    match bp.map_estimate {
        Value::Num(m) => assert!(m < 600_000.0),
        _ => panic!(),
    }
}

// ------------------------------------------------------------------- JOIN

fn join_db() -> Db {
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), price_domain());
    slots.insert(
        "city".to_string(),
        Domain::Categorical {
            values: vec!["berlin".into(), "munich".into(), "hamburg".into()],
        },
    );
    Db::in_memory(
        Schema {
            slots,
            couplings: vec![],
        },
        vec![axiom("notary"), source("scraper", 0.7, Some(60.0))],
    )
    .unwrap()
}

#[test]
fn join_gt_certain_and_probability() {
    let mut db = join_db();
    // cheap is axiomatically ~200k, pricey ~800k, overlap is fuzzy.
    ingest(
        &mut db,
        "cheap",
        interval("price", 190_000.0, 210_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "pricey",
        interval("price", 790_000.0, 810_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "fuzzy",
        interval("price", 150_000.0, 850_000.0),
        "scraper",
        0.0,
    );
    let q = Query::new(&db, day(1.0));

    // pricey > cheap is regionally certain.
    let res = q
        .join(
            &JoinPredicate::Gt {
                left: "price".into(),
                right: "price".into(),
            },
            &JoinOptions {
                certain_only: true,
                ..Default::default()
            },
        )
        .unwrap();
    let names: Vec<(String, String)> = res
        .matches
        .iter()
        .map(|m| (m.left.clone(), m.right.clone()))
        .collect();
    assert!(names.contains(&("pricey".into(), "cheap".into())));
    // The reverse (cheap > pricey) must never appear as certain.
    assert!(!names.contains(&("cheap".into(), "pricey".into())));
    for m in &res.matches {
        assert_eq!(m.certainty, Tri::True);
        assert!(m.probability > 0.99);
    }
}

#[test]
fn join_approx_finds_comparables_symmetric() {
    let mut db = join_db();
    ingest(
        &mut db,
        "a",
        interval("price", 495_000.0, 505_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "b",
        interval("price", 500_000.0, 510_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "c",
        interval("price", 900_000.0, 910_000.0),
        "notary",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    let res = q
        .join(
            &JoinPredicate::Approx {
                left: "price".into(),
                right: "price".into(),
                tol: 50_000.0,
            },
            &JoinOptions::default(),
        )
        .unwrap();
    // a and b are within 50k; c is far. Symmetric self-join yields one pair.
    let pairs: Vec<(String, String)> = res
        .matches
        .iter()
        .map(|m| (m.left.clone(), m.right.clone()))
        .collect();
    assert_eq!(pairs, vec![("a".to_string(), "b".to_string())]);
    assert!(res.matches[0].probability > 0.9);
}

#[test]
fn join_same_is_entity_resolution() {
    let mut db = join_db();
    // Two records that agree on city (candidate duplicates), one that differs.
    ingest(&mut db, "rec1", value("city", "berlin"), "notary", 0.0);
    ingest(&mut db, "rec2", value("city", "berlin"), "notary", 0.0);
    ingest(&mut db, "rec3", value("city", "munich"), "notary", 0.0);
    let q = Query::new(&db, day(1.0));
    let res = q
        .join(
            &JoinPredicate::Same {
                left: "city".into(),
                right: "city".into(),
            },
            &JoinOptions::default(),
        )
        .unwrap();
    let pairs: Vec<(String, String)> = res
        .matches
        .iter()
        .map(|m| (m.left.clone(), m.right.clone()))
        .collect();
    assert_eq!(pairs, vec![("rec1".to_string(), "rec2".to_string())]);
    assert_eq!(res.matches[0].certainty, Tri::True);
    assert!(res.matches[0].probability > 0.99);
}

#[test]
fn join_prefix_expresses_entity_kinds() {
    let mut db = join_db();
    ingest(
        &mut db,
        "house_1",
        interval("price", 400_000.0, 420_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "house_2",
        interval("price", 600_000.0, 620_000.0),
        "notary",
        0.0,
    );
    ingest(
        &mut db,
        "owner_x",
        interval("price", 410_000.0, 415_000.0),
        "notary",
        0.0,
    );
    let q = Query::new(&db, day(1.0));
    // Match houses to owners whose budget is close — cross-kind by prefix.
    let res = q
        .join(
            &JoinPredicate::Approx {
                left: "price".into(),
                right: "price".into(),
                tol: 20_000.0,
            },
            &JoinOptions {
                left_prefix: Some("house_".into()),
                right_prefix: Some("owner_".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let pairs: Vec<(String, String)> = res
        .matches
        .iter()
        .map(|m| (m.left.clone(), m.right.clone()))
        .collect();
    // Only house_1 is within 20k of owner_x; house_2 is not.
    assert_eq!(pairs, vec![("house_1".to_string(), "owner_x".to_string())]);
}

#[test]
fn join_wrong_domain_kind_is_rejected() {
    let db = join_db();
    let q = Query::new(&db, 0);
    // gt on a categorical slot must error.
    assert!(q
        .join(
            &JoinPredicate::Gt {
                left: "city".into(),
                right: "city".into(),
            },
            &JoinOptions::default(),
        )
        .is_err());
    // same on a continuous slot must error.
    assert!(q
        .join(
            &JoinPredicate::Same {
                left: "price".into(),
                right: "price".into(),
            },
            &JoinOptions::default(),
        )
        .is_err());
}

// ------------------------------------------------------- schema evolution

fn condition_domain() -> Domain {
    Domain::Categorical {
        values: vec!["renovated".into(), "original".into(), "derelict".into()],
    }
}

fn condition_price_coupling() -> Coupling {
    Coupling {
        slot_a: "condition".into(),
        slot_b: "price".into(),
        compat: Compat::GaussianByCategory {
            centers: [
                ("renovated".to_string(), 900_000.0),
                ("derelict".to_string(), 100_000.0),
            ]
            .into_iter()
            .collect(),
            sigma: 150_000.0,
        },
        name: None,
    }
}

#[test]
fn added_slot_starts_at_maximal_entropy_then_narrows() {
    let mut db = make_db(vec![source("broker", 0.9, Some(90.0))]);
    ingest(
        &mut db,
        "obj1",
        interval("price", 400_000.0, 500_000.0),
        "broker",
        0.0,
    );
    db.add_slot("floor_area", price_domain()).unwrap();

    let q = Query::new(&db, day(1.0));
    let b = q.bound("obj1", "floor_area", 0.95).unwrap();
    assert!((b.entropy_bits - b.max_entropy_bits).abs() < 1e-9);

    ingest(
        &mut db,
        "obj1",
        interval("floor_area", 100_000.0, 200_000.0),
        "broker",
        1.0,
    );
    let q = Query::new(&db, day(1.0));
    let b = q.bound("obj1", "floor_area", 0.95).unwrap();
    assert!(b.entropy_bits < b.max_entropy_bits);
}

#[test]
fn add_slot_rejects_duplicates_and_bad_domains() {
    let mut db = make_db(vec![]);
    assert!(db.add_slot("price", price_domain()).is_err());
    assert!(db
        .add_slot(
            "bad",
            Domain::Continuous {
                lo: 10.0,
                hi: 5.0,
                n_bins: 100
            }
        )
        .is_err());
    assert!(db.add_slot("price", price_domain()).is_err());
}

#[test]
fn added_coupling_flows_knowledge_and_removal_restores_independence() {
    let mut db = make_db(vec![axiom("notary")]);
    db.add_slot("condition", condition_domain()).unwrap();
    ingest(
        &mut db,
        "obj1",
        value("condition", "renovated"),
        "notary",
        0.0,
    );

    let before = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!((before.entropy_bits - before.max_entropy_bits).abs() < 1e-9);

    db.add_coupling(condition_price_coupling()).unwrap();
    let coupled = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!(coupled.entropy_bits < before.entropy_bits);

    db.remove_coupling("condition~price").unwrap();
    let after = Query::new(&db, day(1.0))
        .bound("obj1", "price", 0.95)
        .unwrap();
    assert!((after.entropy_bits - before.entropy_bits).abs() < 1e-9);
}

#[test]
fn add_coupling_validates_slots_and_labels() {
    let mut db = make_db(vec![]);
    db.add_slot("condition", condition_domain()).unwrap();

    let mut unknown = condition_price_coupling();
    unknown.slot_b = "nope".into();
    assert!(db.add_coupling(unknown).is_err());
    assert!(db.schema.couplings.is_empty()); // nothing committed

    db.add_coupling(condition_price_coupling()).unwrap();
    assert!(db.add_coupling(condition_price_coupling()).is_err()); // duplicate label
    assert!(db.remove_coupling("no~such").is_err());
}

#[test]
fn remove_slot_refuses_while_coupled() {
    let mut db = make_db(vec![]);
    db.add_slot("condition", condition_domain()).unwrap();
    db.add_coupling(condition_price_coupling()).unwrap();
    assert!(db.remove_slot("condition").is_err());
    db.remove_coupling("condition~price").unwrap();
    assert!(db.remove_slot("condition").is_ok());
}

#[test]
fn add_value_extends_domain_couplings_and_priors() {
    let mut db = make_db(vec![axiom("notary")]);
    db.add_slot("condition", condition_domain()).unwrap();
    db.add_coupling(condition_price_coupling()).unwrap();
    db.register_prior("cond", "condition", vec![4.0, 2.0, 0.0])
        .unwrap();
    db.use_prior("obj1", "condition", "cond").unwrap();

    let extended = db.add_value("condition", "gutted").unwrap();
    assert_eq!(extended, 1);
    assert_eq!(db.domain("condition").unwrap().n(), 4);
    // The prior grew by its mean weight; the new value is not impossible,
    // and an axiom can collapse the region onto it.
    ingest(&mut db, "obj2", value("condition", "gutted"), "notary", 0.0);
    let b = Query::new(&db, day(1.0))
        .bound("obj2", "condition", 0.95)
        .unwrap();
    let Region::Values(vals) = &b.region else {
        panic!("expected categorical region")
    };
    assert_eq!(vals, &vec!["gutted".to_string()]);

    // Errors: continuous slot, duplicate value, unknown slot.
    assert!(db.add_value("price", "x").is_err());
    assert!(db.add_value("condition", "gutted").is_err());
    assert!(db.add_value("nope", "x").is_err());
}

#[test]
fn add_value_is_transactional_under_explicit_table_coupling() {
    let mut db = make_db(vec![]);
    db.add_slot("condition", condition_domain()).unwrap();
    db.add_coupling(Coupling {
        slot_a: "condition".into(),
        slot_b: "wants_to_sell".into(),
        compat: Compat::Table {
            rows: vec![vec![1.0, 1.0], vec![1.0, 1.0], vec![0.2, 1.0]],
        },
        name: None,
    })
    .unwrap();
    // An explicit 3x2 table cannot absorb a fourth category: refused,
    // and the domain must be unchanged.
    assert!(db.add_value("condition", "gutted").is_err());
    assert_eq!(db.domain("condition").unwrap().n(), 3);
}

#[test]
fn schema_evolution_persists_across_reopen() {
    let dir = std::env::temp_dir().join(format!("nescio-schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let mut slots = BTreeMap::new();
        slots.insert("price".to_string(), price_domain());
        let mut db = Db::init(
            &dir,
            Schema {
                slots,
                couplings: vec![],
            },
            vec![source("broker", 0.9, Some(90.0)), axiom("notary")],
        )
        .unwrap();
        ingest(
            &mut db,
            "obj1",
            interval("price", 400_000.0, 500_000.0),
            "broker",
            0.0,
        );
        db.add_slot("condition", condition_domain()).unwrap();
        db.add_coupling(condition_price_coupling()).unwrap();
        db.add_value("condition", "gutted").unwrap();
        ingest(&mut db, "obj1", value("condition", "gutted"), "notary", 0.0);
    }
    let db = Db::open(&dir).unwrap();
    assert_eq!(db.domain("condition").unwrap().n(), 4);
    assert_eq!(db.schema.couplings.len(), 1);
    assert_eq!(db.evidence.len(), 2);
    let b = Query::new(&db, day(1.0))
        .bound("obj1", "condition", 0.95)
        .unwrap();
    assert!(b.entropy_bits < b.max_entropy_bits);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_slot_erases_evidence_and_priors_physically() {
    let dir = std::env::temp_dir().join(format!("nescio-rmslot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let mut slots = BTreeMap::new();
        slots.insert("price".to_string(), price_domain());
        slots.insert("floor_area".to_string(), price_domain());
        let mut db = Db::init(
            &dir,
            Schema {
                slots,
                couplings: vec![],
            },
            vec![source("broker", 0.9, Some(90.0))],
        )
        .unwrap();
        ingest(
            &mut db,
            "obj1",
            interval("price", 400_000.0, 500_000.0),
            "broker",
            0.0,
        );
        ingest(
            &mut db,
            "obj1",
            interval("floor_area", 100.0, 200.0),
            "broker",
            0.0,
        );
        ingest(
            &mut db,
            "only_area",
            interval("floor_area", 100.0, 200.0),
            "broker",
            0.0,
        );
        let dom = price_domain();
        db.register_prior(
            "area_prior",
            "floor_area",
            (0..dom.n()).map(|_| 1.0).collect(),
        )
        .unwrap();
        db.use_prior("obj1", "floor_area", "area_prior").unwrap();

        let r = db.remove_slot("floor_area").unwrap();
        assert_eq!(r.evidence_erased, 2);
        assert_eq!(r.priors_removed, 1);
        assert!(db.remove_slot("floor_area").is_err()); // already gone
    }
    // Reopen: the log was rewritten, so replay must not see the dead slot.
    let db = Db::open(&dir).unwrap();
    assert_eq!(db.evidence.len(), 1);
    assert!(db.domain("floor_area").is_err());
    // The entity that only existed through the removed slot is gone too.
    assert!(!db.entities().any(|e| e == "only_area"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn evidence_records_accept_readable_dates_and_the_at_alias() {
    // Hand-written JSONL for `import` may use dates, like --at on the CLI.
    let rec: EvidenceRecord = serde_json::from_str(
        r#"{"entity":"e","claim":{"type":"interval","slot":"price","lo":1,"hi":2},"source":"s","observed_at":"2026-07-03"}"#,
    )
    .unwrap();
    assert_eq!(rec.observed_at, 20_637 * 86_400);

    let rec: EvidenceRecord = serde_json::from_str(
        r#"{"entity":"e","claim":{"type":"value","slot":"c","value":"x"},"source":"s","at":"2026-07-03T01:02:03"}"#,
    )
    .unwrap();
    assert_eq!(rec.observed_at, 20_637 * 86_400 + 3723);

    // Raw unix seconds keep working, and the canonical output stays numeric.
    let rec: EvidenceRecord = serde_json::from_str(
        r#"{"entity":"e","claim":{"type":"value","slot":"c","value":"x"},"source":"s","observed_at":1782345600}"#,
    )
    .unwrap();
    assert_eq!(rec.observed_at, 1_782_345_600);
    assert!(serde_json::to_string(&rec)
        .unwrap()
        .contains("\"observed_at\":1782345600"));

    // Garbage dates fail loudly.
    assert!(serde_json::from_str::<EvidenceRecord>(
        r#"{"entity":"e","claim":{"type":"value","slot":"c","value":"x"},"source":"s","observed_at":"gestern"}"#,
    )
    .is_err());
}
