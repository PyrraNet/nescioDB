"""nesciodb — Python client for the nescioDB HTTP API.

nescioDB is a database whose primary object is ignorance: fields hold
regions with entropy, not values. Start a server with ``nescio serve``,
then talk to it from Python::

    from nesciodb import NescioClient, claim

    db = NescioClient("http://localhost:7777")
    db.ingest("villa_1", claim.interval("price", 900_000, 1_000_000),
              "broker", at="2026-06-25")

    b = db.bound("villa_1", "price", at="2026-07-03")
    print(b.entropy_bits, b.knowledge_ratio, b.region)

Zero dependencies — urllib only. Every method mirrors a server verb; every
``at`` accepts unix seconds, an ISO string, or a ``datetime.date`` /
``datetime.datetime``. Non-2xx responses raise :class:`NescioError` with
``.status`` and the server's message.
"""

from __future__ import annotations

import datetime as _dt
import json as _json
import urllib.error as _urlerror
import urllib.parse as _urlparse
import urllib.request as _urlrequest
from dataclasses import dataclass, field
from typing import Any, Dict, Iterator, List, Optional, Sequence, Tuple, Union

__all__ = [
    "NescioClient",
    "Entity",
    "NescioError",
    "claim",
    "domain",
    "coupling",
    "objective",
    "source",
    "action",
    "Bound",
    "JoinMatch",
    "JoinResult",
    "ResolveStep",
    "ResolvePlan",
    "DecisionStep",
    "DecisionPlan",
    "FittedDecay",
    "SlotRemoval",
    "WatchState",
    "WatchEvent",
]

__version__ = "0.8.0"

#: A point in time: unix seconds, an ISO string, or a date/datetime.
At = Union[int, str, _dt.date, _dt.datetime]

Json = Dict[str, Any]


class NescioError(Exception):
    """Raised for any non-2xx response (or a network failure, status 0)."""

    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status


# ------------------------------------------------------------------ helpers


class claim:
    """Claim constructors — what a source asserts about one slot."""

    @staticmethod
    def interval(slot: str, lo: float, hi: float) -> Json:
        """The value lies in [lo, hi] (continuous slots)."""
        return {"type": "interval", "slot": slot, "lo": lo, "hi": hi}

    @staticmethod
    def value(slot: str, value: str) -> Json:
        """The value is `value` (categorical slots)."""
        return {"type": "value", "slot": slot, "value": value}

    @staticmethod
    def not_value(slot: str, value: str) -> Json:
        """The value is NOT `value` (categorical slots)."""
        return {"type": "not_value", "slot": slot, "value": value}


class domain:
    """Domain constructors — a slot's state space, as in schema.json."""

    @staticmethod
    def continuous(lo: float, hi: float, n_bins: int) -> Json:
        return {"type": "continuous", "lo": lo, "hi": hi, "n_bins": n_bins}

    @staticmethod
    def categorical(*values: str) -> Json:
        return {"type": "categorical", "values": list(values)}

    @staticmethod
    def boolean() -> Json:
        return {"type": "categorical", "values": ["true", "false"]}


class coupling:
    """Coupling constructors. Slot order matters — see the file-format
    reference: gaussian_by_category wants (categorical, continuous),
    step_threshold (continuous, categorical), matrix (categorical,
    categorical)."""

    @staticmethod
    def gaussian_by_category(
        slot_a: str,
        slot_b: str,
        centers: Dict[str, float],
        sigma: float,
        name: Optional[str] = None,
    ) -> Json:
        c: Json = {
            "slot_a": slot_a,
            "slot_b": slot_b,
            "compat": {"kind": "gaussian_by_category", "centers": centers, "sigma": sigma},
        }
        if name:
            c["name"] = name
        return c

    @staticmethod
    def step_threshold(
        slot_a: str,
        slot_b: str,
        threshold: float,
        below: Dict[str, float],
        above: Dict[str, float],
        name: Optional[str] = None,
    ) -> Json:
        c: Json = {
            "slot_a": slot_a,
            "slot_b": slot_b,
            "compat": {
                "kind": "step_threshold",
                "threshold": threshold,
                "below": below,
                "above": above,
            },
        }
        if name:
            c["name"] = name
        return c

    @staticmethod
    def matrix(
        slot_a: str,
        slot_b: str,
        weights: Dict[str, Dict[str, float]],
        default: Optional[float] = None,
        name: Optional[str] = None,
    ) -> Json:
        compat: Json = {"kind": "matrix", "weights": weights}
        if default is not None:
            compat["default"] = default
        c: Json = {"slot_a": slot_a, "slot_b": slot_b, "compat": compat}
        if name:
            c["name"] = name
        return c

    @staticmethod
    def table(
        slot_a: str, slot_b: str, rows: Sequence[Sequence[float]], name: Optional[str] = None
    ) -> Json:
        c: Json = {
            "slot_a": slot_a,
            "slot_b": slot_b,
            "compat": {"kind": "table", "rows": [list(r) for r in rows]},
        }
        if name:
            c["name"] = name
        return c


class objective:
    """Objective constructors for :meth:`NescioClient.decide`."""

    @staticmethod
    def entropy() -> Json:
        """Report the full posterior; risk is Shannon entropy in bits."""
        return {"kind": "entropy"}

    @staticmethod
    def squared_error() -> Json:
        """Commit to the posterior mean; risk is the variance."""
        return {"kind": "squared_error"}

    @staticmethod
    def absolute_error() -> Json:
        """Commit to the posterior median; risk is expected absolute error."""
        return {"kind": "absolute_error"}

    @staticmethod
    def decision(loss: Sequence[Sequence[float]], labels: Optional[Sequence[str]] = None) -> Json:
        """A finite decision with loss[d][cell]; risk is expected loss."""
        o: Json = {"kind": "decision", "loss": [list(r) for r in loss]}
        if labels is not None:
            o["labels"] = list(labels)
        return o


def source(
    name: str,
    reliability: float,
    half_life_days: Optional[float] = None,
    axiomatic: bool = False,
) -> Json:
    """An evidence source: base reliability, optional half-life, axiomatic flag."""
    s: Json = {"name": name, "reliability": reliability}
    if half_life_days is not None:
        s["half_life_days"] = half_life_days
    if axiomatic:
        s["axiomatic"] = True
    return s


def action(
    name: str,
    slot: str,
    cost: float,
    src: Json,
    answer_width: Optional[float] = None,
) -> Json:
    """A procurement action for resolve/decide: ask `src` about `slot` at `cost`."""
    a: Json = {"name": name, "slot": slot, "cost": cost, "source": src}
    if answer_width is not None:
        a["answer_width"] = answer_width
    return a


def _at(v: Optional[At]) -> Optional[Union[int, str]]:
    if v is None or isinstance(v, (int, str)):
        return v
    if isinstance(v, _dt.datetime):
        if v.tzinfo is not None:
            v = v.astimezone(_dt.timezone.utc).replace(tzinfo=None)
        return v.strftime("%Y-%m-%dT%H:%M:%S")
    if isinstance(v, _dt.date):
        return v.strftime("%Y-%m-%d")
    raise TypeError(f"cannot use {type(v).__name__} as a point in time")


# ------------------------------------------------------------ result types

#: A credible region: interval pairs for continuous slots, labels for categorical.
Region = Union[List[Tuple[float, float]], List[str]]


@dataclass
class Bound:
    """Result of BOUND: the region plus how ignorant the DB really is."""

    entity: str
    slot: str
    region: Region
    entropy_bits: float
    max_entropy_bits: float
    #: A number for continuous slots, a label for categorical ones.
    map_estimate: Union[float, str]

    @property
    def knowledge_ratio(self) -> float:
        """0 = knows nothing, 1 = fully collapsed."""
        if self.max_entropy_bits == 0:
            return 1.0
        return 1.0 - self.entropy_bits / self.max_entropy_bits


@dataclass
class JoinMatch:
    left: str
    right: str
    #: P(predicate holds) under the two entities' independent posteriors.
    probability: float
    #: "true" / "possible" / "false" — region containment, like `certainly`.
    certainty: str


@dataclass
class JoinResult:
    matches: List[JoinMatch]
    pairs_examined: int
    truncated: bool


@dataclass
class ResolveStep:
    action: Json
    expected_entropy_bits: float
    expected_gain_bits: float


@dataclass
class ResolvePlan:
    steps: List[ResolveStep]
    start_entropy_bits: float
    planned_entropy_bits: float
    #: Seeded Monte-Carlo estimate over full worlds — the number to trust.
    validated_entropy_bits: Optional[float]
    total_cost: float


@dataclass
class DecisionStep:
    action: Json
    expected_risk: float
    expected_gain: float


@dataclass
class DecisionPlan:
    """Result of DECIDE: risk is in the objective's own units, not bits."""

    objective: str
    units: str
    steps: List[DecisionStep]
    start_risk: float
    planned_risk: float
    #: Seeded Monte-Carlo estimate over full worlds — the number to trust.
    validated_risk: Optional[float]
    total_cost: float
    #: What the DB would decide right now.
    recommended_now: str
    #: What it would decide after executing the plan.
    recommended_after: str


@dataclass
class FittedDecay:
    source_name: str
    r0: float
    half_life_days: Optional[float]
    log_likelihood: float
    n_observations: int


@dataclass
class SlotRemoval:
    evidence_erased: int
    priors_removed: int
    watches_removed: int = 0


@dataclass
class WatchState:
    """A watch (standing question) evaluated at a point in time.

    Exactly one of ``max_entropy_bits`` / ``min_knowledge`` is the watch's
    condition. ``horizon`` is the knowledge horizon: when decay alone will
    trigger the watch (unix seconds, day granularity) — ``None`` when an
    axiomatic, non-decaying source pins the slot forever. ``error`` is set
    when evaluation failed (e.g. an axiom conflict), which triggers the
    watch."""

    name: str
    entity: str
    slot: str
    triggered: bool
    max_entropy_bits: Optional[float] = None
    min_knowledge: Optional[float] = None
    threshold_bits: Optional[float] = None
    entropy_bits: Optional[float] = None
    knowledge: Optional[float] = None
    horizon: Optional[int] = None
    horizon_date: Optional[str] = None
    error: Optional[str] = None


@dataclass
class WatchEvent:
    """One message from the ``/watches/events`` stream.

    ``event`` is "snapshot" (then ``watches`` and ``as_of`` are set) or
    "triggered" / "recovered" (then ``state`` is set)."""

    event: str
    watches: List[WatchState] = field(default_factory=list)
    state: Optional[WatchState] = None
    as_of: Optional[int] = None


# ---------------------------------------------------------------- client


class NescioClient:
    """Typed client for a `nescio serve` HTTP endpoint."""

    def __init__(
        self,
        base_url: str = "http://localhost:7777",
        timeout: float = 30.0,
        headers: Optional[Dict[str, str]] = None,
    ):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.headers = dict(headers or {})

    def entity(self, entity_id: str) -> "Entity":
        """A handle with the entity id bound: ``db.entity("villa_1").bound("price")``."""
        return Entity(self, entity_id)

    # ------------------------------------------------------- introspection

    def health(self) -> Json:
        return self._get("/health")

    def status(self) -> Json:
        return self._get("/status")

    # --------------------------------------------------------------- verbs

    def bound(
        self,
        entity: str,
        slot: str,
        credible: Optional[float] = None,
        at: Optional[At] = None,
    ) -> Bound:
        """BOUND: credible region + entropy — how ignorant is the DB about this slot?"""
        j = self._get(
            "/bound", entity=entity, slot=slot, credible=credible, at=_at(at)
        )
        region = j["region"]
        if region and isinstance(region[0], list):
            region = [tuple(iv) for iv in region]
        return Bound(
            entity=j["entity"],
            slot=j["slot"],
            region=region,
            entropy_bits=j["entropy_bits"],
            max_entropy_bits=j["max_entropy_bits"],
            map_estimate=j["map_estimate"],
        )

    def sample(
        self, entity: str, seed: int = 0, at: Optional[At] = None
    ) -> Dict[str, Union[float, str]]:
        """SAMPLE: one concrete, consistent world, deterministic under the seed."""
        return self._get("/sample", entity=entity, seed=seed, at=_at(at))

    def certainly(
        self,
        entity: str,
        slot: str,
        gt: Optional[float] = None,
        lt: Optional[float] = None,
        between: Optional[Tuple[float, float]] = None,
        is_: Optional[str] = None,
        is_not: Optional[str] = None,
        at: Optional[At] = None,
    ) -> str:
        """Three-valued predicate: "true" / "possible" / "false".

        Pass exactly one of ``gt``, ``lt``, ``between=(lo, hi)`` (continuous)
        or ``is_``, ``is_not`` (categorical).
        """
        preds = [p for p in (gt, lt, between, is_, is_not) if p is not None]
        if len(preds) != 1:
            raise ValueError("pass exactly one of gt / lt / between / is_ / is_not")
        params: Json = {"entity": entity, "slot": slot, "at": _at(at)}
        if gt is not None:
            params.update(op="gt", value=gt)
        elif lt is not None:
            params.update(op="lt", value=lt)
        elif between is not None:
            params.update(op="between", lo=between[0], hi=between[1])
        elif is_ is not None:
            params.update(op="is", value=is_)
        else:
            params.update(op="is_not", value=is_not)
        return self._get("/certainly", **params)["result"]

    def find(
        self,
        slot: str,
        lo: float,
        hi: float,
        mode: str = "possible",
        at: Optional[At] = None,
    ) -> List[str]:
        """FIND: entities whose region certainly lies in / possibly intersects [lo, hi]."""
        return self._get("/find", slot=slot, lo=lo, hi=hi, mode=mode, at=_at(at))

    def join(
        self,
        op: str,
        left: str,
        right: str,
        tol: Optional[float] = None,
        left_prefix: Optional[str] = None,
        right_prefix: Optional[str] = None,
        min_probability: Optional[float] = None,
        certain_only: Optional[bool] = None,
        require_evidence: Optional[bool] = None,
        limit: Optional[int] = None,
        at: Optional[At] = None,
    ) -> JoinResult:
        """JOIN: entity pairs matching a relation, each with a probability AND
        a three-valued certainty. ``op`` is gt | lt | approx | same;
        ``approx`` needs ``tol``."""
        predicate: Json = {"op": op, "left": left, "right": right}
        if tol is not None:
            predicate["tol"] = tol
        options = _drop_none(
            left_prefix=left_prefix,
            right_prefix=right_prefix,
            min_probability=min_probability,
            certain_only=certain_only,
            require_evidence=require_evidence,
            limit=limit,
        )
        j = self._post("/join", {"predicate": predicate, "options": options, "at": _at(at)})
        return JoinResult(
            matches=[JoinMatch(**m) for m in j["matches"]],
            pairs_examined=j["pairs_examined"],
            truncated=j["truncated"],
        )

    def resolve(
        self,
        entity: str,
        slot: str,
        target_bits: float,
        actions: Sequence[Json],
        max_steps: Optional[int] = None,
        mc: Optional[int] = None,
        seed: Optional[int] = None,
        at: Optional[At] = None,
    ) -> ResolvePlan:
        """RESOLVE: plan the minimal-cost evidence to push entropy under a target."""
        j = self._post(
            "/resolve",
            _drop_none(
                entity=entity,
                slot=slot,
                target_bits=target_bits,
                actions=list(actions),
                max_steps=max_steps,
                mc=mc,
                seed=seed,
                at=_at(at),
            ),
        )
        return ResolvePlan(
            steps=[
                ResolveStep(
                    action=s["action"],
                    expected_entropy_bits=s["expected_entropy_bits"],
                    expected_gain_bits=s["expected_gain_bits"],
                )
                for s in j["steps"]
            ],
            start_entropy_bits=j["start_entropy_bits"],
            planned_entropy_bits=j["planned_entropy_bits"],
            validated_entropy_bits=j.get("validated_entropy_bits"),
            total_cost=j["total_cost"],
        )

    def decide(
        self,
        entity: str,
        slot: str,
        objective: Json,
        target: float,
        actions: Sequence[Json],
        max_steps: Optional[int] = None,
        mc: Optional[int] = None,
        seed: Optional[int] = None,
        at: Optional[At] = None,
    ) -> DecisionPlan:
        """DECIDE: plan the evidence that most improves a *decision* — the
        Value of Information for the call you actually face."""
        j = self._post(
            "/decide",
            _drop_none(
                entity=entity,
                slot=slot,
                objective=objective,
                target=target,
                actions=list(actions),
                max_steps=max_steps,
                mc=mc,
                seed=seed,
                at=_at(at),
            ),
        )
        return DecisionPlan(
            objective=j["objective"],
            units=j["units"],
            steps=[
                DecisionStep(
                    action=s["action"],
                    expected_risk=s["expected_risk"],
                    expected_gain=s["expected_gain"],
                )
                for s in j["steps"]
            ],
            start_risk=j["start_risk"],
            planned_risk=j["planned_risk"],
            validated_risk=j.get("validated_risk"),
            total_cost=j["total_cost"],
            recommended_now=j["recommended_now"],
            recommended_after=j["recommended_after"],
        )

    # -------------------------------------------------------------- writes

    def ingest(self, entity: str, claim: Json, source: str, at: Optional[At] = None) -> int:
        """Append one evidence record; returns the stored observed_at."""
        j = self._post(
            "/ingest", {"entity": entity, "claim": claim, "source": source, "at": _at(at)}
        )
        return j["observed_at"]

    def ingest_batch(self, records: Sequence[Json]) -> int:
        """Append many records with a single group commit (one fsync).

        Each record: ``{"entity", "claim", "source", "at"?}`` — ``at`` may be
        anything :data:`At` accepts.
        """
        out = []
        for r in records:
            r = dict(r)
            if "at" in r:
                r["at"] = _at(r["at"])
            out.append(r)
        return self._post("/ingest-batch", out)["ingested"]

    def put_source(
        self,
        name: str,
        reliability: float,
        half_life_days: Optional[float] = None,
        axiomatic: bool = False,
    ) -> int:
        """Register or update a source; returns how many log entries were re-interpreted."""
        j = self._post("/sources", source(name, reliability, half_life_days, axiomatic))
        return j["reinterpreted"]

    def forget_source(self, name: str) -> int:
        """GDPR erasure: physically remove all evidence from a source."""
        return self._post("/forget-source", {"source": name})["erased"]

    def recalibrate(
        self,
        source_name: str,
        apply: bool = False,
        min_truth_reliability: Optional[float] = None,
    ) -> FittedDecay:
        """Learn a source's decay physics from ground truth in the log."""
        j = self._post(
            "/recalibrate",
            _drop_none(
                source=source_name, apply=apply, min_truth_reliability=min_truth_reliability
            ),
        )
        f = j["fit"]
        return FittedDecay(
            source_name=f["source_name"],
            r0=f["r0"],
            half_life_days=f.get("half_life_days"),
            log_likelihood=f["log_likelihood"],
            n_observations=f["n_observations"],
        )

    def register_prior(self, name: str, slot: str, weights: Sequence[float]) -> None:
        self._post("/priors/register", {"name": name, "slot": slot, "weights": list(weights)})

    def use_prior(self, entity: str, slot: str, name: str) -> None:
        self._post("/priors/use", {"entity": entity, "slot": slot, "name": name})

    # ---------------------------------------------------- schema evolution

    def add_slot(self, name: str, domain: Json) -> None:
        """Add a slot; every entity starts at maximal entropy on it."""
        self._post("/schema/add-slot", {"name": name, "domain": domain})

    def remove_slot(self, name: str) -> SlotRemoval:
        """Remove a slot: physically erases its evidence, priors and
        watches. Refused (400) while a coupling references the slot."""
        j = self._post("/schema/remove-slot", {"name": name})
        return SlotRemoval(
            evidence_erased=j["evidence_erased"],
            priors_removed=j["priors_removed"],
            watches_removed=j.get("watches_removed", 0),
        )

    def add_value(self, slot: str, value: str) -> int:
        """Extend a categorical slot; returns how many priors were extended."""
        return self._post("/schema/add-value", {"slot": slot, "value": value})[
            "priors_extended"
        ]

    def add_coupling(self, c: Json) -> None:
        """Add a coupling — it applies to every entity immediately."""
        self._post("/schema/add-coupling", c)

    def remove_coupling(self, name: str) -> None:
        """Remove a coupling by its label (defaults to "slot_a~slot_b")."""
        self._post("/schema/remove-coupling", {"name": name})

    # ------------------------------------------------------------- watches

    def watches(self, at: Optional[At] = None) -> List[WatchState]:
        """Every watch with its current state and knowledge horizon."""
        j = self._get("/watches", at=_at(at))
        return [_watch_state(w) for w in j["watches"]]

    def add_watch(
        self,
        name: str,
        entity: str,
        slot: str,
        max_entropy_bits: Optional[float] = None,
        min_knowledge: Optional[float] = None,
    ) -> WatchState:
        """Register a standing question: fire when the slot decays past a
        threshold. Pass exactly one of ``max_entropy_bits`` (bits) or
        ``min_knowledge`` (ratio in (0, 1]). Returns the initial state —
        including the knowledge horizon, the date decay alone will fire it.
        """
        j = self._post(
            "/watches",
            _drop_none(
                name=name,
                entity=entity,
                slot=slot,
                max_entropy_bits=max_entropy_bits,
                min_knowledge=min_knowledge,
            ),
        )
        return _watch_state(j["state"])

    def remove_watch(self, name: str) -> None:
        self._post("/watches/remove", {"name": name})

    def check_watches(self, at: Optional[At] = None) -> List[WatchState]:
        """Evaluate all watches; returns only the triggered ones."""
        j = self._get("/watches/check", at=_at(at))
        return [_watch_state(w) for w in j["triggered"]]

    def watch_events(self) -> Iterator[WatchEvent]:
        """Subscribe to watch transitions (Server-Sent Events).

        Yields a "snapshot" event first, then "triggered" / "recovered"
        states as they happen. Blocks between events with no timeout —
        iterate in a dedicated thread, or ``break`` when done::

            for ev in db.watch_events():
                if ev.event == "triggered":
                    notify(ev.state)
        """
        req = _urlrequest.Request(
            self.base_url + "/watches/events",
            headers={"Accept": "text/event-stream", **self.headers},
        )
        try:
            res = _urlrequest.urlopen(req)  # no timeout: the stream lives on
        except _urlerror.URLError as e:
            raise NescioError(
                0, f"request to {self.base_url}/watches/events failed: {e.reason}"
            ) from None
        with res:
            event, data = "", []
            for raw in res:
                line = raw.decode("utf-8").rstrip("\r\n")
                if line.startswith("event:"):
                    event = line[6:].strip()
                elif line.startswith("data:"):
                    data.append(line[5:].strip())
                elif not line and data:  # blank line ends the frame
                    j = _json.loads("".join(data))
                    if event == "snapshot":
                        yield WatchEvent(
                            event=event,
                            as_of=j.get("as_of"),
                            watches=[_watch_state(w) for w in j.get("watches", [])],
                        )
                    elif event in ("triggered", "recovered"):
                        yield WatchEvent(event=event, state=_watch_state(j))
                    event, data = "", []
                elif not line:
                    event = ""  # comment/ping frame

    # ------------------------------------------------------------ plumbing

    def _get(self, path: str, **params: Any) -> Any:
        query = _urlparse.urlencode(
            {k: v for k, v in params.items() if v is not None}
        )
        return self._request("GET", f"{path}?{query}" if query else path)

    def _post(self, path: str, body: Any) -> Any:
        return self._request("POST", path, _json.dumps(body).encode())

    def _request(self, method: str, path: str, body: Optional[bytes] = None) -> Any:
        headers = {"Content-Type": "application/json", **self.headers}
        req = _urlrequest.Request(
            self.base_url + path, data=body, headers=headers, method=method
        )
        try:
            with _urlrequest.urlopen(req, timeout=self.timeout) as res:
                return _json.loads(res.read() or b"null")
        except _urlerror.HTTPError as e:
            raw = e.read()
            try:
                message = _json.loads(raw)["error"]
            except Exception:
                message = raw.decode(errors="replace") or f"HTTP {e.code}"
            raise NescioError(e.code, message) from None
        except _urlerror.URLError as e:
            raise NescioError(0, f"request to {self.base_url}{path} failed: {e.reason}") from None


@dataclass
class Entity:
    """An entity id bound to a client — sugar for entity-centric app code."""

    db: NescioClient
    id: str

    def bound(self, slot: str, **kw: Any) -> Bound:
        return self.db.bound(self.id, slot, **kw)

    def sample(self, **kw: Any) -> Dict[str, Union[float, str]]:
        return self.db.sample(self.id, **kw)

    def certainly(self, slot: str, **kw: Any) -> str:
        return self.db.certainly(self.id, slot, **kw)

    def ingest(self, claim: Json, source: str, **kw: Any) -> int:
        return self.db.ingest(self.id, claim, source, **kw)

    def resolve(self, slot: str, **kw: Any) -> ResolvePlan:
        return self.db.resolve(self.id, slot, **kw)

    def decide(self, slot: str, **kw: Any) -> DecisionPlan:
        return self.db.decide(self.id, slot, **kw)


def _drop_none(**kw: Any) -> Json:
    return {k: v for k, v in kw.items() if v is not None}


def _watch_state(j: Json) -> WatchState:
    return WatchState(
        name=j["name"],
        entity=j["entity"],
        slot=j["slot"],
        triggered=j["triggered"],
        max_entropy_bits=j.get("max_entropy_bits"),
        min_knowledge=j.get("min_knowledge"),
        threshold_bits=j.get("threshold_bits"),
        entropy_bits=j.get("entropy_bits"),
        knowledge=j.get("knowledge"),
        horizon=j.get("horizon"),
        horizon_date=j.get("horizon_date"),
        error=j.get("error"),
    )
