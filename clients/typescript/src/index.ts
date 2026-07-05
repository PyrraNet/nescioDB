/**
 * nesciodb — TypeScript client for the nescioDB HTTP API.
 *
 * nescioDB is a database whose primary object is ignorance: fields hold
 * regions with entropy, not values. Start a server with `nescio serve`,
 * then talk to it from any language. This client wraps every verb with
 * typed methods and idiomatic camelCase.
 *
 * Zero runtime dependencies — uses the global `fetch` (Node 18+ / browsers).
 */

/** A point in time: an ISO date/datetime, unix seconds, or a Date. Defaults to now. */
export type At = string | number | Date;

function atOut(at?: At): string | number | undefined {
  if (at === undefined) return undefined;
  return at instanceof Date ? Math.floor(at.getTime() / 1000) : at;
}

export type Tri = "true" | "possible" | "false";

/** The credible region returned by BOUND: numeric intervals or category labels. */
export type Region =
  | { kind: "intervals"; intervals: [number, number][] }
  | { kind: "values"; values: string[] };

export interface Bound {
  entity: string;
  slot: string;
  region: Region;
  entropyBits: number;
  maxEntropyBits: number;
  /** MAP estimate: a number for continuous slots, a label for categorical. */
  mapEstimate: number | string;
  /** 0 = knows nothing, 1 = fully collapsed. Computed from entropy. */
  knowledgeRatio: number;
}

export type Claim =
  | { type: "interval"; slot: string; lo: number; hi: number }
  | { type: "value"; slot: string; value: string }
  | { type: "not_value"; slot: string; value: string };

/** Claim helpers. */
export const claim = {
  interval: (slot: string, lo: number, hi: number): Claim => ({ type: "interval", slot, lo, hi }),
  value: (slot: string, value: string): Claim => ({ type: "value", slot, value }),
  notValue: (slot: string, value: string): Claim => ({ type: "not_value", slot, value }),
};

/** Predicate for `certainly`. */
export type Predicate =
  | { op: "gt"; value: number }
  | { op: "lt"; value: number }
  | { op: "between"; lo: number; hi: number }
  | { op: "is"; value: string }
  | { op: "is_not"; value: string };

export type FindMode = "possible" | "certain";

export interface Source {
  name: string;
  reliability: number;
  halfLifeDays?: number;
  axiomatic?: boolean;
}

export interface ProcurementAction {
  name: string;
  slot: string;
  cost: number;
  source: Source;
  answerWidth?: number;
}

export interface ResolveStep {
  action: ProcurementAction;
  expectedEntropyBits: number;
  expectedGainBits: number;
}

export interface ResolvePlan {
  steps: ResolveStep[];
  startEntropyBits: number;
  plannedEntropyBits: number;
  /** Seeded Monte-Carlo estimate over full worlds — the number to trust. */
  validatedEntropyBits: number | null;
  totalCost: number;
}

/** The decision problem a DECIDE plan optimizes for. */
export type Objective =
  | { kind: "entropy" }
  | { kind: "squared_error" }
  | { kind: "absolute_error" }
  | { kind: "decision"; loss: number[][]; labels?: string[] };

export interface DecisionStep {
  action: ProcurementAction;
  expectedRisk: number;
  expectedGain: number;
}

/** Result of DECIDE: risk is in the objective's own units, not bits. */
export interface DecisionPlan {
  objective: string;
  units: string;
  steps: DecisionStep[];
  startRisk: number;
  plannedRisk: number;
  /** Seeded Monte-Carlo estimate over full worlds — the number to trust. */
  validatedRisk: number | null;
  totalCost: number;
  /** What the DB would decide right now. */
  recommendedNow: string;
  /** What it would decide after executing the plan. */
  recommendedAfter: string;
}

/** A slot's state space, as in schema.json. Build one with the `domain` helpers. */
export type Domain =
  | { type: "continuous"; lo: number; hi: number; n_bins: number }
  | { type: "categorical"; values: string[] };

/** Domain helpers. */
export const domain = {
  continuous: (lo: number, hi: number, nBins: number): Domain => ({
    type: "continuous",
    lo,
    hi,
    n_bins: nBins,
  }),
  categorical: (...values: string[]): Domain => ({ type: "categorical", values }),
  boolean: (): Domain => ({ type: "categorical", values: ["true", "false"] }),
};

/** Cross-slot correlation, as in schema.json. Build one with the `coupling` helpers. */
export interface Coupling {
  slot_a: string;
  slot_b: string;
  compat: Compat;
  name?: string;
}

export type Compat =
  | { kind: "gaussian_by_category"; centers: Record<string, number>; sigma: number }
  | {
      kind: "step_threshold";
      threshold: number;
      below: Record<string, number>;
      above: Record<string, number>;
    }
  | { kind: "matrix"; weights: Record<string, Record<string, number>>; default?: number }
  | { kind: "table"; rows: number[][] };

/**
 * Coupling helpers. Slot order matters — see the file-format reference:
 * gaussianByCategory wants (categorical, continuous), stepThreshold
 * (continuous, categorical), matrix (categorical, categorical).
 */
export const coupling = {
  gaussianByCategory: (
    slotA: string,
    slotB: string,
    centers: Record<string, number>,
    sigma: number,
    name?: string,
  ): Coupling => ({
    slot_a: slotA,
    slot_b: slotB,
    compat: { kind: "gaussian_by_category", centers, sigma },
    name,
  }),
  stepThreshold: (
    slotA: string,
    slotB: string,
    threshold: number,
    below: Record<string, number>,
    above: Record<string, number>,
    name?: string,
  ): Coupling => ({
    slot_a: slotA,
    slot_b: slotB,
    compat: { kind: "step_threshold", threshold, below, above },
    name,
  }),
  matrix: (
    slotA: string,
    slotB: string,
    weights: Record<string, Record<string, number>>,
    opts: { default?: number; name?: string } = {},
  ): Coupling => ({
    slot_a: slotA,
    slot_b: slotB,
    compat: { kind: "matrix", weights, default: opts.default },
    name: opts.name,
  }),
  table: (slotA: string, slotB: string, rows: number[][], name?: string): Coupling => ({
    slot_a: slotA,
    slot_b: slotB,
    compat: { kind: "table", rows },
    name,
  }),
};

export type JoinPredicate =
  | { op: "gt"; left: string; right: string }
  | { op: "lt"; left: string; right: string }
  | { op: "approx"; left: string; right: string; tol: number }
  | { op: "same"; left: string; right: string };

export interface JoinOptions {
  leftPrefix?: string;
  rightPrefix?: string;
  minProbability?: number;
  certainOnly?: boolean;
  requireEvidence?: boolean;
  limit?: number;
}

export interface JoinMatch {
  left: string;
  right: string;
  /** P(predicate holds) under the two entities' independent posteriors. */
  probability: number;
  /** Region-containment truth, consistent with `certainly`. */
  certainty: Tri;
}

export interface JoinResult {
  matches: JoinMatch[];
  pairsExamined: number;
  truncated: boolean;
}

export interface FittedDecay {
  sourceName: string;
  r0: number;
  halfLifeDays: number | null;
  logLikelihood: number;
  nObservations: number;
}

export interface Status {
  evidence: number;
  entities: number;
  slots: Record<string, unknown>;
  couplings: string[];
  sources: Source[];
}

/**
 * A standing question: fire when an entity's slot decays past a threshold.
 * Set exactly one of `maxEntropyBits` / `minKnowledge`.
 */
export interface Watch {
  name: string;
  entity: string;
  slot: string;
  /** Fire when entropy exceeds this many bits. */
  maxEntropyBits?: number;
  /** Fire when knowledge (1 - entropy/max entropy) drops below this ratio in (0, 1]. */
  minKnowledge?: number;
}

/** A watch evaluated at a point in time. */
export interface WatchState extends Watch {
  triggered: boolean;
  thresholdBits?: number;
  entropyBits?: number;
  knowledge?: number;
  /**
   * The knowledge horizon: when decay alone will trigger this watch (unix
   * seconds, day granularity). Absent when an axiomatic, non-decaying
   * source pins the slot forever.
   */
  horizon?: number;
  horizonDate?: string;
  /** Set when evaluation failed — e.g. an axiom conflict, which triggers the watch. */
  error?: string;
}

/** One message from the `/watches/events` stream. */
export type WatchEvent =
  | { event: "snapshot"; asOf: number; watches: WatchState[] }
  | { event: "triggered"; state: WatchState }
  | { event: "recovered"; state: WatchState };

/** Thrown for any non-2xx response; carries the HTTP status and server message. */
export class NescioError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.name = "NescioError";
  }
}

export interface ClientOptions {
  /** Per-request timeout in milliseconds (default 30000). */
  timeoutMs?: number;
  /** Custom fetch implementation (defaults to the global `fetch`). */
  fetch?: typeof fetch;
  /** Extra headers sent with every request (e.g. auth for a reverse proxy). */
  headers?: Record<string, string>;
}

export class NescioClient {
  private readonly base: string;
  private readonly timeoutMs: number;
  private readonly doFetch: typeof fetch;
  private readonly headers: Record<string, string>;

  constructor(baseUrl = "http://localhost:7777", opts: ClientOptions = {}) {
    this.base = baseUrl.replace(/\/$/, "");
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    this.headers = opts.headers ?? {};
    const f = opts.fetch ?? globalThis.fetch;
    if (!f) {
      throw new Error("no fetch available — pass one via ClientOptions.fetch (Node < 18)");
    }
    this.doFetch = f.bind(globalThis);
  }

  /** A handle with the entity id bound: `db.entity("villa_1").bound("price")`. */
  entity(id: string): EntityHandle {
    return new EntityHandle(this, id);
  }

  // ------------------------------------------------------------- introspection

  async health(): Promise<{ ok: boolean; version: string }> {
    return this.get("/health");
  }

  async status(): Promise<Status> {
    return this.get("/status");
  }

  // -------------------------------------------------------------------- verbs

  /** BOUND: credible region + entropy — how ignorant is the DB about this slot? */
  async bound(
    entity: string,
    slot: string,
    opts: { at?: At; credible?: number } = {},
  ): Promise<Bound> {
    const j = await this.get("/bound", {
      entity,
      slot,
      at: atOut(opts.at),
      credible: opts.credible,
    });
    return mapBound(j);
  }

  /** SAMPLE: one concrete, consistent world, deterministic under the seed. */
  async sample(
    entity: string,
    opts: { seed?: number; at?: At } = {},
  ): Promise<Record<string, number | string>> {
    return this.get("/sample", { entity, seed: opts.seed, at: atOut(opts.at) });
  }

  /** Three-valued predicate: `true` / `possible` / `false` (region containment). */
  async certainly(
    entity: string,
    slot: string,
    pred: Predicate,
    opts: { at?: At } = {},
  ): Promise<Tri> {
    const params: Record<string, unknown> = { entity, slot, op: pred.op, at: atOut(opts.at) };
    if ("value" in pred) params.value = pred.value;
    if ("lo" in pred) params.lo = pred.lo;
    if ("hi" in pred) params.hi = pred.hi;
    const j = await this.get("/certainly", params);
    return j.result as Tri;
  }

  /** FIND: entities whose region certainly lies in / possibly intersects [lo, hi]. */
  async find(
    slot: string,
    lo: number,
    hi: number,
    opts: { mode?: FindMode; at?: At } = {},
  ): Promise<string[]> {
    return this.get("/find", { slot, lo, hi, mode: opts.mode, at: atOut(opts.at) });
  }

  /** JOIN: entity pairs matching a relation, each with a probability and certainty. */
  async join(
    predicate: JoinPredicate,
    opts: JoinOptions & { at?: At } = {},
  ): Promise<JoinResult> {
    const j = await this.post("/join", {
      predicate,
      options: {
        left_prefix: opts.leftPrefix,
        right_prefix: opts.rightPrefix,
        min_probability: opts.minProbability,
        certain_only: opts.certainOnly,
        require_evidence: opts.requireEvidence,
        limit: opts.limit,
      },
      at: atOut(opts.at),
    });
    return {
      matches: j.matches,
      pairsExamined: j.pairs_examined,
      truncated: j.truncated,
    };
  }

  /** RESOLVE: plan the minimal-cost evidence to push a slot's entropy under a target. */
  async resolve(req: {
    entity: string;
    slot: string;
    targetBits: number;
    actions: ProcurementAction[];
    maxSteps?: number;
    mc?: number;
    seed?: number;
    at?: At;
  }): Promise<ResolvePlan> {
    const j = await this.post("/resolve", {
      entity: req.entity,
      slot: req.slot,
      target_bits: req.targetBits,
      actions: req.actions.map(actionOut),
      max_steps: req.maxSteps,
      mc: req.mc,
      seed: req.seed,
      at: atOut(req.at),
    });
    return mapPlan(j);
  }

  /**
   * DECIDE: plan the evidence that most improves a *decision*, not just
   * entropy — the Value of Information for the call you actually face.
   */
  async decide(req: {
    entity: string;
    slot: string;
    objective: Objective;
    /** Stop once the Bayes risk (in the objective's units) reaches this. */
    target: number;
    actions: ProcurementAction[];
    maxSteps?: number;
    mc?: number;
    seed?: number;
    at?: At;
  }): Promise<DecisionPlan> {
    const j = await this.post("/decide", {
      entity: req.entity,
      slot: req.slot,
      objective: req.objective,
      target: req.target,
      actions: req.actions.map(actionOut),
      max_steps: req.maxSteps,
      mc: req.mc,
      seed: req.seed,
      at: atOut(req.at),
    });
    return mapDecision(j);
  }

  // ------------------------------------------------------------------- writes

  /** Append one evidence record to the log. */
  async ingest(
    entity: string,
    c: Claim,
    source: string,
    opts: { at?: At } = {},
  ): Promise<{ ok: boolean; observedAt: number }> {
    const j = await this.post("/ingest", { entity, claim: c, source, at: atOut(opts.at) });
    return { ok: j.ok, observedAt: j.observed_at };
  }

  /** Append many records with a single group commit (one fsync). */
  async ingestBatch(
    records: { entity: string; claim: Claim; source: string; at?: At }[],
  ): Promise<{ ok: boolean; ingested: number }> {
    return this.post(
      "/ingest-batch",
      records.map((r) => ({ ...r, at: atOut(r.at) })),
    );
  }

  /** Register or update a source (updating re-interprets its whole history). */
  async putSource(source: Source): Promise<{ ok: boolean; reinterpreted: number }> {
    const j = await this.post("/sources", sourceOut(source));
    return { ok: j.ok, reinterpreted: j.reinterpreted };
  }

  /** GDPR erasure: physically remove all evidence from a source. */
  async forgetSource(source: string): Promise<{ ok: boolean; erased: number }> {
    return this.post("/forget-source", { source });
  }

  /** Learn a source's decay physics from ground truth in the log. */
  async recalibrate(
    source: string,
    opts: { apply?: boolean; minTruthReliability?: number } = {},
  ): Promise<{ fit: FittedDecay; applied: number }> {
    const j = await this.post("/recalibrate", {
      source,
      apply: opts.apply,
      min_truth_reliability: opts.minTruthReliability,
    });
    return { fit: mapFit(j.fit), applied: j.applied };
  }

  async registerPrior(name: string, slot: string, weights: number[]): Promise<{ ok: boolean }> {
    return this.post("/priors/register", { name, slot, weights });
  }

  async usePrior(entity: string, slot: string, name: string): Promise<{ ok: boolean }> {
    return this.post("/priors/use", { entity, slot, name });
  }

  // ---------------------------------------------------------- schema evolution

  /** Add a slot to the live database; every entity starts at maximal entropy on it. */
  async addSlot(name: string, d: Domain): Promise<{ ok: boolean }> {
    return this.post("/schema/add-slot", { name, domain: d });
  }

  /**
   * Remove a slot: physically erases its evidence and priors. Refused (400)
   * while a coupling references the slot — remove the coupling first.
   */
  async removeSlot(
    name: string,
  ): Promise<{ ok: boolean; evidenceErased: number; priorsRemoved: number }> {
    const j = await this.post("/schema/remove-slot", { name });
    return { ok: j.ok, evidenceErased: j.evidence_erased, priorsRemoved: j.priors_removed };
  }

  /** Extend a categorical slot with a new value; history stays valid. */
  async addValue(slot: string, value: string): Promise<{ ok: boolean; priorsExtended: number }> {
    const j = await this.post("/schema/add-value", { slot, value });
    return { ok: j.ok, priorsExtended: j.priors_extended };
  }

  /** Add a coupling — it applies to every entity immediately. */
  async addCoupling(c: Coupling): Promise<{ ok: boolean }> {
    return this.post("/schema/add-coupling", c);
  }

  /** Remove a coupling by its label (defaults to "slot_a~slot_b"). */
  async removeCoupling(name: string): Promise<{ ok: boolean }> {
    return this.post("/schema/remove-coupling", { name });
  }

  // ------------------------------------------------------------------ watches

  /** Every watch with its current state and knowledge horizon. */
  async watches(opts: { at?: At } = {}): Promise<{ asOf: number; watches: WatchState[] }> {
    const j = await this.get("/watches", { at: atOut(opts.at) });
    return { asOf: j.as_of, watches: (j.watches as any[]).map(mapWatchState) };
  }

  /**
   * Register a standing question. Returns the initial state — including
   * the knowledge horizon: the date decay alone will fire it.
   */
  async addWatch(w: Watch): Promise<WatchState> {
    const j = await this.post("/watches", {
      name: w.name,
      entity: w.entity,
      slot: w.slot,
      max_entropy_bits: w.maxEntropyBits,
      min_knowledge: w.minKnowledge,
    });
    return mapWatchState(j.state);
  }

  async removeWatch(name: string): Promise<{ ok: boolean }> {
    return this.post("/watches/remove", { name });
  }

  /** Evaluate all watches; returns only the triggered ones. */
  async checkWatches(
    opts: { at?: At } = {},
  ): Promise<{ asOf: number; checked: number; triggered: WatchState[] }> {
    const j = await this.get("/watches/check", { at: atOut(opts.at) });
    return {
      asOf: j.as_of,
      checked: j.checked,
      triggered: (j.triggered as any[]).map(mapWatchState),
    };
  }

  /**
   * Subscribe to watch transitions (Server-Sent Events). Yields a
   * `snapshot` first, then `triggered` / `recovered` states as they
   * happen. Runs until the server goes away or `opts.signal` aborts.
   *
   * ```ts
   * for await (const ev of db.watchEvents()) {
   *   if (ev.event === "triggered") notify(ev.state);
   * }
   * ```
   */
  async *watchEvents(opts: { signal?: AbortSignal } = {}): AsyncGenerator<WatchEvent> {
    let res: Response;
    try {
      res = await this.doFetch(this.base + "/watches/events", {
        headers: { Accept: "text/event-stream", ...this.headers },
        signal: opts.signal,
      });
    } catch (e) {
      throw new NescioError(
        0,
        `request to ${this.base}/watches/events failed: ${(e as Error).message}`,
      );
    }
    if (!res.ok || !res.body) {
      throw new NescioError(res.status, `HTTP ${res.status}`);
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      buf += decoder.decode(value, { stream: true });
      let i: number;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        const ev = parseSseFrame(buf.slice(0, i));
        buf = buf.slice(i + 2);
        if (ev) yield ev;
      }
    }
  }

  // ----------------------------------------------------------------- plumbing

  private async get(path: string, params: Record<string, unknown> = {}): Promise<any> {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) qs.set(k, String(v));
    }
    const query = qs.toString();
    return this.request("GET", query ? `${path}?${query}` : path);
  }

  private async post(path: string, body: unknown): Promise<any> {
    return this.request("POST", path, JSON.stringify(body));
  }

  private async request(method: string, path: string, body?: string): Promise<any> {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), this.timeoutMs);
    let res: Response;
    try {
      res = await this.doFetch(this.base + path, {
        method,
        body,
        headers: body ? { "Content-Type": "application/json", ...this.headers } : this.headers,
        signal: ctrl.signal,
      });
    } catch (e) {
      throw new NescioError(0, `request to ${this.base}${path} failed: ${(e as Error).message}`);
    } finally {
      clearTimeout(timer);
    }
    const text = await res.text();
    const parsed = text ? JSON.parse(text) : undefined;
    if (!res.ok) {
      throw new NescioError(res.status, parsed?.error ?? `HTTP ${res.status}`);
    }
    return parsed;
  }
}

/** An entity id bound to a client — sugar for entity-centric app code. */
export class EntityHandle {
  private readonly db: NescioClient;
  readonly id: string;

  constructor(db: NescioClient, id: string) {
    this.db = db;
    this.id = id;
  }

  bound(slot: string, opts?: { at?: At; credible?: number }): Promise<Bound> {
    return this.db.bound(this.id, slot, opts);
  }

  sample(opts?: { seed?: number; at?: At }): Promise<Record<string, number | string>> {
    return this.db.sample(this.id, opts);
  }

  certainly(slot: string, pred: Predicate, opts?: { at?: At }): Promise<Tri> {
    return this.db.certainly(this.id, slot, pred, opts);
  }

  ingest(c: Claim, source: string, opts?: { at?: At }): Promise<{ ok: boolean; observedAt: number }> {
    return this.db.ingest(this.id, c, source, opts);
  }

  resolve(req: {
    slot: string;
    targetBits: number;
    actions: ProcurementAction[];
    maxSteps?: number;
    mc?: number;
    seed?: number;
    at?: At;
  }): Promise<ResolvePlan> {
    return this.db.resolve({ ...req, entity: this.id });
  }

  decide(req: {
    slot: string;
    objective: Objective;
    target: number;
    actions: ProcurementAction[];
    maxSteps?: number;
    mc?: number;
    seed?: number;
    at?: At;
  }): Promise<DecisionPlan> {
    return this.db.decide({ ...req, entity: this.id });
  }
}

// --------------------------------------------------------------- mappers

function mapBound(j: any): Bound {
  const raw = j.region as unknown[];
  const region: Region =
    raw.length > 0 && Array.isArray(raw[0])
      ? { kind: "intervals", intervals: raw as [number, number][] }
      : { kind: "values", values: raw as string[] };
  const entropyBits = j.entropy_bits as number;
  const maxEntropyBits = j.max_entropy_bits as number;
  return {
    entity: j.entity,
    slot: j.slot,
    region,
    entropyBits,
    maxEntropyBits,
    mapEstimate: j.map_estimate,
    knowledgeRatio: maxEntropyBits === 0 ? 1 : 1 - entropyBits / maxEntropyBits,
  };
}

function mapPlan(j: any): ResolvePlan {
  return {
    steps: (j.steps as any[]).map((s) => ({
      action: mapAction(s.action),
      expectedEntropyBits: s.expected_entropy_bits,
      expectedGainBits: s.expected_gain_bits,
    })),
    startEntropyBits: j.start_entropy_bits,
    plannedEntropyBits: j.planned_entropy_bits,
    validatedEntropyBits: j.validated_entropy_bits ?? null,
    totalCost: j.total_cost,
  };
}

function mapDecision(j: any): DecisionPlan {
  return {
    objective: j.objective,
    units: j.units,
    steps: (j.steps as any[]).map((s) => ({
      action: mapAction(s.action),
      expectedRisk: s.expected_risk,
      expectedGain: s.expected_gain,
    })),
    startRisk: j.start_risk,
    plannedRisk: j.planned_risk,
    validatedRisk: j.validated_risk ?? null,
    totalCost: j.total_cost,
    recommendedNow: j.recommended_now,
    recommendedAfter: j.recommended_after,
  };
}

function mapAction(j: any): ProcurementAction {
  return {
    name: j.name,
    slot: j.slot,
    cost: j.cost,
    source: mapSource(j.source),
    answerWidth: j.answer_width ?? undefined,
  };
}

function mapSource(j: any): Source {
  return {
    name: j.name,
    reliability: j.reliability,
    halfLifeDays: j.half_life_days ?? undefined,
    axiomatic: j.axiomatic ?? false,
  };
}

function mapWatchState(j: any): WatchState {
  return {
    name: j.name,
    entity: j.entity,
    slot: j.slot,
    maxEntropyBits: j.max_entropy_bits ?? undefined,
    minKnowledge: j.min_knowledge ?? undefined,
    triggered: j.triggered,
    thresholdBits: j.threshold_bits ?? undefined,
    entropyBits: j.entropy_bits ?? undefined,
    knowledge: j.knowledge ?? undefined,
    horizon: j.horizon ?? undefined,
    horizonDate: j.horizon_date ?? undefined,
    error: j.error ?? undefined,
  };
}

/** One SSE frame -> a WatchEvent; undefined for comments/pings. */
function parseSseFrame(frame: string): WatchEvent | undefined {
  let event = "";
  let data = "";
  for (const line of frame.split("\n")) {
    if (line.startsWith("event:")) event = line.slice(6).trim();
    else if (line.startsWith("data:")) data += line.slice(5).trim();
  }
  if (!data) return undefined;
  const j = JSON.parse(data);
  if (event === "snapshot") {
    return { event, asOf: j.as_of, watches: (j.watches as any[]).map(mapWatchState) };
  }
  if (event === "triggered" || event === "recovered") {
    return { event, state: mapWatchState(j) };
  }
  return undefined;
}

function mapFit(j: any): FittedDecay {
  return {
    sourceName: j.source_name,
    r0: j.r0,
    halfLifeDays: j.half_life_days ?? null,
    logLikelihood: j.log_likelihood,
    nObservations: j.n_observations,
  };
}

function sourceOut(s: Source): Record<string, unknown> {
  return {
    name: s.name,
    reliability: s.reliability,
    half_life_days: s.halfLifeDays,
    axiomatic: s.axiomatic ?? false,
  };
}

function actionOut(a: ProcurementAction): Record<string, unknown> {
  return {
    name: a.name,
    slot: a.slot,
    cost: a.cost,
    source: sourceOut(a.source),
    answer_width: a.answerWidth,
  };
}
