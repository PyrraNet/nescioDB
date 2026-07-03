//! Persistence: a database is a directory.
//!
//! ```text
//! mydb/
//!   schema.json    slots (domains) + couplings — set at init, evolvable
//!                  afterwards (add_slot, add_value, add/remove_coupling,
//!                  remove_slot)
//!   sources.json   source registry: reliability, half-life, axiomatic
//!   priors.json    shared priors: registry + per-entity assignments
//!   log.bin        append-only evidence log, compact binary (see binlog)
//! ```
//!
//! The log is the only growing file and is only ever appended to — except
//! by `forget_source`, which *physically rewrites* it without the erased
//! source's records. GDPR erasure must be physical, and because every
//! region is recomputed from surviving factors, there is no aggregate that
//! could remember the deleted data.
//!
//! The binary format ([`crate::binlog`]) is the production on-disk truth;
//! `export_jsonl` reconstructs the human-readable form for debugging, and a
//! database that still has only a legacy `log.jsonl` is migrated on open.
//!
//! Sources live in one place and are referenced by name from the log, so
//! re-calibrating a source's decay physics corrects the interpretation of
//! its entire history without touching the log.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::binlog::{self, LOG_BIN};
use crate::calibrate::{calibration_pairs, fit_decay, FittedDecay};
use crate::error::{Error, Result};
use crate::model::coupling::Coupling;
use crate::model::domain::Domain;
use crate::model::evidence::{Evidence, EvidenceRecord, Source};

pub const SCHEMA_FILE: &str = "schema.json";
pub const SOURCES_FILE: &str = "sources.json";
pub const PRIORS_FILE: &str = "priors.json";
/// Legacy human-readable log. New databases use [`LOG_BIN`]; a database
/// that still has only this is migrated on open.
pub const LOG_FILE: &str = "log.jsonl";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Schema {
    pub slots: BTreeMap<String, Domain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub couplings: Vec<Coupling>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriorDef {
    pub slot: String,
    pub weights: Vec<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Priors {
    /// Stored once, referenced by any number of entities (factorized
    /// shared assumptions).
    #[serde(default)]
    pub registry: BTreeMap<String, PriorDef>,
    /// entity -> slot -> prior name
    #[serde(default)]
    pub assignments: BTreeMap<String, BTreeMap<String, String>>,
    /// slot -> weights, applied to every entity without an assignment
    #[serde(default)]
    pub defaults: BTreeMap<String, Vec<f64>>,
}

/// What [`Db::remove_slot`] took with it.
#[derive(Clone, Copy, Debug)]
pub struct SlotRemoval {
    /// Evidence records physically erased (the log was rewritten).
    pub evidence_erased: usize,
    /// Priors (registry entries and slot defaults) removed with the slot.
    pub priors_removed: usize,
}

pub struct Db {
    dir: Option<PathBuf>,
    pub schema: Schema,
    pub sources: BTreeMap<String, Source>,
    pub priors: Priors,
    pub evidence: Vec<Evidence>,
    pub(crate) index: HashMap<(String, String), Vec<usize>>,
    pub(crate) entities: BTreeSet<String>,
    /// Compiled factor tables, one per coupling (rows over slot_a).
    pub(crate) tables: Vec<Vec<Vec<f64>>>,
    /// slot -> [(neighbor slot, coupling index)]
    pub(crate) adjacency: BTreeMap<String, Vec<(String, usize)>>,
    pub(crate) loopy: bool,
}

impl Db {
    // ------------------------------------------------------------- lifecycle

    pub fn in_memory(schema: Schema, sources: Vec<Source>) -> Result<Self> {
        Self::build(None, schema, sources, Priors::default(), Vec::new())
    }

    pub fn init(dir: &Path, schema: Schema, sources: Vec<Source>) -> Result<Self> {
        if dir.join(SCHEMA_FILE).exists() {
            return Err(Error::Invalid(format!(
                "{} already contains a nescioDB (schema.json exists)",
                dir.display()
            )));
        }
        fs::create_dir_all(dir)?;
        let db = Self::build(
            Some(dir.to_path_buf()),
            schema,
            sources,
            Priors::default(),
            Vec::new(),
        )?;
        db.persist_schema()?;
        db.persist_sources()?;
        db.persist_priors()?;
        fs::write(dir.join(LOG_BIN), binlog::header())?;
        Ok(db)
    }

    pub fn open(dir: &Path) -> Result<Self> {
        let schema: Schema = read_json(&dir.join(SCHEMA_FILE))?;
        let sources: BTreeMap<String, Source> = read_json(&dir.join(SOURCES_FILE))?;
        let priors: Priors = if dir.join(PRIORS_FILE).exists() {
            read_json(&dir.join(PRIORS_FILE))?
        } else {
            Priors::default()
        };
        let bin_path = dir.join(LOG_BIN);
        let jsonl_path = dir.join(LOG_FILE);
        let (records, migrate_legacy) = if bin_path.exists() {
            let bytes = fs::read(&bin_path)?;
            let (recs, trailing) = binlog::decode(&bytes)?;
            if trailing > 0 {
                eprintln!(
                    "nescioDB: recovered {} evidence records; ignored a {}-byte torn tail in log.bin",
                    recs.len(),
                    trailing
                );
            }
            (recs, false)
        } else if jsonl_path.exists() {
            // Legacy database: read the JSONL and migrate it to binary below.
            (read_jsonl(&jsonl_path)?, true)
        } else {
            (Vec::new(), false)
        };
        let db = Self::build(
            Some(dir.to_path_buf()),
            schema,
            sources.into_values().collect(),
            priors,
            records,
        )?;
        if migrate_legacy {
            db.rewrite_log()?; // writes log.bin from the resolved evidence
            let _ = fs::rename(&jsonl_path, dir.join("log.jsonl.migrated"));
        }
        Ok(db)
    }

    fn build(
        dir: Option<PathBuf>,
        schema: Schema,
        sources: Vec<Source>,
        priors: Priors,
        records: Vec<EvidenceRecord>,
    ) -> Result<Self> {
        for (slot, d) in &schema.slots {
            d.validate(slot)?;
        }
        let mut source_map = BTreeMap::new();
        for s in sources {
            s.validate()?;
            source_map.insert(s.name.clone(), s);
        }
        let (tables, adjacency, loopy) = compile_couplings(&schema)?;
        let mut db = Db {
            dir,
            schema,
            sources: source_map,
            priors: Priors::default(),
            evidence: Vec::new(),
            index: HashMap::new(),
            entities: BTreeSet::new(),
            tables,
            adjacency,
            loopy,
        };
        db.set_priors_checked(priors)?;
        for rec in records {
            let ev = db.resolve_record(rec)?;
            db.push_evidence(ev);
        }
        Ok(db)
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    // ------------------------------------------------------------- sources

    /// Insert or update a source. Updating re-resolves the in-memory log,
    /// so corrected decay physics applies to the source's entire history.
    pub fn put_source(&mut self, source: Source) -> Result<usize> {
        source.validate()?;
        let name = source.name.clone();
        self.sources.insert(name.clone(), source.clone());
        let mut n = 0;
        for ev in &mut self.evidence {
            if ev.source.name == name && ev.source != source {
                ev.source = source.clone();
                n += 1;
            }
        }
        self.persist_sources()?;
        Ok(n)
    }

    // ------------------------------------------------------------- evidence

    pub fn ingest(&mut self, rec: EvidenceRecord) -> Result<()> {
        self.ingest_batch(vec![rec]).map(|_| ())
    }

    /// Append many records with one write and one fsync (group commit).
    /// All records are validated before anything touches the log — a batch
    /// either lands completely or not at all.
    pub fn ingest_batch(&mut self, recs: Vec<EvidenceRecord>) -> Result<usize> {
        let mut resolved = Vec::with_capacity(recs.len());
        for rec in &recs {
            resolved.push(self.resolve_record(rec.clone())?);
        }
        if let Some(dir) = &self.dir {
            let mut buf = Vec::with_capacity(recs.len() * 48);
            let path = dir.join(LOG_BIN);
            if !path.exists() {
                buf.extend_from_slice(&binlog::header());
            }
            for rec in &recs {
                binlog::encode(rec, &mut buf);
            }
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            f.write_all(&buf)?;
            // Durability, not just delivery to the page cache.
            f.sync_data()?;
        }
        let n = resolved.len();
        for ev in resolved {
            self.push_evidence(ev);
        }
        Ok(n)
    }

    /// GDPR erasure: physically remove all evidence from a source and
    /// rewrite the log. Every derived region widens automatically.
    pub fn forget_source(&mut self, source_name: &str) -> Result<usize> {
        let before = self.evidence.len();
        self.evidence.retain(|e| e.source.name != source_name);
        let removed = before - self.evidence.len();
        self.rebuild_index();
        if removed > 0 {
            self.rewrite_log()?;
        }
        Ok(removed)
    }

    // --------------------------------------------------------------- priors

    /// A shared prior: stored once, referenced by any number of entities.
    pub fn register_prior(&mut self, name: &str, slot: &str, weights: Vec<f64>) -> Result<()> {
        let domain = self.domain(slot)?;
        check_prior(slot, &weights, domain)?;
        self.priors.registry.insert(
            name.to_string(),
            PriorDef {
                slot: slot.to_string(),
                weights,
            },
        );
        self.persist_priors()
    }

    pub fn use_prior(&mut self, entity: &str, slot: &str, name: &str) -> Result<()> {
        let def = self
            .priors
            .registry
            .get(name)
            .ok_or_else(|| Error::Invalid(format!("unknown prior {name:?}")))?;
        if def.slot != slot {
            return Err(Error::Invalid(format!(
                "prior {name:?} is for slot {:?}, not {slot:?}",
                def.slot
            )));
        }
        self.priors
            .assignments
            .entry(entity.to_string())
            .or_default()
            .insert(slot.to_string(), name.to_string());
        self.entities.insert(entity.to_string());
        self.persist_priors()
    }

    pub fn set_default_prior(&mut self, slot: &str, weights: Vec<f64>) -> Result<()> {
        let domain = self.domain(slot)?;
        check_prior(slot, &weights, domain)?;
        self.priors.defaults.insert(slot.to_string(), weights);
        self.persist_priors()
    }

    pub(crate) fn prior_for(&self, entity: &str, slot: &str) -> Option<&[f64]> {
        if let Some(name) = self
            .priors
            .assignments
            .get(entity)
            .and_then(|m| m.get(slot))
        {
            return self.priors.registry.get(name).map(|d| d.weights.as_slice());
        }
        self.priors.defaults.get(slot).map(|w| w.as_slice())
    }

    // ----------------------------------------------------- schema evolution

    /// Add a slot to a live database. No backfill is needed: ignorance is
    /// the default state, so every existing entity simply starts at maximal
    /// entropy on the new slot.
    pub fn add_slot(&mut self, name: &str, domain: Domain) -> Result<()> {
        if self.schema.slots.contains_key(name) {
            return Err(Error::Invalid(format!("slot {name:?} already exists")));
        }
        domain.validate(name)?;
        self.schema.slots.insert(name.to_string(), domain);
        self.persist_schema()
    }

    /// Remove a slot together with everything that only has meaning through
    /// it: its evidence (physically, with a log rewrite — a record on an
    /// unknown slot would fail replay) and its priors. Couplings are
    /// cross-slot and are never removed implicitly: drop them first.
    pub fn remove_slot(&mut self, name: &str) -> Result<SlotRemoval> {
        self.domain(name)?;
        if let Some(c) = self
            .schema
            .couplings
            .iter()
            .find(|c| c.slot_a == name || c.slot_b == name)
        {
            return Err(Error::Invalid(format!(
                "slot {name:?} is referenced by coupling {}; remove the coupling first",
                c.label()
            )));
        }
        let before = self.evidence.len();
        self.evidence.retain(|e| e.claim.slot() != name);
        let evidence_erased = before - self.evidence.len();
        let mut priors_removed = 0;
        let orphaned: Vec<String> = self
            .priors
            .registry
            .iter()
            .filter(|(_, d)| d.slot == name)
            .map(|(n, _)| n.clone())
            .collect();
        for n in &orphaned {
            self.priors.registry.remove(n);
            priors_removed += 1;
        }
        if self.priors.defaults.remove(name).is_some() {
            priors_removed += 1;
        }
        for slots in self.priors.assignments.values_mut() {
            slots.remove(name);
        }
        self.priors.assignments.retain(|_, m| !m.is_empty());
        self.schema.slots.remove(name);
        self.rebuild_index();
        if evidence_erased > 0 {
            self.rewrite_log()?;
        }
        self.persist_schema()?;
        self.persist_priors()?;
        Ok(SlotRemoval {
            evidence_erased,
            priors_removed,
        })
    }

    /// Extend a categorical slot with a new value. History stays valid —
    /// the log stores values as strings, not indices. Coupling tables are
    /// recompiled (a category without an entry is uninformative); priors on
    /// the slot are extended with the mean of their existing weights, a
    /// neutral choice. Returns how many priors were extended.
    pub fn add_value(&mut self, slot: &str, value: &str) -> Result<usize> {
        let mut schema = self.schema.clone();
        let domain = schema
            .slots
            .get_mut(slot)
            .ok_or_else(|| Error::Invalid(format!("unknown slot {slot:?}")))?;
        let Domain::Categorical { values } = domain else {
            return Err(Error::Invalid(format!(
                "slot {slot:?} is continuous; add_value needs a categorical slot"
            )));
        };
        if values.iter().any(|v| v == value) {
            return Err(Error::Invalid(format!(
                "slot {slot:?} already has value {value:?}"
            )));
        }
        values.push(value.to_string());
        // Validate before committing: an explicit Table coupling has fixed
        // dimensions and rejects the grown domain here.
        let (tables, adjacency, loopy) = compile_couplings(&schema)?;
        self.schema = schema;
        self.tables = tables;
        self.adjacency = adjacency;
        self.loopy = loopy;
        let mut extended = 0;
        for def in self.priors.registry.values_mut() {
            if def.slot == slot {
                let mean = def.weights.iter().sum::<f64>() / def.weights.len() as f64;
                def.weights.push(mean);
                extended += 1;
            }
        }
        if let Some(w) = self.priors.defaults.get_mut(slot) {
            let mean = w.iter().sum::<f64>() / w.len() as f64;
            w.push(mean);
            extended += 1;
        }
        self.persist_schema()?;
        if extended > 0 {
            self.persist_priors()?;
        }
        Ok(extended)
    }

    /// Add a coupling to a live database. Schema-level: it applies to every
    /// entity immediately. Like a contradicting axiom, a coupling that is
    /// incompatible with existing evidence surfaces as a conflict at query
    /// time, not here — contradiction is a state, not an ingest error.
    pub fn add_coupling(&mut self, c: Coupling) -> Result<()> {
        if self.schema.couplings.iter().any(|x| x.label() == c.label()) {
            return Err(Error::Invalid(format!(
                "a coupling labelled {} already exists",
                c.label()
            )));
        }
        let mut schema = self.schema.clone();
        schema.couplings.push(c);
        let (tables, adjacency, loopy) = compile_couplings(&schema)?;
        self.schema = schema;
        self.tables = tables;
        self.adjacency = adjacency;
        self.loopy = loopy;
        self.persist_schema()
    }

    /// Remove a coupling by its label (`nescio status` lists them). The
    /// affected slots decouple; their evidence is untouched.
    pub fn remove_coupling(&mut self, label: &str) -> Result<()> {
        let idx = self
            .schema
            .couplings
            .iter()
            .position(|c| c.label() == label)
            .ok_or_else(|| Error::Invalid(format!("no coupling labelled {label:?}")))?;
        let mut schema = self.schema.clone();
        schema.couplings.remove(idx);
        let (tables, adjacency, loopy) = compile_couplings(&schema)?;
        self.schema = schema;
        self.tables = tables;
        self.adjacency = adjacency;
        self.loopy = loopy;
        self.persist_schema()
    }

    // ---------------------------------------------------------- calibration

    /// Learn a source's decay physics from ground truth in the log.
    pub fn recalibrate_source(
        &self,
        source_name: &str,
        min_truth_reliability: f64,
    ) -> Result<FittedDecay> {
        let pairs = calibration_pairs(&self.evidence, source_name, min_truth_reliability);
        fit_decay(source_name, &pairs)
    }

    // -------------------------------------------------------------- lookups

    pub fn domain(&self, slot: &str) -> Result<&Domain> {
        self.schema
            .slots
            .get(slot)
            .ok_or_else(|| Error::Invalid(format!("unknown slot {slot:?}")))
    }

    pub fn entities(&self) -> impl Iterator<Item = &str> {
        self.entities.iter().map(|s| s.as_str())
    }

    pub(crate) fn evidence_for(&self, entity: &str, slot: &str) -> &[usize] {
        self.index
            .get(&(entity.to_string(), slot.to_string()))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    // ------------------------------------------------------------ internals

    fn resolve_record(&self, rec: EvidenceRecord) -> Result<Evidence> {
        let domain = self.domain(rec.claim.slot())?;
        rec.claim.validate(domain)?;
        let source = self
            .sources
            .get(&rec.source)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "unknown source {:?} (register it first)",
                    rec.source
                ))
            })?
            .clone();
        Ok(Evidence {
            entity: rec.entity,
            claim: rec.claim,
            source,
            observed_at: rec.observed_at,
        })
    }

    fn push_evidence(&mut self, ev: Evidence) {
        let key = (ev.entity.clone(), ev.claim.slot().to_string());
        self.index.entry(key).or_default().push(self.evidence.len());
        self.entities.insert(ev.entity.clone());
        self.evidence.push(ev);
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        self.entities.clear();
        for name in self.priors.assignments.keys() {
            self.entities.insert(name.clone());
        }
        let evidence = std::mem::take(&mut self.evidence);
        for ev in evidence {
            self.push_evidence(ev);
        }
    }

    fn set_priors_checked(&mut self, priors: Priors) -> Result<()> {
        for (name, def) in &priors.registry {
            let domain = self.domain(&def.slot).map_err(|_| {
                Error::Invalid(format!(
                    "prior {name:?} references unknown slot {:?}",
                    def.slot
                ))
            })?;
            check_prior(&def.slot, &def.weights, domain)?;
        }
        for (slot, w) in &priors.defaults {
            check_prior(slot, w, self.domain(slot)?)?;
        }
        for entity in priors.assignments.keys() {
            self.entities.insert(entity.clone());
        }
        self.priors = priors;
        Ok(())
    }

    fn rewrite_log(&self) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let mut out = binlog::header();
        for ev in &self.evidence {
            binlog::encode(&self.record_of(ev), &mut out);
        }
        atomic_write(&dir.join(LOG_BIN), &out)
    }

    fn record_of(&self, ev: &Evidence) -> EvidenceRecord {
        EvidenceRecord {
            entity: ev.entity.clone(),
            claim: ev.claim.clone(),
            source: ev.source.name.clone(),
            observed_at: ev.observed_at,
        }
    }

    /// Reconstruct the human-readable JSONL from the binary log — for
    /// debugging, diffing, or piping into another tool. One record per line.
    pub fn export_jsonl(&self) -> Result<String> {
        let mut out = String::with_capacity(self.evidence.len() * 128);
        for ev in &self.evidence {
            out.push_str(&serde_json::to_string(&self.record_of(ev))?);
            out.push('\n');
        }
        Ok(out)
    }

    fn persist_schema(&self) -> Result<()> {
        self.persist(SCHEMA_FILE, &self.schema)
    }

    fn persist_sources(&self) -> Result<()> {
        self.persist(SOURCES_FILE, &self.sources)
    }

    fn persist_priors(&self) -> Result<()> {
        self.persist(PRIORS_FILE, &self.priors)
    }

    fn persist<T: Serialize>(&self, file: &str, value: &T) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let json = serde_json::to_string_pretty(value)?;
        atomic_write(&dir.join(file), json.as_bytes())
    }
}

/// Compile coupling tables and the adjacency map; detect cycles. Pure in
/// the schema, so schema evolution can validate a candidate before
/// committing anything.
#[allow(clippy::type_complexity)]
fn compile_couplings(
    schema: &Schema,
) -> Result<(
    Vec<Vec<Vec<f64>>>,
    BTreeMap<String, Vec<(String, usize)>>,
    bool,
)> {
    let mut tables = Vec::new();
    let mut adjacency: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for (ci, c) in schema.couplings.iter().enumerate() {
        let da = schema.slots.get(&c.slot_a).ok_or_else(|| {
            Error::Invalid(format!(
                "coupling {} references unknown slot {:?}",
                c.label(),
                c.slot_a
            ))
        })?;
        let db_ = schema.slots.get(&c.slot_b).ok_or_else(|| {
            Error::Invalid(format!(
                "coupling {} references unknown slot {:?}",
                c.label(),
                c.slot_b
            ))
        })?;
        tables.push(c.build_table(da, db_)?);
        adjacency
            .entry(c.slot_a.clone())
            .or_default()
            .push((c.slot_b.clone(), ci));
        adjacency
            .entry(c.slot_b.clone())
            .or_default()
            .push((c.slot_a.clone(), ci));
    }
    Ok((tables, adjacency, has_cycle(schema)))
}

fn check_prior(slot: &str, weights: &[f64], domain: &Domain) -> Result<()> {
    if weights.len() != domain.n() {
        return Err(Error::Invalid(format!(
            "prior for slot {slot:?} needs {} weights, got {}",
            domain.n(),
            weights.len()
        )));
    }
    if weights.iter().any(|w| !w.is_finite() || *w < 0.0) {
        return Err(Error::Invalid(format!(
            "prior for slot {slot:?}: weights must be finite and >= 0"
        )));
    }
    if weights.iter().sum::<f64>() <= 0.0 {
        return Err(Error::Invalid(format!(
            "prior for slot {slot:?}: all-zero weights"
        )));
    }
    Ok(())
}

fn has_cycle(schema: &Schema) -> bool {
    let mut parent: BTreeMap<&str, &str> = schema
        .slots
        .keys()
        .map(|k| (k.as_str(), k.as_str()))
        .collect();
    fn find<'a>(parent: &mut BTreeMap<&'a str, &'a str>, mut x: &'a str) -> &'a str {
        while parent[x] != x {
            let up = parent[parent[x]];
            parent.insert(x, up);
            x = up;
        }
        x
    }
    for c in &schema.couplings {
        let (ra, rb) = (
            find(&mut parent, c.slot_a.as_str()),
            find(&mut parent, c.slot_b.as_str()),
        );
        if ra == rb {
            return true;
        }
        parent.insert(ra, rb);
    }
    false
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let data = fs::read_to_string(path)
        .map_err(|e| Error::Invalid(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&data).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

/// Read a legacy log.jsonl (one EvidenceRecord per line) for migration.
fn read_jsonl(path: &Path) -> Result<Vec<EvidenceRecord>> {
    let reader = BufReader::new(fs::File::open(path)?);
    let mut records = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: EvidenceRecord = serde_json::from_str(&line)
            .map_err(|e| Error::Invalid(format!("log.jsonl line {}: {e}", lineno + 1)))?;
        records.push(rec);
    }
    Ok(records)
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_data()?; // the rename must never expose a torn file
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
