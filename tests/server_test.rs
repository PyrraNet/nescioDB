//! HTTP API tests: a real server on an ephemeral port, raw TCP requests.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use nesciodb::prelude::*;
use nesciodb::server::NescioServer;

fn test_db() -> Db {
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
    Db::in_memory(
        Schema {
            slots,
            couplings: vec![],
        },
        sources,
    )
    .unwrap()
}

fn start_server() -> u16 {
    let server = NescioServer::bind("127.0.0.1:0").unwrap();
    let port = server.port();
    let db = test_db();
    std::thread::spawn(move || server.run(db).unwrap());
    port
}

fn request(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
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

#[test]
fn http_api_end_to_end() {
    let port = start_server();

    // Health.
    let (status, body) = request(port, "GET", "/health", "");
    assert_eq!(status, 200);
    assert!(body.contains("\"ok\":true"));

    // Ingest evidence at day 0 (unix 0).
    let (status, _) = request(
        port,
        "POST",
        "/ingest",
        r#"{"entity":"obj1","claim":{"type":"interval","slot":"price","lo":400000,"hi":500000},"source":"broker","at":0}"#,
    );
    assert_eq!(status, 200);

    // BOUND one day later reflects it.
    let (status, body) = request(port, "GET", "/bound?entity=obj1&slot=price&at=86400", "");
    assert_eq!(status, 200);
    let bound: serde_json::Value = serde_json::from_str(&body).unwrap();
    let entropy = bound["entropy_bits"].as_f64().unwrap();
    assert!(entropy < bound["max_entropy_bits"].as_f64().unwrap());

    // Same query a year later: erosion has widened the region.
    let year = 366 * 86_400;
    let (_, body) = request(
        port,
        "GET",
        &format!("/bound?entity=obj1&slot=price&at={year}"),
        "",
    );
    let stale: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(stale["entropy_bits"].as_f64().unwrap() > entropy);

    // Three-valued predicate: one soft source leaves residual mass
    // everywhere, so even far-off ranges stay "possible" — certainty
    // requires corroboration.
    let (status, body) = request(
        port,
        "GET",
        "/certainly?entity=obj1&slot=price&op=gt&value=900000&at=86400",
        "",
    );
    assert_eq!(status, 200);
    assert!(body.contains("possible"));

    // SAMPLE is deterministic across requests.
    let (_, w1) = request(port, "GET", "/sample?entity=obj1&seed=7&at=86400", "");
    let (_, w2) = request(port, "GET", "/sample?entity=obj1&seed=7&at=86400", "");
    assert_eq!(w1, w2);

    // FIND.
    let (status, body) = request(port, "GET", "/find?slot=price&lo=0&hi=600000&at=86400", "");
    assert_eq!(status, 200);
    assert!(body.contains("obj1"));

    // RESOLVE over the wire.
    let (status, body) = request(
        port,
        "POST",
        "/resolve",
        r#"{"entity":"obj1","slot":"price","target_bits":3.0,"at":86400,
            "actions":[{"name":"ask_owner","slot":"price","cost":100,
                        "source":{"name":"owner","reliability":0.95,"half_life_days":60},
                        "answer_width":50000}]}"#,
    );
    assert_eq!(status, 200);
    let plan: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(plan["steps"][0]["action"]["name"], "ask_owner");
    assert!(plan["validated_entropy_bits"].as_f64().is_some());

    // GDPR erasure widens the region back to maximal ignorance.
    let (status, body) = request(port, "POST", "/forget-source", r#"{"source":"broker"}"#);
    assert_eq!(status, 200);
    assert!(body.contains("\"erased\":1"));
    let (_, body) = request(port, "GET", "/bound?entity=obj1&slot=price&at=86400", "");
    let after: serde_json::Value = serde_json::from_str(&body).unwrap();
    let h = after["entropy_bits"].as_f64().unwrap();
    assert!((h - after["max_entropy_bits"].as_f64().unwrap()).abs() < 1e-9);

    // Axiom conflict is a 409, not a server error.
    request(
        port,
        "POST",
        "/ingest",
        r#"{"entity":"c1","claim":{"type":"interval","slot":"price","lo":100000,"hi":200000},"source":"registry_a","at":0}"#,
    );
    request(
        port,
        "POST",
        "/ingest",
        r#"{"entity":"c1","claim":{"type":"interval","slot":"price","lo":700000,"hi":800000},"source":"registry_b","at":0}"#,
    );
    let (status, body) = request(port, "GET", "/bound?entity=c1&slot=price&at=86400", "");
    assert_eq!(status, 409);
    assert!(body.contains("axiom conflict"));

    // Bad requests are 400, unknown routes 404.
    let (status, _) = request(port, "GET", "/bound?entity=obj1", "");
    assert_eq!(status, 400);
    let (status, _) = request(port, "GET", "/nope", "");
    assert_eq!(status, 404);
}

#[test]
fn concurrent_reads_and_batch_ingest() {
    let port = start_server();

    // Group-commit a batch over the wire.
    let batch: Vec<String> = (0..50)
        .map(|i| {
            format!(
                r#"{{"entity":"b{i}","claim":{{"type":"interval","slot":"price","lo":{},"hi":{}}},"source":"broker","at":0}}"#,
                i * 1000,
                i * 1000 + 500
            )
        })
        .collect();
    let (status, body) = request(
        port,
        "POST",
        "/ingest-batch",
        &format!("[{}]", batch.join(",")),
    );
    assert_eq!(status, 200);
    assert!(body.contains("\"ingested\":50"));

    // Hammer the server with parallel reads; every response must be
    // complete and correct (shared read lock, no interleaving).
    let mut handles = Vec::new();
    for t in 0..8 {
        handles.push(std::thread::spawn(move || {
            for i in 0..25 {
                let e = (t * 25 + i) % 50;
                let (status, body) = request(
                    port,
                    "GET",
                    &format!("/bound?entity=b{e}&slot=price&at=86400"),
                    "",
                );
                assert_eq!(status, 200);
                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(v["entity"], format!("b{e}"));
                assert!(v["entropy_bits"].as_f64().unwrap() < 7.65);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}
