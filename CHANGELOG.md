# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[0.6.0]: https://github.com/PyrraNet/nescioDB/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/PyrraNet/nescioDB/releases/tag/v0.5.0
