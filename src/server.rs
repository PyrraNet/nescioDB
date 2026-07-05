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
//! POST /decide         {entity, slot, objective, target, actions, max_steps?, mc?, seed?, at?}
//! POST /sources        {name, reliability, half_life_days?, axiomatic?}
//! POST /forget-source  {source}
//! POST /recalibrate    {source, apply?, min_truth_reliability?}
//! POST /priors/register {name, slot, weights}
//! POST /priors/use      {entity, slot, name}
//! POST /schema/add-slot        {name, domain}
//! POST /schema/remove-slot     {name}
//! POST /schema/add-value       {slot, value}
//! POST /schema/add-coupling    {slot_a, slot_b, compat, name?}
//! POST /schema/remove-coupling {name}
//! GET  /watches[?at=]          every watch: state + knowledge horizon
//! GET  /watches/check[?at=]    only the triggered ones
//! GET  /watches/events         Server-Sent Events: snapshot, then
//!                              triggered / recovered transitions
//! POST /watches         {name, entity, slot, max_entropy_bits?|min_knowledge?}
//! POST /watches/remove  {name}
//! ```
//!
//! `at` accepts "YYYY-MM-DD", a date-time, or unix seconds; default now.
//! Axiom conflicts map to HTTP 409 — contradiction is a real state, not a
//! server failure.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::engine::{
    FindMode, JoinOptions, JoinPredicate, Objective, Predicate, ProcurementAction, Query,
};
use crate::error::{Error, Result};
use crate::model::coupling::Coupling;
use crate::model::domain::Domain;
use crate::model::evidence::{Claim, EvidenceRecord, Source};
use crate::store::Db;
use crate::time::{now_unix, parse_when};
use crate::watch::{check_watches, evaluate_watch, Watch, WatchState, DEFAULT_HORIZON_DAYS};

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The watch evaluator re-checks at least this often; it is also woken by
/// every write. Doubles as the SSE ping cadence.
const HEARTBEAT_SECS: u64 = 15;

/// Live `/watches/events` connections, each fed by cloned frame strings.
type Subscribers = Arc<Mutex<Vec<mpsc::Sender<String>>>>;

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
    ///
    /// A background evaluator re-checks the watches after every write and
    /// at least every `HEARTBEAT_SECS`, pushing `triggered` / `recovered`
    /// transitions to `/watches/events` subscribers.
    pub fn run(self, db: Db) -> Result<()> {
        let server = Arc::new(self.inner);
        let db = Arc::new(RwLock::new(db));
        let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
        let (notify_tx, notify_rx) = mpsc::channel::<()>();
        {
            let db = db.clone();
            let subs = subscribers.clone();
            std::thread::spawn(move || watch_evaluator(&db, &subs, &notify_rx));
        }
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 8))
            .unwrap_or(4);
        let mut handles = Vec::new();
        for _ in 0..workers {
            let server = server.clone();
            let db = db.clone();
            let subs = subscribers.clone();
            let notify = notify_tx.clone();
            handles.push(std::thread::spawn(move || {
                while let Ok(mut request) = server.recv() {
                    let method = request.method().as_str().to_string();
                    let url = request.url().to_string();
                    let (path, query) = match url.split_once('?') {
                        Some((p, q)) => (p.to_string(), q.to_string()),
                        None => (url, String::new()),
                    };
                    if method == "GET" && path == "/watches/events" {
                        subscribe_events(request, &db, &subs);
                        continue;
                    }
                    let body = if request.body_length().unwrap_or(0) > MAX_BODY_BYTES {
                        None
                    } else {
                        let mut s = String::new();
                        match request.as_reader().read_to_string(&mut s) {
                            Ok(_) => Some(s),
                            Err(_) => Some(String::new()),
                        }
                    };
                    let writing = is_write(&method, &path);
                    let (status, payload) = match body {
                        None => (413, json!({"error": "body too large"}).to_string()),
                        Some(b) => {
                            if writing {
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
                    if writing && status < 400 {
                        // Any write can move a knowledge horizon.
                        let _ = notify.send(());
                    }
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- watches

/// Background thread: evaluate all watches, emit SSE events on state
/// transitions, sleep until the next write or heartbeat.
fn watch_evaluator(db: &RwLock<Db>, subs: &Subscribers, notify: &mpsc::Receiver<()>) {
    let mut last: BTreeMap<String, bool> = BTreeMap::new();
    loop {
        let states = {
            let guard = db.read().expect("db lock poisoned");
            check_watches(&guard, now_unix(), DEFAULT_HORIZON_DAYS)
        };
        for s in &states {
            let was = last.get(&s.watch.name).copied();
            if s.triggered && was != Some(true) {
                broadcast(subs, &sse_frame("triggered", &state_json(s)));
            } else if !s.triggered && was == Some(true) {
                broadcast(subs, &sse_frame("recovered", &state_json(s)));
            }
        }
        last = states
            .iter()
            .map(|s| (s.watch.name.clone(), s.triggered))
            .collect();
        // The ping keeps intermediaries from timing the stream out and
        // flushes closed connections out of the subscriber list.
        broadcast(subs, ": ping\n\n");
        match notify.recv_timeout(Duration::from_secs(HEARTBEAT_SECS)) {
            Ok(()) => while notify.try_recv().is_ok() {}, // coalesce bursts
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Answer `GET /watches/events`: register a channel, send the snapshot,
/// and hand the connection its own thread — a stream that lives for the
/// life of the client must not occupy a pool worker. The response is
/// written raw ([`tiny_http::Request::into_writer`]) with a flush per
/// frame; tiny_http's own response path buffers until the body ends,
/// which a body that never ends cannot afford.
fn subscribe_events(request: tiny_http::Request, db: &RwLock<Db>, subs: &Subscribers) {
    let (tx, rx) = mpsc::channel::<String>();
    let at = now_unix();
    let snapshot = {
        let guard = db.read().expect("db lock poisoned");
        let states = check_watches(&guard, at, DEFAULT_HORIZON_DAYS);
        json!({"as_of": at, "watches": states}).to_string()
    };
    let _ = tx.send(sse_frame("snapshot", &snapshot));
    subs.lock().expect("subscribers lock poisoned").push(tx);
    let mut writer = request.into_writer();
    std::thread::spawn(move || {
        let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                      Cache-Control: no-cache\r\nConnection: close\r\n\r\n";
        let mut send = |bytes: &[u8]| {
            writer
                .write_all(bytes)
                .and_then(|()| writer.flush())
                .is_ok()
        };
        if !send(header.as_bytes()) {
            return;
        }
        while let Ok(frame) = rx.recv() {
            if !send(frame.as_bytes()) {
                return; // client gone; the next broadcast prunes the sender
            }
        }
    });
}

fn broadcast(subs: &Subscribers, frame: &str) {
    subs.lock()
        .expect("subscribers lock poisoned")
        .retain(|tx| tx.send(frame.to_string()).is_ok());
}

fn sse_frame(event: &str, data: &str) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn state_json(s: &WatchState) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "{}".into())
}

/// Routes that mutate the database and need the exclusive write lock.
/// POST /resolve, /decide and /join only read (they plan / match), so they
/// run under the shared read lock alongside the GET verbs.
fn is_write(method: &str, path: &str) -> bool {
    method == "POST" && !matches!(path, "/resolve" | "/decide" | "/join")
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
        ("GET", "/watches") => {
            let at = at_param(params)?;
            let states = check_watches(db, at, DEFAULT_HORIZON_DAYS);
            Ok(serde_json::to_string(
                &json!({"as_of": at, "watches": states}),
            )?)
        }
        ("GET", "/watches/check") => {
            let at = at_param(params)?;
            let states = check_watches(db, at, DEFAULT_HORIZON_DAYS);
            let triggered: Vec<_> = states.iter().filter(|s| s.triggered).collect();
            Ok(serde_json::to_string(&json!({
                "as_of": at,
                "checked": states.len(),
                "triggered": triggered,
            }))?)
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
        ("POST", "/decide") => {
            #[derive(Deserialize)]
            struct Body {
                entity: String,
                slot: String,
                objective: Objective,
                target: f64,
                actions: Vec<ProcurementAction>,
                max_steps: Option<usize>,
                mc: Option<usize>,
                seed: Option<u64>,
                at: Option<serde_json::Value>,
            }
            let b: Body = from_body(body)?;
            let q = Query::new(db, at_value(b.at)?);
            let plan = q.resolve_decision(
                &b.entity,
                &b.slot,
                &b.objective,
                b.target,
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
        ("POST", "/schema/add-slot") => {
            #[derive(Deserialize)]
            struct Body {
                name: String,
                domain: Domain,
            }
            let b: Body = from_body(body)?;
            db.add_slot(&b.name, b.domain)?;
            Ok(json!({"ok": true}).to_string())
        }
        ("POST", "/schema/remove-slot") => {
            #[derive(Deserialize)]
            struct Body {
                name: String,
            }
            let b: Body = from_body(body)?;
            let r = db.remove_slot(&b.name)?;
            Ok(json!({
                "ok": true,
                "evidence_erased": r.evidence_erased,
                "priors_removed": r.priors_removed,
                "watches_removed": r.watches_removed,
            })
            .to_string())
        }
        ("POST", "/watches") => {
            let w: Watch = from_body(body)?;
            db.add_watch(w.clone())?;
            let state = evaluate_watch(db, &w, now_unix(), DEFAULT_HORIZON_DAYS);
            Ok(serde_json::to_string(&json!({"ok": true, "state": state}))?)
        }
        ("POST", "/watches/remove") => {
            #[derive(Deserialize)]
            struct Body {
                name: String,
            }
            let b: Body = from_body(body)?;
            db.remove_watch(&b.name)?;
            Ok(json!({"ok": true}).to_string())
        }
        ("POST", "/schema/add-value") => {
            #[derive(Deserialize)]
            struct Body {
                slot: String,
                value: String,
            }
            let b: Body = from_body(body)?;
            let extended = db.add_value(&b.slot, &b.value)?;
            Ok(json!({"ok": true, "priors_extended": extended}).to_string())
        }
        ("POST", "/schema/add-coupling") => {
            let c: Coupling = from_body(body)?;
            db.add_coupling(c)?;
            Ok(json!({"ok": true}).to_string())
        }
        ("POST", "/schema/remove-coupling") => {
            #[derive(Deserialize)]
            struct Body {
                name: String,
            }
            let b: Body = from_body(body)?;
            db.remove_coupling(&b.name)?;
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
