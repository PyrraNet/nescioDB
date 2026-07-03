# nesciodb (TypeScript client)

Typed client for the [nescioDB](../../) HTTP API. Zero runtime dependencies —
uses the global `fetch` (Node 18+ or any modern browser).

```bash
npm install nesciodb
```

```ts
import { NescioClient, claim } from "nesciodb";

const db = new NescioClient("http://localhost:7777");

await db.ingest("villa_1", claim.interval("price", 900_000, 1_000_000), "broker", {
  at: "2026-06-25",
});

const b = await db.bound("villa_1", "price", { at: "2026-07-03" });
console.log(b.entropyBits, b.knowledgeRatio, b.region);

// The same query a year later — the region has widened on its own.
await db.bound("villa_1", "price", { at: "2027-07-03" });

// Joins over uncertain regions carry a probability AND a certainty.
const { matches } = await db.join(
  { op: "approx", left: "price", right: "price", tol: 50_000 },
  { minProbability: 0.5 },
);
for (const m of matches) console.log(m.left, m.right, m.probability, m.certainty);
```

## API

Every method mirrors a server verb and returns idiomatic camelCase types.

- `bound(entity, slot, { at?, credible? })` → `Bound` (region, entropy, `knowledgeRatio`)
- `sample(entity, { seed?, at? })` → one consistent world
- `certainly(entity, slot, pred, { at? })` → `"true" | "possible" | "false"`
- `find(slot, lo, hi, { mode?, at? })` → entity ids
- `join(predicate, options)` → `JoinResult` (`matches`, `pairsExamined`, `truncated`)
- `resolve({ entity, slot, targetBits, actions, ... })` → `ResolvePlan`
- `ingest` / `ingestBatch` / `putSource` / `forgetSource` / `recalibrate`
- `registerPrior` / `usePrior`, plus `health()` and `status()`

Times (`at`) accept an ISO date (`"2026-07-03"`), a datetime, or unix seconds.
Non-2xx responses throw `NescioError` with `.status` and the server message.

## Develop

```bash
npm install
npm run build        # tsc -> dist/
npm run example      # node examples/demo.ts  (needs a server on :7777)
```
