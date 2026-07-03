//! Measured performance, not adjectives.
//!
//! ```bash
//! cargo run --release --example bench            # 50k entities
//! cargo run --release --example bench -- 200000  # custom size
//! ```
//!
//! Two schemas are measured: uncoupled (the common case, fast path) and
//! coupled (belief propagation on every query — honest numbers included).

use std::collections::BTreeMap;
use std::time::Instant;

use nescio::prelude::*;
use nescio::rng::Rng;
use nescio::time::SECONDS_PER_DAY;

const DAY: i64 = SECONDS_PER_DAY as i64;

fn schema(coupled: bool) -> Schema {
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
    let couplings = if coupled {
        vec![Coupling {
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
            name: None,
        }]
    } else {
        vec![]
    };
    Schema { slots, couplings }
}

fn sources() -> Vec<Source> {
    vec![
        Source {
            name: "notary".into(),
            reliability: 1.0,
            half_life_days: None,
            axiomatic: true,
        },
        Source {
            name: "scraper".into(),
            reliability: 0.7,
            half_life_days: Some(45.0),
            axiomatic: false,
        },
    ]
}

fn records(n_entities: usize) -> Vec<EvidenceRecord> {
    let mut rng = Rng::from_parts(&["bench-data"]);
    let mut recs = Vec::with_capacity(n_entities * 2);
    for i in 0..n_entities {
        let entity = format!("e{i}");
        let center = 100_000.0 + rng.next_f64() * 1_700_000.0;
        let source = if i % 10 == 0 { "notary" } else { "scraper" };
        recs.push(EvidenceRecord {
            entity: entity.clone(),
            claim: Claim::Interval {
                slot: "price".into(),
                lo: (center - 60_000.0).max(0.0),
                hi: (center + 60_000.0).min(2_000_000.0),
            },
            source: source.into(),
            observed_at: (i as i64 % 60) * DAY,
        });
        recs.push(EvidenceRecord {
            entity,
            claim: Claim::Value {
                slot: "condition".into(),
                value: ["renovated", "original", "derelict"][i % 3].into(),
            },
            source: "scraper".into(),
            observed_at: (i as i64 % 60) * DAY,
        });
    }
    recs
}

fn bench_schema(label: &str, coupled: bool, n: usize) {
    let dir = std::env::temp_dir().join(format!("nescio-bench-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut db = Db::init(&dir, schema(coupled), sources()).unwrap();

    let recs = records(n);
    let n_recs = recs.len();
    let t = Instant::now();
    db.ingest_batch(recs).unwrap();
    let ingest_s = t.elapsed().as_secs_f64();

    drop(db);
    let t = Instant::now();
    let db = Db::open(&dir).unwrap();
    let open_s = t.elapsed().as_secs_f64();

    let as_of = 90 * DAY;
    let q = Query::new(&db, as_of);
    let t = Instant::now();
    let probes = 1_000.min(n);
    for i in 0..probes {
        q.bound(
            &format!("e{}", i * (n / probes.max(1)).max(1) % n),
            "price",
            0.95,
        )
        .unwrap();
    }
    let bound_us = t.elapsed().as_secs_f64() * 1e6 / probes as f64;

    let q = Query::new(&db, as_of);
    let t = Instant::now();
    let hits = q
        .find("price", 0.0, 600_000.0, FindMode::Certain)
        .unwrap()
        .len();
    let find_s = t.elapsed().as_secs_f64();

    let actions = vec![ProcurementAction {
        name: "ask".into(),
        slot: "price".into(),
        cost: 50.0,
        source: Source {
            name: "owner".into(),
            reliability: 0.9,
            half_life_days: Some(120.0),
            axiomatic: false,
        },
        answer_width: Some(100_000.0),
    }];
    let t = Instant::now();
    q.resolve("e1", "price", 3.0, &actions, 10, 12, 0).unwrap();
    let resolve_ms = t.elapsed().as_secs_f64() * 1e3;

    // Approx self-join with a tight tolerance: selective, so the support-hull
    // pruning keeps it well below the O(N^2) worst case.
    let t = Instant::now();
    let jr = q
        .join(
            &JoinPredicate::Approx {
                left: "price".into(),
                right: "price".into(),
                tol: 2_000.0,
            },
            &JoinOptions {
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
    let join_ms = t.elapsed().as_secs_f64() * 1e3;

    println!("--- {label} ({n} entities, {n_recs} evidence records) ---");
    println!(
        "  ingest_batch: {:>10.0} records/s  ({ingest_s:.2}s, one fsync)",
        n_recs as f64 / ingest_s
    );
    println!(
        "  open/replay:  {:>10.0} records/s  ({open_s:.2}s)",
        n_recs as f64 / open_s
    );
    println!("  bound:        {bound_us:>10.1} us/query");
    println!("  find certain: {find_s:>10.3} s over {n} entities ({hits} hits)");
    println!("  resolve:      {resolve_ms:>10.1} ms (1 action, mc=12)");
    // Wide, uncertain regions make almost every pair regionally-possible —
    // an adversarial case for an approx join. Report the data-independent
    // invariant: probability integrals per second. (A selective join over
    // tight regions examines far fewer pairs and finishes in microseconds.)
    println!(
        "  join approx:  {:>10.0} pairs/s ({} examined{})",
        jr.pairs_examined as f64 / (join_ms / 1e3),
        jr.pairs_examined,
        if jr.truncated { ", capped" } else { "" }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000);
    bench_schema("uncoupled", false, n);
    bench_schema("coupled", true, n);
}
