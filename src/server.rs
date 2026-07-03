//! `nescio serve`: the database as an HTTP/JSON service.
//!
//! One process owns the database directory. Queries (including the
//! read-only POST /resolve) run concurrently under a shared read lock;
//! mutations take the write lock exclusively — many readers, one writer.
//!
//! Routes (all responses JSON):
//!
//! ```text
//! GET  /health
//! GET  /status
//! GET  /bound?entity=&slot=[&at=][&credible=]
//! GET  /sample?entity=[&seed=][&at=]
//! GET  /certainly?entity=&slot=&op=gt|lt|between|is|is_not[&value=][&lo=&hi=][&at=]
//! GET  /find?slot=&lo=&hi=[&mode=possible|certain][&at=]
//! POST /join           {predicate: {op, left, right, tol?}, options?, at?}
//! POST /ingest         {entity, claim, source, at?}
//! POST /ingest-batch   [{entity, claim, source, at?}, ...]  (group commit)
//! POST /resolve        {entity, slot, target_bits, actions, max_steps?, mc?, seed?, at?}
//! POST /sources        {name, reliability, half_life_days?, axiomatic?}
//! POST /forget-source  {source}
//! POST /recalibrate    {source, apply?, min_truth_reliability?}
//! POST /priors/register {name, slot, weights}
//! POST /priors/use      {entity, slot, name}
//! ```
//!
//! `at` accepts "YYYY-MM-DD", a date-time, or unix seconds; default now.
//! Axiom conflicts map to HTTP 409 — contradiction is a real state, not a
//! server failure.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::json;

use crate::engine::{FindMode, JoinOptions, JoinPredicate, Predicate, ProcurementAction, Query};
use crate::error::{Error, Result};
use crate::model::evidence::{Claim, EvidenceRecord, Source};
use crate::store::Db;
use crate::time::{now_unix, parse_when};

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

pub struct NescioServer {
    inner: tiny_http::Server,
}

impl NescioServer {
    pub fn bind(addr: &str) -> Result<Self> {
        let inner = tiny_http::Server::http(addr)
            .map_err(|e| Error::Invalid(format!("cannot bind {addr}: {e}")))?;
        Ok(NescioServer { inner })
    }

    pub fn port(&self) -> u16 {
        self.inner
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .unwrap_or(0)
    }

    /// Serve forever on a small thread pool. Queries (including the
    /// read-only POST /resolve) run concurrently under a read lock;
    /// mutations take the write lock exclusively — many readers, one
    /// writer, single process owning the directory.
    pub fn run(self, db: Db) -> Result<()> {
        let server = std::sync::Arc::new(self.inner);
        let db = std::sync::Arc::new(std::sync::RwLock::new(db));
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 8))
            .unwrap_or(4);
        let mut handles = Vec::new();
        for _ in 0..workers {
            let server = server.clone();
            let db = db.clone();
            handles.push(std::thread::spawn(move || {
                while let Ok(mut request) = server.recv() {
                    let method = request.method().as_str().to_string();
                    let url = request.url().to_string();
                    let (path, query) = match url.split_once('?') {
                        Some((p, q)) => (p.to_string(), q.to_string()),
                        None => (url, String::new()),
                    };
                    let body = if request.body_length().unwrap_or(0) > MAX_BODY_BYTES {
                        None
                    } else {
                        let mut s = String::new();
                        match request.as_reader().read_to_string(&mut s) {
                            Ok(_) => Some(s),
                            Err(_) => Some(String::new()),
                        }
                    };
                    let (status, payload) = match body {
                        None => (413, json!({"error": "body too large"}).to_string()),
                        Some(b) => {
                            if is_write(&method, &path) {
                                let mut guard = db.write().expect("db lock poisoned");
                                handle(&mut guard, &method, &path, &query, &b)
                            } else {
                                let guard = db.read().expect("db lock poisoned");
                                let params = parse_query(&query);
                                to_response(route_read(&guard, &method, &path, &params, &b))
                            }
                        }
                    };
                    let header = tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json"[..],
                    )
                    .expect("static header");
                    let response = tiny_http::Response::from_string(payload)
                        .with_status_code(status)
                        .with_header(header);
                    let _ = request.respond(response);
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
        Ok(())
    }
}

/// Routes that mutate the database and need the exclusive write lock.
/// POST /resolve and POST /join only read (they plan / match), so they
/// run under the shared read lock alongside the GET verbs.
fn is_write(method: &str, path: &str) -> bool {
    method == "POST" && path != "/resolve" && path != "/join"
}

/// Pure request handler: (method, path, query, body) -> (status, JSON).
pub fn handle(db: &mut Db, method: &str, path: &str, query: &str, body: &str) -> (u16, String) {
    let params = parse_query(query);
    let result = if is_write(method, path) {
        route_write(db, method, path, body)
    } else {
        route_read(db, method, path, &params, body)
    };
    to_response(result)
}

fn to_response(result: Result<String>) -> (u16, String) {
    match result {
        Ok(payload) => (200, payload),
        Err(Error::AxiomConflict(m)) => (
            409,
            json!({"error": format!("axiom conflict: {m}")}).to_string(),
        ),
        Err(Error::Invalid(m)) => {
            let status = if m.starts_with("no such route") {
                404
            } else {
                400
            };
            (status, json!({"error": m}).to_string())
        }
        Err(e) => (500, json!({"error": e.to_string()}).to_string()),
    }
}

fn route_read(
    db: &Db,
    method: &str,
    path: &str,
    params: &BTreeMap<String, String>,
    body: &str,
) -> Result<String> {
    match (method, path) {
        ("GET", "/health") => {
            Ok(json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}).to_string())
        }
        ("GET", "/status") => {
            let sources: Vec<&Source> = db.sources.values().collect();
            Ok(serde_json::to_string(&json!({
                "evidence": db.evidence.len(),
                "entities": db.entities().count(),
                "slots": db.schema.slots,
                "couplings": db.schema.couplings.iter().map(|c| c.label()).collect::<Vec<_>>(),
                "sources": sources,
            }))?)
        }
        ("GET", "/bound") => {
            let q = Query::new(db, at_param(params)?);
            let credible = num_param(params, "credible")?.unwrap_or(0.95);
            let b = q.bound(
                req_param(params, "entity")?,
                req_param(params, "slot")?,
                credible,
            )?;
            Ok(serde_json::to_string(&b)?)
        }
        ("GET", "/sample") => {
            let q = Query::new(db, at_param(params)?);
            let seed = num_param(params, "seed")?.unwrap_or(0.0) as u64;
            let world = q.sample(req_param(params, "entity")?, seed)?;
            Ok(serde_json::to_string(&world)?)
        }
        ("GET", "/certainly") => {
            let q = Query::new(db, at_param(params)?);
            let pred = predicate_from(params)?;
            let tri = q.certainly(
                req_param(params, "entity")?,
                req_param(params, "slot")?,
                &pred,
            )?;
            Ok(serde_json::to_string(&json!({"result": tri}))?)
        }
        ("GET", "/find") => {
            let q = Query::new(db, at_param(params)?);
            let lo = num_param(params, "lo")?
                .ok_or_else(|| Error::Invalid("missing param lo".into()))?;
            let hi = num_param(params, "hi")?
                .ok_or_else(|| Error::Invalid("missing param hi".into()))?;
            let mode: FindMode = params
                .get("mode")
                .map_or("possible", |s| s.as_str())
                .parse()?;
            let found = q.find(req_param(params, "slot")?, lo, hi, mode)?;
            Ok(serde_json::to_string(&found)?)
        }
        ("POST", "/resolve") => {
            #[derive(Deserialize)]
            struct Body {
                entity: String,
                slot: String,
                target_bits: f64,
                actions: Vec<ProcurementAction>,
                max_steps: Option<usize>,
                mc: Option<usize>,
                seed: Option<u64>,
                at: Option<serde_json::Value>,
            }
            let b: Body = from_body(body)?;
            let q = Query::new(db, at_value(b.at)?);
            let plan = q.resolve(
                &b.entity,
                &b.slot,
                b.target_bits,
                &b.actions,
                b.max_steps.unwrap_or(10),
                b.mc.unwrap_or(12),
                b.seed.unwrap_or(0),
            )?;
            Ok(serde_json::to_string(&plan)?)
        }
        ("POST", "/join") => {
            #[derive(Deserialize)]
            struct Body {
                predicate: JoinPredicate,
                #[serde(default)]
                options: JoinOptions,
                #[serde(default)]
                at: Option<serde_json::Value>,
            }
            let b: Body = from_body(body)?;
            let q = Query::new(db, at_value(b.at)?);
            let res = q.join(&b.predicate, &b.options)?;
            Ok(serde_json::to_string(&res)?)
        }
        _ => Err(Error::Invalid(format!("no such route: {method} {path}"))),
    }
}

#[derive(Deserialize)]
struct IngestBody {
    entity: String,
    claim: Claim,
    source: String,
    at: Option<serde_json::Value>,
}

impl IngestBody {
    fn into_record(self) -> Result<EvidenceRecord> {
        Ok(EvidenceRecord {
            entity: self.entity,
            claim: self.claim,
            source: self.source,
            observed_at: at_value(self.at)?,
        })
    }
}

fn route_write(db: &mut Db, method: &str, path: &str, body: &str) -> Result<String> {
    match (method, path) {
        ("POST", "/ingest") => {
            let b: IngestBody = from_body(body)?;
            let rec = b.into_record()?;
            let observed_at = rec.observed_at;
            db.ingest(rec)?;
            Ok(json!({"ok": true, "observed_at": observed_at}).to_string())
        }
        ("POST", "/ingest-batch") => {
            let bodies: Vec<IngestBody> = from_body(body)?;
            let recs = bodies
                .into_iter()
                .map(IngestBody::into_record)
                .collect::<Result<Vec<_>>>()?;
            let n = db.ingest_batch(recs)?;
            Ok(json!({"ok": true, "ingested": n}).to_string())
        }
        ("POST", "/sources") => {
            let source: Source = from_body(body)?;
            let n = db.put_source(source)?;
            Ok(json!({"ok": true, "reinterpreted": n}).to_string())
        }
        ("POST", "/forget-source") => {
            #[derive(Deserialize)]
            struct Body {
                source: String,
            }
            let b: Body = from_body(body)?;
            let removed = db.forget_source(&b.source)?;
            Ok(json!({"ok": true, "erased": removed}).to_string())
        }
        ("POST", "/recalibrate") => {
            #[derive(Deserialize)]
            struct Body {
                source: String,
                apply: Option<bool>,
                min_truth_reliability: Option<f64>,
            }
            let b: Body = from_body(body)?;
            let fit = db.recalibrate_source(&b.source, b.min_truth_reliability.unwrap_or(0.99))?;
            let mut applied = 0;
            if b.apply.unwrap_or(false) {
                let axiomatic = db.sources.get(&b.source).is_some_and(|s| s.axiomatic);
                applied = db.put_source(Source {
                    name: b.source.clone(),
                    reliability: fit.r0,
                    half_life_days: fit.half_life_days,
                    axiomatic,
                })?;
            }
            Ok(serde_json::to_string(
                &json!({"fit": fit, "applied": applied}),
            )?)
        }
        ("POST", "/priors/register") => {
            #[derive(Deserialize)]
            struct Body {
                name: String,
                slot: String,
                weights: Vec<f64>,
            }
            let b: Body = from_body(body)?;
            db.register_prior(&b.name, &b.slot, b.weights)?;
            Ok(json!({"ok": true}).to_string())
        }
        ("POST", "/priors/use") => {
            #[derive(Deserialize)]
            struct Body {
                entity: String,
                slot: String,
                name: String,
            }
            let b: Body = from_body(body)?;
            db.use_prior(&b.entity, &b.slot, &b.name)?;
            Ok(json!({"ok": true}).to_string())
        }
        _ => Err(Error::Invalid(format!("no such route: {method} {path}"))),
    }
}

// ---------------------------------------------------------------- helpers

fn from_body<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(|e| Error::Invalid(format!("bad request body: {e}")))
}

fn req_param<'a>(params: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .map(|s| s.as_str())
        .ok_or_else(|| Error::Invalid(format!("missing param {key}")))
}

fn num_param(params: &BTreeMap<String, String>, key: &str) -> Result<Option<f64>> {
    match params.get(key) {
        None => Ok(None),
        Some(s) => s
            .parse::<f64>()
            .map(Some)
            .map_err(|_| Error::Invalid(format!("param {key} is not a number"))),
    }
}

fn at_param(params: &BTreeMap<String, String>) -> Result<i64> {
    match params.get("at") {
        Some(s) => parse_when(s),
        None => Ok(now_unix()),
    }
}

/// `at` in a JSON body: string date, numeric unix seconds, or absent (now).
fn at_value(v: Option<serde_json::Value>) -> Result<i64> {
    match v {
        None => Ok(now_unix()),
        Some(serde_json::Value::String(s)) => parse_when(&s),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| Error::Invalid("'at' must be an integer or date string".into())),
        Some(_) => Err(Error::Invalid(
            "'at' must be an integer or date string".into(),
        )),
    }
}

fn predicate_from(params: &BTreeMap<String, String>) -> Result<Predicate> {
    let op = req_param(params, "op")?;
    let value_num = || {
        num_param(params, "value")?
            .ok_or_else(|| Error::Invalid("missing numeric param value".into()))
    };
    match op {
        "gt" => Ok(Predicate::Gt {
            value: value_num()?,
        }),
        "lt" => Ok(Predicate::Lt {
            value: value_num()?,
        }),
        "between" => {
            let lo = num_param(params, "lo")?
                .ok_or_else(|| Error::Invalid("missing param lo".into()))?;
            let hi = num_param(params, "hi")?
                .ok_or_else(|| Error::Invalid("missing param hi".into()))?;
            Ok(Predicate::Between { lo, hi })
        }
        "is" => Ok(Predicate::Is {
            value: req_param(params, "value")?.to_string(),
        }),
        "is_not" => Ok(Predicate::IsNot {
            value: req_param(params, "value")?.to_string(),
        }),
        _ => Err(Error::Invalid(format!("unknown predicate op {op:?}"))),
    }
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
