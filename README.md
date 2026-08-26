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
(1:1 source of truth) plus a Next.js 16 app with auth and API routes wired to the
gateway. Built per `PLANSET/07`. Note: this is not a paid/billed product; there
is no billing surface.

## The core invariant
> **Derived = deterministic & rebuildable. Asserted = nondeterministic but signed &
> append-only. Any LLM/heuristic output is an _assertion_, never a _derivation_.**

## Workspace (current)
```
crates/
├─ mem-core     # ontology, content-only identity, bitemporal nodes, Belnap confidence
├─ mem-store    # content-addressed DAG over a KvStore backend (RocksDB) with per-tenant
│               #   XChaCha20-Poly1305 crypto-shredding at rest
├─ mem-ingest   # git/docs/frontmatter/chain-state ingestion into the Derived plane
├─ mem-index    # HNSW vector index + local embedding model
├─ mem-query     # query surface over the DAG
├─ mem-assert   # signed, append-only Asserted-plane records
├─ mem-authz    # capability grants + audit chain
├─ mem-sync     # federation merge + chain anchoring
├─ mem-mcp      # MCP-accessible surface
└─ mem-gateway  # HTTP control-plane: OIDC/JWKS auth, ed25519-signed writes, org/registry, webhooks
```
Further work lands per `PLANSET/05`.

## Build
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Agentile
This repo follows the [Agentile methodology](../AGENTILE.md). Implementation has
shipped well past the MEM-S0 foundations: ingestion, indexing, the gateway
control-plane (OIDC/JWKS, signed writes), encryption-at-rest, and federation
chain-anchoring are all in tree. The repo received its first security audit in
2026-06 (see `.agentile/AUDIT_REF.md`); the HIGH findings were remediated on
merged branches. Sprints live under
`citrate-federation/repos/citrate-memories/sprints/`.

---
© 2026 Citrate Inc. Licensed under Apache-2.0.
