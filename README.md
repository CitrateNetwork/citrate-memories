# citrate-memories

> A federated, MCP-accessible **knowledge DAG** that gives every agent (and human)
> the storyline and code-shape of any Citrate repo in seconds — without reading the
> code — and never lets them act on stale, contradicted, or unverified information.
> "Git for agentic operators," off-GitHub.

**Status:** v1 first pass, in active development. This cycle is exploratory; a
greenfield v2 rebuild is planned once the full cycle teaches us where the design
holds and where it doesn't.

## Read first
The complete design lives in [`PLANSET/`](PLANSET/):
- [`00_OVERVIEW.md`](PLANSET/00_OVERVIEW.md) — vision, the 11 locked decisions, architecture, reuse map
- [`01_SCOPE_OF_WORK.md`](PLANSET/01_SCOPE_OF_WORK.md) — phases, deliverables, risk register
- [`02_ARCHITECTURE.md`](PLANSET/02_ARCHITECTURE.md) — two-plane model, ontology, authz, crypto, MCP surface
- [`03_TLA_SPECS.md`](PLANSET/03_TLA_SPECS.md) — formal invariants
- [`04_FEATURES_BDD.md`](PLANSET/04_FEATURES_BDD.md) — Gherkin features
- [`05_SPRINTS_AND_WPS.md`](PLANSET/05_SPRINTS_AND_WPS.md) — sprints & work packages
- [`06_WEBAPP_FRONTEND_SPEC.md`](PLANSET/06_WEBAPP_FRONTEND_SPEC.md) — the front-end brief (capability→UI, auth/RBAC, viz semantics)
- [`07_IMPLEMENTATION_AND_HARDENING_PLAN.md`](PLANSET/07_IMPLEMENTATION_AND_HARDENING_PLAN.md) — **the webapp build plan**: 1:1 design pin, connection hardening, quantum-safe crypto roadmap, test strategy, WPs

## Webapp (Memrizz)
The on-brand design landed; see [`webapp/`](webapp/) — the design prototype
(1:1 source of truth) plus the Next.js 16 foundation. Built per `PLANSET/07`.

## The core invariant
> **Derived = deterministic & rebuildable. Asserted = nondeterministic but signed &
> append-only. Any LLM/heuristic output is an _assertion_, never a _derivation_.**

## Workspace (current)
```
crates/
├─ mem-core    # ontology, content-only identity, bitemporal nodes, Belnap confidence
└─ mem-store   # generic content-addressed DAG over a KvStore backend (in-memory; RocksDB next)
```
More crates (`mem-ingest`, `mem-query`, `mem-mcp`, `mem-authz`, `mem-fed`, `mem-ml`, …)
land per `PLANSET/05`.

## Build
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Agentile
This repo follows the [Agentile methodology](../AGENTILE.md). Active sprint:
`MEM-S0` (Foundations) — see `citrate-federation/repos/citrate-memories/sprints/`.

---
© 2026 Citrate Inc.. Licensed under Apache-2.0.
