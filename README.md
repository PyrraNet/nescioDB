<p align="center">
  <img src="assets/nescio.png" alt="nescio" width="420">
</p>

<p align="center">
  <em>The database that knows what it doesn't know.</em>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/rust-1.75%2B-orange.svg" alt="Rust 1.75+">
</p>

---

**nescio** *(Latin: "I do not know")* stores ignorance as a first-class object. A field without evidence is not `NULL` — it is a region of maximal entropy. Evidence narrows regions, time widens them again, and the database can tell you which evidence to acquire next.

Instead of values, you store **claims**: who said what, when, and how reliable they are. Everything else — regions, entropy, answers — is derived at query time. Every source has a half-life; old claims lose their grip on the data by physics, not by TTL. A classical relational database is the special case where every claim is an axiom and every region is a point.

Built for data that is inherently uncertain, contradictory, and decaying: lead data, OSINT, sensor fusion, real-estate intelligence.

## The verbs

| Verb | Answers |
|---|---|
| `bound` | What is known — credible region and entropy in bits |
| `sample` | One concrete, consistent world, deterministic under a seed |
| `resolve` | Which minimal-cost evidence would push entropy under a target |
| `find` | Which entities *certainly* / *possibly* lie in a range |
| `join` | Entity pairs matching a relation — each with a probability *and* a three-valued certainty, because joining two regions is itself uncertain |
| `certainly` | Three-valued predicates: `true` / `possible` / `false` |

## Quick start

```bash
cargo install --path .

nescio init mydb --template real-estate

nescio ingest mydb --entity villa_1 --slot price --interval 900000..1000000 \
       --source broker --at 2026-06-25

nescio bound mydb --entity villa_1 --slot price --at 2026-07-03
```

```
BOUND villa_1.price as of 2026-07-03
  region (95%): [570000, 1210000]
  entropy: 4.20 of 7.64 bits (knowledge 45%)
  MAP estimate: 905000
```

Ask again a year later — same command, `--at 2027-07-03` — and the region has widened on its own. Erase a source with `nescio forget-source`, and every derived region widens correctly: there is no aggregate that could forget to forget.

Joins compare uncertain regions, so each match carries a probability and a certainty:

```bash
nescio join mydb --op approx --left price --right price --tol 50000   # comparable properties
nescio join mydb --op gt --left price --right price --certain          # A certainly dearer than B
nescio join mydb --op same --left city --right city                    # candidate duplicates
```

## As a server

```bash
nescio serve mydb --port 7777
```

```bash
curl 'localhost:7777/bound?entity=villa_1&slot=price&at=2026-07-03'
```

All verbs over HTTP/JSON, usable from any language. One process owns the database; reads run in parallel, writes are exclusive.

Typed clients for [TypeScript](clients/typescript/) and [Java](clients/java/) wrap every verb — both zero-dependency.

## As a library

```rust
use nescio::prelude::*;
use nescio::time::now_unix;
use std::path::Path;

let db = Db::open(Path::new("mydb"))?;
let q = Query::new(&db, now_unix());

let bound = q.bound("villa_1", "price", 0.95)?;
println!("{:.2} bits", bound.entropy_bits);
```

## Performance

Measured on an M-series MacBook, 200,000 entities / 400,000 evidence records (`cargo run --release --example bench`):

```
ingest (group commit, one fsync)   ~1.1M records/s
open / log replay                  ~1.2M records/s
bound                              4.5 µs  (8.6 µs with couplings)
resolve                            < 1 ms
```

The server runs reads in parallel; every write is durable before it is acknowledged.

## Storage

A database is a directory. Config is human-readable JSON; the evidence log is
a compact, append-only binary format (~2.6× smaller than JSONL, no parse cost
on replay). `nescio export` reconstructs readable JSONL any time, and `nescio
import` goes the other way.

```
mydb/
  schema.json     slots and couplings
  sources.json    reliability, half-life, axiomatic
  priors.json     shared priors
  log.bin         the evidence log (append-only binary)
```

## License

[MIT](LICENSE)
