//! Watches: trigger semantics, knowledge-horizon prediction, persistence,
//! cascade on remove_slot, the HTTP routes, and the SSE stream.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use nescio::prelude::*;
use nescio::server::{handle, NescioServer};

const DAY: i64 = 86_400;

fn schema_and_sources() -> (Schema, Vec<Source>) {
    let mut slots = BTreeMap::new();
    slots.insert(
        "price".to_string(),
        Domain::Continuous {
            lo: 0.0,
            hi: 1_000_000.0,
            n_bins: 200,
        },
    );
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    let sources = vec![
        Source {
            name: "broker".into(),
            reliability: 0.9,
            half_life_days: Some(90.0),
            axiomatic: false,
        },
        Source {
            name: "registry_a".into(),
            reliability: 1.0,
            half_life_days: None,
            axiomatic: true,
        },
        Source {
            name: "registry_b".into(),
            reliability: 1.0,
            half_life_days: None,
            axiomatic: true,
        },
    ];
    (
        Schema {
            slots,
            couplings: vec![],
        },
        sources,
    )
}

fn test_db() -> Db {
    let (schema, sources) = schema_and_sources();
    Db::in_memory(schema, sources).unwrap()
}

fn watch(name: &str, entity: &str, slot: &str, max_bits: f64) -> Watch {
    Watch {
        name: name.into(),
        entity: entity.into(),
        slot: slot.into(),
        max_entropy_bits: Some(max_bits),
        min_knowledge: None,
    }
}

fn ingest_price(db: &mut Db, source: &str, lo: f64, hi: f64, at: i64) {
    db.ingest(EvidenceRecord {
        entity: "villa_1".into(),
        claim: Claim::Interval {
            slot: "price".into(),
            lo,
            hi,
        },
        source: source.into(),
        observed_at: at,
    })
    .unwrap();
}

#[test]
fn horizon_is_predicted_to_the_day() {
    let mut db = test_db();
    ingest_price(&mut db, "broker", 400_000.0, 500_000.0, 0);
    let at = DAY;
    let e_now = Query::new(&db, at)
        .bound("villa_1", "price", 0.95)
        .unwrap()
        .entropy_bits;
    let w = watch("price_fresh", "villa_1", "price", e_now + 1.0);
    let st = evaluate_watch(&db, &w, at, DEFAULT_HORIZON_DAYS);
    assert!(!st.triggered);
    let h = st.horizon.expect("a decaying source must yield a horizon");
    assert!(h > at);
    // Sharp to the day: still quiet the day before, fired at the horizon.
    assert!(!evaluate_watch(&db, &w, h - DAY, DEFAULT_HORIZON_DAYS).triggered);
    assert!(evaluate_watch(&db, &w, h, DEFAULT_HORIZON_DAYS).triggered);
    // Already-triggered watches carry the evaluation time as horizon.
    assert_eq!(
        evaluate_watch(&db, &w, h, DEFAULT_HORIZON_DAYS).horizon,
        Some(h)
    );
}

#[test]
fn min_knowledge_is_threshold_sugar() {
    let mut db = test_db();
    ingest_price(&mut db, "broker", 400_000.0, 500_000.0, 0);
    let max = db.domain("price").unwrap().max_entropy_bits();
    let w = Watch {
        name: "k".into(),
        entity: "villa_1".into(),
        slot: "price".into(),
        max_entropy_bits: None,
        min_knowledge: Some(0.4),
    };
    let st = evaluate_watch(&db, &w, DAY, DEFAULT_HORIZON_DAYS);
    let threshold = st.threshold_bits.unwrap();
    assert!((threshold - 0.6 * max).abs() < 1e-9);
    let e = st.entropy_bits.unwrap();
    assert!((st.knowledge.unwrap() - (1.0 - e / max)).abs() < 1e-9);
}

#[test]
fn axiomatic_evidence_has_no_horizon() {
    let mut db = test_db();
    ingest_price(&mut db, "registry_a", 400_000.0, 500_000.0, 0);
    let e_now = Query::new(&db, DAY)
        .bound("villa_1", "price", 0.95)
        .unwrap()
        .entropy_bits;
    let w = watch("pinned", "villa_1", "price", e_now + 0.5);
    let st = evaluate_watch(&db, &w, DAY, DEFAULT_HORIZON_DAYS);
    assert!(!st.triggered);
    assert!(st.horizon.is_none(), "an axiom without decay never lets go");
}

#[test]
fn axiom_conflict_fires_the_watch() {
    let mut db = test_db();
    ingest_price(&mut db, "registry_a", 0.0, 100_000.0, 0);
    ingest_price(&mut db, "registry_b", 900_000.0, 1_000_000.0, 0);
    // A threshold no entropy could ever exceed: only the conflict fires it.
    let w = watch("conflicted", "villa_1", "price", 100.0);
    let st = evaluate_watch(&db, &w, DAY, DEFAULT_HORIZON_DAYS);
    assert!(st.triggered);
    assert!(st.error.unwrap().contains("axiom"));
}

#[test]
fn watches_validate_persist_and_remove() {
    let dir = std::env::temp_dir().join(format!("nescio-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (schema, sources) = schema_and_sources();
    let mut db = Db::init(&dir, schema, sources).unwrap();
    db.add_watch(watch("w1", "villa_1", "price", 5.0)).unwrap();
    // rejected: duplicate name, unknown slot, zero or two conditions, bad ratio
    assert!(db.add_watch(watch("w1", "x", "price", 4.0)).is_err());
    assert!(db.add_watch(watch("w2", "x", "colour", 4.0)).is_err());
    assert!(db
        .add_watch(Watch {
            name: "w3".into(),
            entity: "x".into(),
            slot: "price".into(),
            max_entropy_bits: None,
            min_knowledge: None,
        })
        .is_err());
    assert!(db
        .add_watch(Watch {
            name: "w4".into(),
            entity: "x".into(),
            slot: "price".into(),
            max_entropy_bits: Some(3.0),
            min_knowledge: Some(0.5),
        })
        .is_err());
    assert!(db
        .add_watch(Watch {
            name: "w5".into(),
            entity: "x".into(),
            slot: "price".into(),
            max_entropy_bits: None,
            min_knowledge: Some(1.5),
        })
        .is_err());
    drop(db);
    let mut db = Db::open(&dir).unwrap();
    assert_eq!(db.watches.len(), 1);
    assert_eq!(db.watches[0].name, "w1");
    db.remove_watch("w1").unwrap();
    assert!(db.remove_watch("w1").is_err());
    drop(db);
    let db = Db::open(&dir).unwrap();
    assert!(db.watches.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_slot_takes_its_watches() {
    let mut db = test_db();
    db.add_watch(watch("wp", "villa_1", "price", 5.0)).unwrap();
    db.add_watch(watch("ws", "villa_1", "wants_to_sell", 0.5))
        .unwrap();
    let r = db.remove_slot("price").unwrap();
    assert_eq!(r.watches_removed, 1);
    assert_eq!(db.watches.len(), 1);
    assert_eq!(db.watches[0].slot, "wants_to_sell");
}

#[test]
fn watch_routes_over_http() {
    let mut db = test_db();
    ingest_price(&mut db, "broker", 400_000.0, 500_000.0, 0);
    let (s, body) = handle(
        &mut db,
        "POST",
        "/watches",
        "",
        r#"{"name":"w1","entity":"villa_1","slot":"price","max_entropy_bits":5.0}"#,
    );
    assert_eq!(s, 200, "{body}");
    assert!(body.contains("\"state\"") && body.contains("\"threshold_bits\""));
    // invalid: two conditions
    let (s, _) = handle(
        &mut db,
        "POST",
        "/watches",
        "",
        r#"{"name":"w2","entity":"e","slot":"price","max_entropy_bits":5.0,"min_knowledge":0.4}"#,
    );
    assert_eq!(s, 400);
    let (s, body) = handle(&mut db, "GET", "/watches", "", "");
    assert_eq!(s, 200);
    assert!(body.contains("\"w1\"") && body.contains("\"horizon\""));
    // far in the future the watch has fired
    let (s, body) = handle(&mut db, "GET", "/watches/check", "at=2036-01-01", "");
    assert_eq!(s, 200, "{body}");
    assert!(body.contains("\"triggered\":[{"), "{body}");
    let (s, _) = handle(&mut db, "POST", "/watches/remove", "", r#"{"name":"w1"}"#);
    assert_eq!(s, 200);
    let (s, _) = handle(&mut db, "POST", "/watches/remove", "", r#"{"name":"w1"}"#);
    assert_eq!(s, 400);
}

fn raw_request(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status: u16 = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let payload = raw
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, payload)
}

/// Read from the SSE stream until `needle` shows up (chunked framing may
/// interleave, so search the accumulated text) or the deadline passes.
fn read_until(stream: &mut TcpStream, acc: &mut String, needle: &str, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 4096];
    while !acc.contains(needle) {
        assert!(
            Instant::now() < deadline,
            "SSE stream never delivered {needle:?}; got so far: {acc}"
        );
        match stream.read(&mut buf) {
            Ok(0) => panic!("SSE stream closed early; got so far: {acc}"),
            Ok(n) => acc.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("SSE read error: {e}"),
        }
    }
}

#[test]
fn sse_stream_delivers_snapshot_and_transitions() {
    let server = NescioServer::bind("127.0.0.1:0").unwrap();
    let port = server.port();
    std::thread::spawn(move || server.run(test_db()).unwrap());

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    stream
        .write_all(
            b"GET /watches/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
        )
        .unwrap();
    let mut acc = String::new();
    read_until(&mut stream, &mut acc, "event: snapshot", 5);
    assert!(acc.contains("text/event-stream"));

    // A watch on an entity without evidence sits at maximal entropy, so a
    // 1-bit threshold has already fired — the write wakes the evaluator,
    // which must push the transition to the open stream.
    let (s, body) = raw_request(
        port,
        "POST",
        "/watches",
        r#"{"name":"stale","entity":"nobody","slot":"price","max_entropy_bits":1.0}"#,
    );
    assert_eq!(s, 200, "{body}");
    read_until(&mut stream, &mut acc, "event: triggered", 20);
    assert!(acc.contains("\"stale\""));
}
