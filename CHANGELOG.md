# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.0] — 2026-07-05

### Added
- **Watches & knowledge horizons.** A watch is a standing question — "fire
  when the entropy of `villa_1.price` exceeds 5 bits" (or `min_knowledge`
  as the relative form). Because decay is deterministic, every watch
  reports its **knowledge horizon**: the exact date decay alone will fire
  it, predicted the moment you register it. `nescio watch add | rm | list
  | check` (`check` exits 2 when triggered — cron-ready), routes
  `GET /watches`, `GET /watches/check`, `POST /watches`,
  `POST /watches/remove`, and `GET /watches/events` — a Server-Sent-Events
  stream fed by a background evaluator that re-checks after every write
  (`snapshot`, then `triggered` / `recovered` transitions). A watch also
  fires on axiom conflict; `schema remove-slot` cascades watches on the
  removed slot; watches persist in `watches.json`. Both clients cover it,
  including the stream (`db.watch_events()` generator in Python,
  `db.watchEvents()` async iterator in TypeScript).
- **Python client** (`clients/python`, package `nesciodb`): zero-dependency
  (urllib, 3.9+), fully typed, covers every verb including `decide` and
  schema evolution. Dataclass results, `datetime`-aware `at=`, entity
  handles (`db.entity("v1").bound("price")`), and builders for every wire
  format (`claim`, `domain`, `coupling`, `objective`, `source`, `action`).
- **TypeScript client 0.7.1**: full server parity — `decide` (Value of
  Information), all `/schema/*` methods with `domain`/`coupling` builders,
  entity handles, `Date` support for `at`, extra headers option.
- Docs: client section rewritten — Python/TypeScript/Java tabs with
  end-to-end examples and a capability table.

## [0.7.1] — 2026-07-03

### Added
- **File-format reference** on the docs site: schema.json, all four coupling
  kinds with their required slot order, sources.json, priors.json, evidence
  JSONL, and resolve/decide actions — no more reading Rust source to learn a
  format.
- **Two new built-in templates** (`osint`, `sensor`) and a `nescio templates`
  command: list built-ins, or `--show NAME` to print a complete schema +
  sources pair as JSON to start your own from.
- **Readable dates everywhere.** `import` JSONL accepts `observed_at` as unix
  seconds *or* a date string ("2026-06-25", "2026-06-25T14:30"), plus `at` as
  a field alias — hand-written evidence files no longer need a timestamp
  converter. `init --sources` now also accepts the name-keyed map that
  nescio itself writes, so a file copied from an existing database just works.

## [0.7.0] — 2026-07-03

### Added
- **Schema evolution.** A live database can now grow and shrink:
  `nescio schema add-slot | remove-slot | add-value | add-coupling |
  remove-coupling`, the matching `POST /schema/*` routes, and the
  corresponding `Db` methods. Adding a slot needs no backfill (every entity
  starts at maximal entropy); extending a categorical slot keeps history
  valid (the log stores values as strings) and recompiles coupling tables;
  removing a slot physically erases its evidence and priors, and is refused
  while a coupling references the slot. All changes validate before they
  commit.

## [0.6.0] — 2026-07-03

### Added
- **Decision-theoretic RESOLVE (Value of Information).** `resolve` can now plan
  against an `Objective`: instead of "push entropy under a target", ask "which
  evidence purchase maximizes expected value minus cost". New types
  `Objective`, `DecisionPlan`, `DecisionStep`, `ProcurementAction`; new CLI
  flags and `/resolve` server parameters to match.

### Changed
- **Crate renamed `nesciodb` → `nescio`** (the `nesciodb` name on crates.io is
  retired). Library imports change from `use nesciodb::…` to `use nescio::…`;
  the binary, on-disk format, and HTTP API are unchanged.

## [0.5.0] — 2026-07-03

Initial public release.

- Core model: claims instead of values — evidence with source reliability,
  half-life decay (erosion), and couplings between slots.
- The verbs: `bound`, `sample`, `resolve`, `find`, `join`, `certainly`.
- Storage: directory layout with human-readable JSON config and an
  append-only binary evidence log (`log.bin`); `export`/`import` to and from
  JSONL; `forget-source` with correct region widening.
- HTTP server (`nescio serve`): all verbs over HTTP/JSON, parallel reads,
  exclusive durable writes.
- Zero-dependency typed clients for TypeScript and Java.
- Examples: `realestate` (end-to-end walkthrough) and `bench`.

[0.8.0]: https://github.com/PyrraNet/nescioDB/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/PyrraNet/nescioDB/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/PyrraNet/nescioDB/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/PyrraNet/nescioDB/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/PyrraNet/nescioDB/releases/tag/v0.5.0
