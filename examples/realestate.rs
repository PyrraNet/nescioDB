//! Property intelligence: every concept in one story.
//!
//! Real-estate data is the canonical nescioDB workload — prices are
//! uncertain, sources disagree and age at different rates, and attributes
//! constrain each other. This walks through every verb on one portfolio.
//!
//! ```bash
//! cargo run --example realestate
//! ```

use std::collections::BTreeMap;

use nesciodb::prelude::*;

const DAY: i64 = 86_400;

fn day(d: f64) -> i64 {
    (d * DAY as f64) as i64
}

fn show(q: &Query, entity: &str, slot: &str, label: &str) {
    let b = q.bound(entity, slot, 0.95).unwrap();
    let region = match &b.region {
        Region::Intervals(ivs) => ivs
            .iter()
            .map(|(a, c)| format!("[{a:.0}, {c:.0}]"))
            .collect::<Vec<_>>()
            .join(" u "),
        Region::Values(vs) => format!("{{{}}}", vs.join(", ")),
    };
    println!("  {label:<26} {region}");
    println!(
        "  {:<26} {:.2} of {:.2} bits (knowledge {:.0}%, MAP {})",
        "",
        b.entropy_bits,
        b.max_entropy_bits,
        b.knowledge_ratio() * 100.0,
        b.map_estimate
    );
}

fn main() {
    // ---------------------------------------------------------------- schema
    let mut slots = BTreeMap::new();
    slots.insert(
        "price".to_string(),
        Domain::Continuous {
            lo: 0.0,
            hi: 2_000_000.0,
            n_bins: 200,
        },
    );
    slots.insert(
        "condition".to_string(),
        Domain::Categorical {
            values: vec!["renovated".into(), "original".into(), "derelict".into()],
        },
    );
    slots.insert("for_sale".to_string(), Domain::boolean());
    slots.insert(
        "year_built".to_string(),
        Domain::Continuous {
            lo: 1900.0,
            hi: 2026.0,
            n_bins: 126,
        },
    );

    // Coupling rules: knowledge flows between slots.
    let couplings = vec![
        Coupling {
            slot_a: "condition".into(),
            slot_b: "price".into(),
            compat: Compat::GaussianByCategory {
                centers: [
                    ("renovated".to_string(), 1_300_000.0),
                    ("original".to_string(), 900_000.0),
                    ("derelict".to_string(), 500_000.0),
                ]
                .into_iter()
                .collect(),
                sigma: 300_000.0,
            },
            name: Some("condition~price".into()),
        },
        Coupling {
            slot_a: "year_built".into(),
            slot_b: "condition".into(),
            compat: Compat::StepThreshold {
                threshold: 1980.0,
                below: [("renovated".to_string(), 0.5)].into_iter().collect(),
                above: [("derelict".to_string(), 0.2)].into_iter().collect(),
            },
            name: Some("year_built~condition".into()),
        },
    ];

    // Sources of different reliability, decaying at different rates.
    let sources = vec![
        Source {
            name: "land_registry".into(),
            reliability: 1.0,
            half_life_days: None,
            axiomatic: true,
        },
        Source {
            name: "broker".into(),
            reliability: 0.85,
            half_life_days: Some(90.0),
            axiomatic: false,
        },
        Source {
            name: "web_scraper".into(),
            reliability: 0.7,
            half_life_days: Some(45.0),
            axiomatic: false,
        },
        Source {
            name: "neighbor".into(),
            reliability: 0.4,
            half_life_days: Some(30.0),
            axiomatic: false,
        },
    ];

    let mut db = Db::in_memory(Schema { slots, couplings }, sources).unwrap();

    // ------------------------------------------------------------- evidence
    let obj = "villa_lakeside_4";
    let ev = |entity: &str, claim: Claim, source: &str, at_day: f64| EvidenceRecord {
        entity: entity.into(),
        claim,
        source: source.into(),
        observed_at: day(at_day),
    };
    let interval = |slot: &str, lo: f64, hi: f64| Claim::Interval {
        slot: slot.into(),
        lo,
        hi,
    };
    let value = |slot: &str, v: &str| Claim::Value {
        slot: slot.into(),
        value: v.into(),
    };

    db.ingest(ev(
        obj,
        interval("year_built", 1962.0, 1963.0),
        "land_registry",
        0.0,
    ))
    .unwrap();
    db.ingest(ev(
        obj,
        interval("price", 800_000.0, 1_100_000.0),
        "web_scraper",
        10.0,
    ))
    .unwrap();
    db.ingest(ev(
        obj,
        interval("price", 900_000.0, 1_000_000.0),
        "broker",
        40.0,
    ))
    .unwrap();
    db.ingest(ev(obj, value("for_sale", "true"), "neighbor", 55.0))
        .unwrap();
    db.ingest(ev(obj, value("for_sale", "true"), "broker", 60.0))
        .unwrap();

    // A few more priced properties, notarized, for the JOIN.
    for (name, lo, hi) in [
        ("terrace_elm", 495_000.0, 505_000.0),
        ("semi_lark", 500_000.0, 510_000.0),
        ("penthouse_dome", 1_800_000.0, 1_850_000.0),
    ] {
        db.ingest(ev(name, interval("price", lo, hi), "land_registry", 50.0))
            .unwrap();
    }

    let today = day(70.0);
    let q = Query::new(&db, today);
    let sep = "=".repeat(70);

    println!("{sep}\nBOUND — what does the DB know about {obj}? (day 70)\n{sep}");
    show(&q, obj, "year_built", "year_built");
    show(&q, obj, "price", "price");
    show(&q, obj, "condition", "condition (no evidence!)");
    println!("  -> No condition claim exists. The 1962 land-registry axiom and the");
    println!("     price band flow through the couplings and shape it anyway.\n");

    println!("{sep}\nEROSION — the same query a year later, no new evidence\n{sep}");
    let q_future = Query::new(&db, day(70.0 + 365.0));
    show(&q, obj, "price", "price today");
    show(&q_future, obj, "price", "price in a year");
    show(&q_future, obj, "year_built", "year_built in a year");
    println!("  -> Soft evidence erodes; the land-registry axiom does not.\n");

    println!("{sep}\nThree-valued predicates — region containment, not comparison\n{sep}");
    for (label, slot, pred) in [
        (
            "year_built < 1980?",
            "year_built",
            Predicate::Lt { value: 1980.0 },
        ),
        (
            "year_built > 2000?",
            "year_built",
            Predicate::Gt { value: 2000.0 },
        ),
        ("price > 500k?", "price", Predicate::Gt { value: 500_000.0 }),
    ] {
        println!("  {label:<22} {}", q.certainly(obj, slot, &pred).unwrap());
    }
    println!();

    println!("{sep}\nSAMPLE — one consistent world, deterministic under seed 7\n{sep}");
    for (slot, v) in q.sample(obj, 7).unwrap() {
        println!("  {slot:<14} {v}");
    }
    println!("  -> Drawn by the chain rule: couplings hold in every world.\n");

    println!("{sep}\nRESOLVE — cut a vague property's price entropy by a site visit\n{sep}");
    let visit = ProcurementAction {
        name: "site_visit".into(),
        slot: "condition".into(),
        cost: 200.0,
        source: Source {
            name: "inspector".into(),
            reliability: 0.95,
            half_life_days: Some(365.0),
            axiomatic: false,
        },
        answer_width: None,
    };
    // terrace_elm has a tight price already; give a genuinely vague one.
    db.ingest(ev(
        "plot_marsh",
        interval("price", 200_000.0, 1_400_000.0),
        "web_scraper",
        50.0,
    ))
    .unwrap();
    let q = Query::new(&db, today);
    let plan = q
        .resolve("plot_marsh", "price", 4.0, &[visit], 10, 16, 1)
        .unwrap();
    println!("  price entropy now: {:.2} bits", plan.start_entropy_bits);
    for (i, s) in plan.steps.iter().enumerate() {
        println!(
            "  {}. {} (slot {}) -> expected {:.2} bits",
            i + 1,
            s.action.name,
            s.action.slot,
            s.expected_entropy_bits
        );
    }
    if let Some(v) = plan.validated_entropy_bits {
        println!("  Monte-Carlo-validated: {v:.2} bits");
    }
    println!("  -> A question about CONDITION lowers PRICE entropy via the coupling.\n");

    println!("{sep}\nJOIN — pairs whose relation is itself uncertain\n{sep}");
    let comparables = q
        .join(
            &JoinPredicate::Approx {
                left: "price".into(),
                right: "price".into(),
                tol: 50_000.0,
            },
            &JoinOptions {
                min_probability: 0.5,
                limit: 5,
                ..Default::default()
            },
        )
        .unwrap();
    println!("  Comparable (price within 50k, P>=0.5):");
    for m in &comparables.matches {
        println!(
            "    {:<18} ~ {:<18} p={:.2}  {}",
            m.left, m.right, m.probability, m.certainty
        );
    }
    let dearer = q
        .join(
            &JoinPredicate::Gt {
                left: "price".into(),
                right: "price".into(),
            },
            &JoinOptions {
                certain_only: true,
                limit: 3,
                ..Default::default()
            },
        )
        .unwrap();
    println!("  Certainly dearer (region containment, top 3):");
    for m in &dearer.matches {
        println!(
            "    {:<18} > {:<18} p={:.2}  {}",
            m.left, m.right, m.probability, m.certainty
        );
    }
    println!("  -> Independent entities => exact integral over the product measure,");
    println!(
        "     no sampling. {} pairs examined.\n",
        comparables.pairs_examined + dearer.pairs_examined
    );

    println!("{sep}\nGDPR — the neighbor retracts\n{sep}");
    show(&q, obj, "for_sale", "before");
    db.forget_source("neighbor").unwrap();
    let q = Query::new(&db, today);
    show(&q, obj, "for_sale", "after");
    println!("  -> No aggregate to forget: the region widens automatically.");
}
