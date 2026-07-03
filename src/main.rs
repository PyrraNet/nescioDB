//! `nescio` — CLI for nescioDB.
//!
//! A database is a directory (schema.json, sources.json, priors.json,
//! log.bin). Every query takes `--at` (default: now) — time travel is a
//! parameter.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use nescio::prelude::*;
use nescio::time::{format_unix, now_unix, parse_when};

#[derive(Parser)]
#[command(
    name = "nescio",
    version,
    about = "nescioDB: a database whose primary object is ignorance"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new database directory
    Init {
        dir: PathBuf,
        /// Schema JSON file ({"slots": {...}, "couplings": [...]})
        #[arg(long)]
        schema: Option<PathBuf>,
        /// Sources JSON file ([{name, reliability, half_life_days?, axiomatic?}, ...])
        #[arg(long)]
        sources: Option<PathBuf>,
        /// Start from a built-in template instead: "real-estate"
        #[arg(long)]
        template: Option<String>,
    },
    /// Show slots, sources, entities and log size
    Status { dir: PathBuf },
    /// Register or update a source
    Source {
        dir: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        reliability: f64,
        #[arg(long)]
        half_life_days: Option<f64>,
        #[arg(long)]
        axiomatic: bool,
    },
    /// Append evidence to the log
    Ingest {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long)]
        slot: String,
        #[command(flatten)]
        claim: ClaimArgs,
        #[arg(long)]
        source: String,
        /// When observed (YYYY-MM-DD, date-time, or unix seconds; default now)
        #[arg(long)]
        at: Option<String>,
    },
    /// BOUND: credible region + entropy — how ignorant is the DB?
    Bound {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long)]
        slot: String,
        #[arg(long, default_value_t = 0.95)]
        credible: f64,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// SAMPLE: draw one consistent world, deterministic under the seed
    Sample {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Three-valued predicate: true / possible / false (region containment)
    Certainly {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long)]
        slot: String,
        #[command(flatten)]
        pred: PredArgs,
        #[arg(long)]
        at: Option<String>,
    },
    /// FIND: entities whose region certainly lies in / possibly intersects a range
    Find {
        dir: PathBuf,
        #[arg(long)]
        slot: String,
        #[arg(long)]
        lo: f64,
        #[arg(long)]
        hi: f64,
        /// "possible" or "certain"
        #[arg(long, default_value = "possible")]
        mode: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// RESOLVE: plan the minimal-cost evidence to reach an entropy target
    Resolve {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long)]
        slot: String,
        #[arg(long)]
        target_bits: f64,
        /// JSON file: [{name, slot, cost, source: {...}, answer_width?}, ...]
        #[arg(long)]
        actions: PathBuf,
        #[arg(long, default_value_t = 10)]
        max_steps: usize,
        #[arg(long, default_value_t = 12)]
        mc: usize,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// DECIDE: plan evidence that most improves a *decision*, not just
    /// entropy — the Value of Information for the call you actually face
    Decide {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long)]
        slot: String,
        /// JSON file describing the objective, e.g.
        /// {"kind":"squared_error"} or
        /// {"kind":"decision","loss":[[..],[..]],"labels":["buy","pass"]}
        #[arg(long)]
        objective: PathBuf,
        /// Stop once the Bayes risk (in the objective's units) is at or below this
        #[arg(long)]
        target: f64,
        /// JSON file: [{name, slot, cost, source: {...}, answer_width?}, ...]
        #[arg(long)]
        actions: PathBuf,
        #[arg(long, default_value_t = 10)]
        max_steps: usize,
        #[arg(long, default_value_t = 12)]
        mc: usize,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// JOIN: relational predicate between entities, under uncertainty
    Join {
        dir: PathBuf,
        /// gt | lt | approx | same
        #[arg(long)]
        op: String,
        /// Left entity's slot
        #[arg(long)]
        left: String,
        /// Right entity's slot
        #[arg(long)]
        right: String,
        /// Tolerance for --op approx
        #[arg(long)]
        tol: Option<f64>,
        /// Restrict left side to entities whose id starts with this
        #[arg(long)]
        left_prefix: Option<String>,
        #[arg(long)]
        right_prefix: Option<String>,
        /// Keep only matches with at least this probability
        #[arg(long, default_value_t = 0.0)]
        min_prob: f64,
        /// Keep only regionally-certain matches
        #[arg(long)]
        certain: bool,
        /// Also join entities that have no evidence on the slot
        #[arg(long)]
        all_entities: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Manage shared priors (stored once, referenced by many entities)
    Prior {
        #[command(subcommand)]
        cmd: PriorCmd,
    },
    /// GDPR erasure: physically remove all evidence from a source
    ForgetSource {
        dir: PathBuf,
        #[arg(long)]
        source: String,
    },
    /// Learn a source's decay physics from ground truth in the log
    Recalibrate {
        dir: PathBuf,
        #[arg(long)]
        source: String,
        /// Write the fitted reliability/half-life back to sources.json
        #[arg(long)]
        apply: bool,
        #[arg(long, default_value_t = 0.99)]
        min_truth_reliability: f64,
    },
    /// Bulk-import evidence from a JSONL file (one record per line)
    /// with a single group commit
    Import { dir: PathBuf, file: PathBuf },
    /// Export the binary log as human-readable JSONL (to a file, or stdout
    /// if omitted) — for debugging and diffing
    Export { dir: PathBuf, file: Option<PathBuf> },
    /// Serve the database as an HTTP/JSON API
    Serve {
        dir: PathBuf,
        #[arg(long, default_value_t = 7777)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
    },
}

#[derive(Subcommand)]
enum PriorCmd {
    /// Register a shared prior for a slot
    Register {
        dir: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        slot: String,
        /// JSON file with a weights array matching the slot's cell count
        #[arg(long)]
        weights_file: Option<PathBuf>,
        /// Or a Gaussian "CENTER,SIGMA" over a continuous slot
        #[arg(long)]
        gaussian: Option<String>,
    },
    /// Assign a registered prior to an entity's slot
    Use {
        dir: PathBuf,
        #[arg(long)]
        entity: String,
        #[arg(long)]
        slot: String,
        #[arg(long)]
        name: String,
    },
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct ClaimArgs {
    /// Interval claim "LO..HI" (continuous slots)
    #[arg(long)]
    interval: Option<String>,
    /// Value claim (categorical slots)
    #[arg(long)]
    value: Option<String>,
    /// Negative value claim (categorical slots)
    #[arg(long)]
    not_value: Option<String>,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct PredArgs {
    #[arg(long)]
    gt: Option<f64>,
    #[arg(long)]
    lt: Option<f64>,
    /// "LO..HI"
    #[arg(long)]
    between: Option<String>,
    #[arg(long)]
    is: Option<String>,
    #[arg(long)]
    is_not: Option<String>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Init {
            dir,
            schema,
            sources,
            template,
        } => {
            let (schema, sources) = match (&schema, &sources, template.as_deref()) {
                (None, None, Some("real-estate")) => real_estate_template(),
                (None, None, Some(t)) => {
                    return Err(Error::Invalid(format!(
                        "unknown template {t:?} (available: real-estate)"
                    )))
                }
                (Some(s), src, None) => {
                    let schema: Schema = read_json_file(s)?;
                    let sources: Vec<Source> = match src {
                        Some(p) => read_json_file(p)?,
                        None => Vec::new(),
                    };
                    (schema, sources)
                }
                _ => {
                    return Err(Error::Invalid(
                        "use either --schema [--sources], or --template real-estate".into(),
                    ))
                }
            };
            let db = Db::init(&dir, schema, sources)?;
            println!("initialized nescioDB in {}", dir.display());
            println!(
                "  slots:   {}",
                db.schema
                    .slots
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!(
                "  sources: {}",
                db.sources.keys().cloned().collect::<Vec<_>>().join(", ")
            );
            Ok(())
        }
        Cmd::Status { dir } => {
            let db = Db::open(&dir)?;
            println!("nescioDB at {}", dir.display());
            println!("  evidence records: {}", db.evidence.len());
            println!("  entities:         {}", db.entities().count());
            for (slot, d) in &db.schema.slots {
                match d {
                    Domain::Continuous { lo, hi, n_bins } => {
                        println!("  slot {slot}: continuous [{lo}, {hi}], {n_bins} bins")
                    }
                    Domain::Categorical { values } => {
                        println!("  slot {slot}: categorical {values:?}")
                    }
                }
            }
            for c in &db.schema.couplings {
                println!("  coupling: {}", c.label());
            }
            for s in db.sources.values() {
                let hl = s
                    .half_life_days
                    .map_or("no decay".to_string(), |d| format!("half-life {d}d"));
                let ax = if s.axiomatic { ", axiomatic" } else { "" };
                println!("  source {}: r0={}, {hl}{ax}", s.name, s.reliability);
            }
            Ok(())
        }
        Cmd::Source {
            dir,
            name,
            reliability,
            half_life_days,
            axiomatic,
        } => {
            let mut db = Db::open(&dir)?;
            let n = db.put_source(Source {
                name: name.clone(),
                reliability,
                half_life_days,
                axiomatic,
            })?;
            println!("source {name:?} registered ({n} existing log entries re-interpreted)");
            Ok(())
        }
        Cmd::Ingest {
            dir,
            entity,
            slot,
            claim,
            source,
            at,
        } => {
            let mut db = Db::open(&dir)?;
            let claim = claim.into_claim(&slot)?;
            let observed_at = when_or_now(&at)?;
            db.ingest(EvidenceRecord {
                entity: entity.clone(),
                claim,
                source,
                observed_at,
            })?;
            println!(
                "ingested evidence for {entity}.{slot} at {}",
                format_unix(observed_at)
            );
            Ok(())
        }
        Cmd::Bound {
            dir,
            entity,
            slot,
            credible,
            at,
            json,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            let b = q.bound(&entity, &slot, credible)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&b).map_err(Error::Json)?);
            } else {
                println!("BOUND {entity}.{slot} as of {}", format_unix(q.as_of()));
                println!(
                    "  region ({:.0}%): {}",
                    credible * 100.0,
                    fmt_region(&b.region)
                );
                println!(
                    "  entropy: {:.2} of {:.2} bits (knowledge {:.0}%)",
                    b.entropy_bits,
                    b.max_entropy_bits,
                    b.knowledge_ratio() * 100.0
                );
                println!("  MAP estimate: {}", b.map_estimate);
            }
            Ok(())
        }
        Cmd::Sample {
            dir,
            entity,
            seed,
            at,
            json,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            let world = q.sample(&entity, seed)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&world).map_err(Error::Json)?
                );
            } else {
                println!(
                    "SAMPLE {entity} (seed {seed}) as of {}",
                    format_unix(q.as_of())
                );
                for (slot, v) in &world {
                    println!("  {slot} = {v}");
                }
            }
            Ok(())
        }
        Cmd::Certainly {
            dir,
            entity,
            slot,
            pred,
            at,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            println!("{}", q.certainly(&entity, &slot, &pred.into_predicate()?)?);
            Ok(())
        }
        Cmd::Find {
            dir,
            slot,
            lo,
            hi,
            mode,
            at,
            json,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            let found = q.find(&slot, lo, hi, mode.parse()?)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&found).map_err(Error::Json)?
                );
            } else if found.is_empty() {
                println!("(none)");
            } else {
                for e in found {
                    println!("{e}");
                }
            }
            Ok(())
        }
        Cmd::Join {
            dir,
            op,
            left,
            right,
            tol,
            left_prefix,
            right_prefix,
            min_prob,
            certain,
            all_entities,
            limit,
            at,
            json,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            let pred = match op.as_str() {
                "gt" => JoinPredicate::Gt { left, right },
                "lt" => JoinPredicate::Lt { left, right },
                "approx" => JoinPredicate::Approx {
                    left,
                    right,
                    tol: tol.ok_or_else(|| Error::Invalid("--op approx requires --tol".into()))?,
                },
                "same" => JoinPredicate::Same { left, right },
                other => {
                    return Err(Error::Invalid(format!(
                        "unknown --op {other:?} (gt|lt|approx|same)"
                    )))
                }
            };
            let opts = JoinOptions {
                left_prefix,
                right_prefix,
                min_probability: min_prob,
                certain_only: certain,
                require_evidence: !all_entities,
                limit,
            };
            let res = q.join(&pred, &opts)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&res).map_err(Error::Json)?
                );
            } else {
                if res.matches.is_empty() {
                    println!("(no matches)");
                }
                for m in &res.matches {
                    println!(
                        "  {:<24} {:<24} p={:.3}  {}",
                        m.left, m.right, m.probability, m.certainty
                    );
                }
                println!(
                    "  {} pairs examined{}",
                    res.pairs_examined,
                    if res.truncated { ", truncated" } else { "" }
                );
            }
            Ok(())
        }
        Cmd::Resolve {
            dir,
            entity,
            slot,
            target_bits,
            actions,
            max_steps,
            mc,
            seed,
            at,
            json,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            let actions: Vec<ProcurementAction> = read_json_file(&actions)?;
            let plan = q.resolve(&entity, &slot, target_bits, &actions, max_steps, mc, seed)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).map_err(Error::Json)?
                );
            } else {
                println!(
                    "RESOLVE {entity}.{slot}: {:.2} bits now, target {target_bits:.2}",
                    plan.start_entropy_bits
                );
                if plan.steps.is_empty() {
                    println!("  no action helps — the DB cannot know more this way");
                } else {
                    for (i, s) in plan.steps.iter().enumerate() {
                        println!(
                            "  {}. {} (slot {}, cost {}) -> expected {:.2} bits",
                            i + 1,
                            s.action.name,
                            s.action.slot,
                            s.action.cost,
                            s.expected_entropy_bits
                        );
                    }
                    println!(
                        "  total cost {} | greedy estimate {:.2} bits | MC-validated {}",
                        plan.total_cost,
                        plan.planned_entropy_bits,
                        plan.validated_entropy_bits
                            .map_or("-".to_string(), |v| format!("{v:.2} bits"))
                    );
                }
            }
            Ok(())
        }
        Cmd::Decide {
            dir,
            entity,
            slot,
            objective,
            target,
            actions,
            max_steps,
            mc,
            seed,
            at,
            json,
        } => {
            let db = Db::open(&dir)?;
            let q = Query::new(&db, when_or_now(&at)?);
            let objective: Objective = read_json_file(&objective)?;
            let actions: Vec<ProcurementAction> = read_json_file(&actions)?;
            let plan = q.resolve_decision(
                &entity, &slot, &objective, target, &actions, max_steps, mc, seed,
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).map_err(Error::Json)?
                );
            } else {
                println!(
                    "DECIDE {entity}.{slot} [{}]: risk {:.4} {} now, target {target:.4}",
                    plan.objective, plan.start_risk, plan.units
                );
                println!("  would decide now:   {}", plan.recommended_now);
                if plan.steps.is_empty() {
                    println!("  no action improves the decision — evidence would not change it");
                } else {
                    for (i, s) in plan.steps.iter().enumerate() {
                        println!(
                            "  {}. {} (slot {}, cost {}) -> expected risk {:.4} {}",
                            i + 1,
                            s.action.name,
                            s.action.slot,
                            s.action.cost,
                            s.expected_risk,
                            plan.units,
                        );
                    }
                    println!("  would decide after: {}", plan.recommended_after);
                    println!(
                        "  total cost {} | greedy {:.4} {} | MC-validated {}",
                        plan.total_cost,
                        plan.planned_risk,
                        plan.units,
                        plan.validated_risk
                            .map_or("-".to_string(), |v| format!("{v:.4} {}", plan.units)),
                    );
                }
            }
            Ok(())
        }
        Cmd::Prior { cmd } => match cmd {
            PriorCmd::Register {
                dir,
                name,
                slot,
                weights_file,
                gaussian,
            } => {
                let mut db = Db::open(&dir)?;
                let weights = match (weights_file, gaussian) {
                    (Some(f), None) => read_json_file(&f)?,
                    (None, Some(g)) => {
                        let (center, sigma) = parse_pair(&g, ',')?;
                        let domain = db.domain(&slot)?;
                        let Domain::Continuous { .. } = domain else {
                            return Err(Error::Invalid(
                                "--gaussian needs a continuous slot".into(),
                            ));
                        };
                        (0..domain.n())
                            .map(|i| {
                                let x = (domain.midpoint(i) - center) / sigma;
                                (-x * x).exp()
                            })
                            .collect()
                    }
                    _ => {
                        return Err(Error::Invalid(
                            "use exactly one of --weights-file / --gaussian".into(),
                        ))
                    }
                };
                db.register_prior(&name, &slot, weights)?;
                println!("prior {name:?} registered for slot {slot:?}");
                Ok(())
            }
            PriorCmd::Use {
                dir,
                entity,
                slot,
                name,
            } => {
                let mut db = Db::open(&dir)?;
                db.use_prior(&entity, &slot, &name)?;
                println!("entity {entity:?} now shares prior {name:?} on {slot:?}");
                Ok(())
            }
        },
        Cmd::ForgetSource { dir, source } => {
            let mut db = Db::open(&dir)?;
            let n = db.forget_source(&source)?;
            println!("physically erased {n} evidence records from source {source:?}; all derived regions widen");
            Ok(())
        }
        Cmd::Recalibrate {
            dir,
            source,
            apply,
            min_truth_reliability,
        } => {
            let mut db = Db::open(&dir)?;
            let fit = db.recalibrate_source(&source, min_truth_reliability)?;
            println!(
                "source {:?}: learned r0={:.2}, half-life {} from {} ground-truth pairs (log-likelihood {:.1})",
                fit.source_name,
                fit.r0,
                fit.half_life_days.map_or("none (no decay)".to_string(), |d| format!("{d} days")),
                fit.n_observations,
                fit.log_likelihood,
            );
            if apply {
                let axiomatic = db.sources.get(&source).is_some_and(|s| s.axiomatic);
                let n = db.put_source(Source {
                    name: source.clone(),
                    reliability: fit.r0,
                    half_life_days: fit.half_life_days,
                    axiomatic,
                })?;
                println!("applied: {n} log entries now erode under the learned physics");
            }
            Ok(())
        }
        Cmd::Import { dir, file } => {
            let mut db = Db::open(&dir)?;
            let data = std::fs::read_to_string(&file)
                .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", file.display())))?;
            let mut recs = Vec::new();
            for (i, line) in data.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let rec: EvidenceRecord = serde_json::from_str(line).map_err(|e| {
                    Error::Invalid(format!("{} line {}: {e}", file.display(), i + 1))
                })?;
                recs.push(rec);
            }
            let started = std::time::Instant::now();
            let n = db.ingest_batch(recs)?;
            let secs = started.elapsed().as_secs_f64();
            println!(
                "imported {n} records in {secs:.2}s ({:.0} records/s, one fsync)",
                n as f64 / secs.max(1e-9)
            );
            Ok(())
        }
        Cmd::Export { dir, file } => {
            let db = Db::open(&dir)?;
            let jsonl = db.export_jsonl()?;
            match file {
                Some(path) => {
                    std::fs::write(&path, &jsonl)?;
                    println!(
                        "exported {} records to {}",
                        db.evidence.len(),
                        path.display()
                    );
                }
                None => print!("{jsonl}"),
            }
            Ok(())
        }
        Cmd::Serve { dir, port, bind } => {
            let db = Db::open(&dir)?;
            let server = nescio::server::NescioServer::bind(&format!("{bind}:{port}"))?;
            println!(
                "nescioDB serving {} on http://{bind}:{} (parallel reads, exclusive writes)",
                dir.display(),
                server.port()
            );
            server.run(db)
        }
    }
}

impl ClaimArgs {
    fn into_claim(self, slot: &str) -> Result<Claim> {
        if let Some(iv) = self.interval {
            let (lo, hi) = parse_pair(&iv, '.')?;
            return Ok(Claim::Interval {
                slot: slot.to_string(),
                lo,
                hi,
            });
        }
        if let Some(v) = self.value {
            return Ok(Claim::Value {
                slot: slot.to_string(),
                value: v,
            });
        }
        if let Some(v) = self.not_value {
            return Ok(Claim::NotValue {
                slot: slot.to_string(),
                value: v,
            });
        }
        Err(Error::Invalid(
            "one of --interval / --value / --not-value is required".into(),
        ))
    }
}

impl PredArgs {
    fn into_predicate(self) -> Result<Predicate> {
        if let Some(v) = self.gt {
            return Ok(Predicate::Gt { value: v });
        }
        if let Some(v) = self.lt {
            return Ok(Predicate::Lt { value: v });
        }
        if let Some(b) = self.between {
            let (lo, hi) = parse_pair(&b, '.')?;
            return Ok(Predicate::Between { lo, hi });
        }
        if let Some(v) = self.is {
            return Ok(Predicate::Is { value: v });
        }
        if let Some(v) = self.is_not {
            return Ok(Predicate::IsNot { value: v });
        }
        Err(Error::Invalid("a predicate flag is required".into()))
    }
}

/// Parse "A..B" (sep='.') or "A,B" (sep=',') into two floats.
fn parse_pair(s: &str, sep: char) -> Result<(f64, f64)> {
    let parts: Vec<&str> = if sep == '.' {
        s.splitn(2, "..").collect()
    } else {
        s.splitn(2, sep).collect()
    };
    if parts.len() != 2 {
        return Err(Error::Invalid(format!("cannot parse pair {s:?}")));
    }
    let a = parts[0]
        .trim()
        .parse::<f64>()
        .map_err(|_| Error::Invalid(format!("bad number in {s:?}")))?;
    let b = parts[1]
        .trim()
        .parse::<f64>()
        .map_err(|_| Error::Invalid(format!("bad number in {s:?}")))?;
    Ok((a, b))
}

fn when_or_now(at: &Option<String>) -> Result<i64> {
    match at {
        Some(s) => parse_when(s),
        None => Ok(now_unix()),
    }
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&data).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

fn fmt_region(r: &Region) -> String {
    match r {
        Region::Intervals(ivs) => ivs
            .iter()
            .map(|(a, b)| format!("[{a:.0}, {b:.0}]"))
            .collect::<Vec<_>>()
            .join(" u "),
        Region::Values(vs) => format!("{{{}}}", vs.join(", ")),
    }
}

/// Built-in demo schema: real-estate objects with coupled slots.
fn real_estate_template() -> (Schema, Vec<Source>) {
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
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    slots.insert(
        "year_built".to_string(),
        Domain::Continuous {
            lo: 1900.0,
            hi: 2026.0,
            n_bins: 126,
        },
    );
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
    let sources = vec![
        Source {
            name: "land_registry".into(),
            reliability: 1.0,
            half_life_days: None,
            axiomatic: true,
        },
        Source {
            name: "notary".into(),
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
    (Schema { slots, couplings }, sources)
}
