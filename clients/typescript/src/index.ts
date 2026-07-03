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

/** A point in time: an ISO date/datetime, or unix seconds. Defaults to now. */
export type At = string | number;

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
}

export class NescioClient {
  private readonly base: string;
  private readonly timeoutMs: number;
  private readonly doFetch: typeof fetch;

  constructor(baseUrl = "http://localhost:7777", opts: ClientOptions = {}) {
    this.base = baseUrl.replace(/\/$/, "");
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    const f = opts.fetch ?? globalThis.fetch;
    if (!f) {
      throw new Error("no fetch available — pass one via ClientOptions.fetch (Node < 18)");
    }
    this.doFetch = f.bind(globalThis);
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
      at: opts.at,
      credible: opts.credible,
    });
    return mapBound(j);
  }

  /** SAMPLE: one concrete, consistent world, deterministic under the seed. */
  async sample(
    entity: string,
    opts: { seed?: number; at?: At } = {},
  ): Promise<Record<string, number | string>> {
    return this.get("/sample", { entity, seed: opts.seed, at: opts.at });
  }

  /** Three-valued predicate: `true` / `possible` / `false` (region containment). */
  async certainly(
    entity: string,
    slot: string,
    pred: Predicate,
    opts: { at?: At } = {},
  ): Promise<Tri> {
    const params: Record<string, unknown> = { entity, slot, op: pred.op, at: opts.at };
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
    return this.get("/find", { slot, lo, hi, mode: opts.mode, at: opts.at });
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
      at: opts.at,
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
      at: req.at,
    });
    return mapPlan(j);
  }

  // ------------------------------------------------------------------- writes

  /** Append one evidence record to the log. */
  async ingest(
    entity: string,
    c: Claim,
    source: string,
    opts: { at?: At } = {},
  ): Promise<{ ok: boolean; observedAt: number }> {
    const j = await this.post("/ingest", { entity, claim: c, source, at: opts.at });
    return { ok: j.ok, observedAt: j.observed_at };
  }

  /** Append many records with a single group commit (one fsync). */
  async ingestBatch(
    records: { entity: string; claim: Claim; source: string; at?: At }[],
  ): Promise<{ ok: boolean; ingested: number }> {
    return this.post("/ingest-batch", records);
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
        headers: body ? { "Content-Type": "application/json" } : undefined,
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
