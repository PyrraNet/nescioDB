# nesciodb (Python client)

Typed client for the [nescioDB](../../) HTTP API. Zero dependencies —
`urllib` only, Python 3.9+. The whole client is
[one file](src/nesciodb/__init__.py); `pip install nesciodb`, or just copy it
into your project as `nesciodb.py`.

```python
from datetime import date
from nesciodb import NescioClient, claim, source, action

db = NescioClient("http://localhost:7777")

db.ingest("villa_1", claim.interval("price", 900_000, 1_000_000),
          "broker", at="2026-06-25")

b = db.bound("villa_1", "price", at=date(2026, 7, 3))
print(f"{b.entropy_bits:.2f} bits, knowledge {b.knowledge_ratio:.0%}")
print(b.region)          # [(570000.0, 1210000.0)]

# The same query a year later — the region has widened on its own.
db.bound("villa_1", "price", at=date(2027, 7, 3))

# Joins over uncertain regions carry a probability AND a certainty.
r = db.join(op="approx", left="price", right="price", tol=50_000,
            min_probability=0.5)
for m in r.matches:
    print(m.left, m.right, m.probability, m.certainty)

# Ask the DB what evidence to buy next.
plan = db.resolve("villa_1", "price", target_bits=2.0, actions=[
    action("call the broker", "price", cost=5,
           src=source("broker", 0.85, half_life_days=90),
           answer_width=100_000),
])
print(plan.validated_entropy_bits)   # the number to trust

# Entity handles for entity-centric code:
villa = db.entity("villa_1")
villa.certainly("price", lt=1_500_000)   # "true" | "possible" | "false"

# Watches: fire when knowledge decays past a threshold. The horizon —
# the date decay alone will fire it — is predicted on registration.
st = db.add_watch("price_fresh", "villa_1", "price", max_entropy_bits=5.0)
print(st.horizon_date)                   # "2026-08-10"
for ev in db.watch_events():             # SSE: snapshot, then transitions
    if ev.event == "triggered":
        notify(ev.state)
```

## API

Every method mirrors a server verb and returns typed dataclasses:

- `bound(entity, slot, credible=…, at=…)` → `Bound` (region, entropy, `knowledge_ratio`)
- `sample(entity, seed=…, at=…)` → one consistent world
- `certainly(entity, slot, gt=… | lt=… | between=(lo, hi) | is_=… | is_not=…, at=…)` → `"true" | "possible" | "false"`
- `find(slot, lo, hi, mode="possible"|"certain", at=…)` → entity ids
- `join(op, left, right, tol=…, min_probability=…, …)` → `JoinResult`
- `resolve(entity, slot, target_bits=…, actions=[…])` → `ResolvePlan`
- `decide(entity, slot, objective=…, target=…, actions=[…])` → `DecisionPlan` (Value of Information)
- `ingest` / `ingest_batch` / `put_source` / `forget_source` / `recalibrate`
- `register_prior` / `use_prior`, plus `health()` and `status()`
- schema evolution: `add_slot`, `remove_slot`, `add_value`, `add_coupling`, `remove_coupling`
- watches: `add_watch`, `remove_watch`, `watches()`, `check_watches()` → `WatchState`
  (with `horizon` / `horizon_date`), and `watch_events()` — a generator over the
  Server-Sent-Events stream (`WatchEvent`: snapshot, triggered, recovered)
- `entity(id)` → a handle with the entity bound: `db.entity("v1").bound("price")`

Constructors for the wire formats: `claim.interval/value/not_value`,
`domain.continuous/categorical/boolean`,
`coupling.gaussian_by_category/step_threshold/matrix/table`,
`objective.entropy/squared_error/absolute_error/decision`, `source(…)`,
`action(…)`.

Times (`at=`) accept an ISO string, unix seconds, `datetime.date`, or
`datetime.datetime`. Non-2xx responses raise `NescioError` with `.status`
and the server message.

## Develop

```bash
# Run a server first:
#   nescio init /tmp/demodb --template real-estate
#   nescio serve /tmp/demodb --port 7777
python3 examples/demo.py
```
