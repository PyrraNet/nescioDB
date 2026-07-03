# nesciodb (TypeScript client)

Typed client for the [nescioDB](../../) HTTP API. Zero runtime dependencies —
uses the global `fetch` (Node 18+ or any modern browser). The whole client is
[one file](src/index.ts); `npm install nesciodb`, or just vendor it.

```ts
import { NescioClient, claim } from "nesciodb";

const db = new NescioClient("http://localhost:7777");

await db.ingest("villa_1", claim.interval("price", 900_000, 1_000_000), "broker", {
  at: "2026-06-25",
});

const b = await db.bound("villa_1", "price", { at: new Date("2026-07-03") });
console.log(b.entropyBits, b.knowledgeRatio, b.region);

// The same query a year later — the region has widened on its own.
await db.bound("villa_1", "price", { at: "2027-07-03" });

// Entity handles for entity-centric code:
const villa = db.entity("villa_1");
await villa.certainly("price", { op: "lt", value: 1_500_000 });

// Joins over uncertain regions carry a probability AND a certainty.
const { matches } = await db.join(
  { op: "approx", left: "price", right: "price", tol: 50_000 },
  { minProbability: 0.5 },
);

// Value of Information: which evidence most improves the decision?
const plan = await villa.decide({
  slot: "price",
  objective: { kind: "absolute_error" },
  target: 10_000,
  actions: [{
    name: "pull the land registry", slot: "price", cost: 40,
    source: { name: "land_registry", reliability: 1.0, axiomatic: true },
    answerWidth: 20_000,
  }],
});
console.log(plan.recommendedNow, "->", plan.recommendedAfter, plan.validatedRisk);
```

## API

Every method mirrors a server verb and returns idiomatic camelCase types.

- `bound(entity, slot, { at?, credible? })` → `Bound` (region, entropy, `knowledgeRatio`)
- `sample(entity, { seed?, at? })` → one consistent world
- `certainly(entity, slot, pred, { at? })` → `"true" | "possible" | "false"`
- `find(slot, lo, hi, { mode?, at? })` → entity ids
- `join(predicate, options)` → `JoinResult` (`matches`, `pairsExamined`, `truncated`)
- `resolve({ entity, slot, targetBits, actions, … })` → `ResolvePlan`
- `decide({ entity, slot, objective, target, actions, … })` → `DecisionPlan` (Value of Information)
- `ingest` / `ingestBatch` / `putSource` / `forgetSource` / `recalibrate`
- `registerPrior` / `usePrior`, plus `health()` and `status()`
- schema evolution: `addSlot`, `removeSlot`, `addValue`, `addCoupling`, `removeCoupling`
- `entity(id)` → an `EntityHandle` with the entity bound: `db.entity("v1").bound("price")`

Builders for the wire formats: `claim.interval/value/notValue`,
`domain.continuous/categorical/boolean`,
`coupling.gaussianByCategory/stepThreshold/matrix/table`.

Times (`at`) accept an ISO date (`"2026-07-03"`), a datetime, unix seconds, or
a `Date`. Non-2xx responses throw `NescioError` with `.status` and the server
message. `ClientOptions` takes `timeoutMs`, a custom `fetch`, and extra
`headers` (e.g. auth for a reverse proxy).

## Develop

```bash
npm install
npm run build        # tsc -> dist/
npm run example      # node examples/demo.ts  (needs a server on :7777)
```
