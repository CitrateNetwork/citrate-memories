---
created: 2026-06-05T22:40:00Z
last_updated: 2026-07-16
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
planset: MEM
repo: citrate-memories (Tier-1)
---

# Agent Entry Point — citrate-memories

> Start here. This repo is the **federated agent memory DAG** — "git for agentic
> operators." A Rust, MCP-accessible knowledge graph that indexes the federation's
> Markdown/git history (Derived plane) and records agent/human knowledge that lives
> nowhere else (Asserted plane). Read this before touching the repo.

## Where am I?

`CitrateNetwork/citrate-memories` — v1 first pass. Exploratory cycle; a greenfield
v2 rebuild is planned once this cycle completes and teaches us where the design holds.

## The one rule everything depends on

> **Derived = deterministic & rebuildable. Asserted = nondeterministic but signed &
> append-only. Any LLM/heuristic output is an _assertion_, never a _derivation_.**

If you are about to write code that makes the Derived plane depend on a model
output, a timestamp, or anything non-reproducible — stop. That belongs in the
Asserted plane, signed.

## Canonical pointers

- **Design (read in order):** `PLANSET/00_OVERVIEW.md` → `01_SCOPE_OF_WORK.md` →
  `02_ARCHITECTURE.md` → `03_TLA_SPECS.md` → `04_FEATURES_BDD.md` →
  `05_SPRINTS_AND_WPS.md`.
- **Sprints:** `citrate-federation/repos/citrate-memories/sprints/` (MEM-S0
  foundations are long done; see current state below).
- **Security audit:** `.agentile/AUDIT_REF.md` — first dedicated audit ran 2026-06;
  HIGH findings remediated on merged branches.
- **Federation rules:** `../citrate-federation/.agentile/rules/CORE_RULES.md`
  (Rule 1 no mocks · Rule 2 tests monotone · Rule 5 frontmatter · Rule 8 zero
  unwraps · Rule 11 manifest canonical · Rule 12 drift map).

## Reuse contract (do not re-invent)

- `mem-core::belnap` is a v1 local copy of `citrate-chain/core/learning/belnap.rs`;
  the v2 rebuild imports the verified original. Same for embeddings/LoRA.
- `mem-store::kv::KvStore` mirrors `citrate_consensus::dag_store::KvStore`
  byte-for-byte; v2 swaps the `use`.
- Authz reuses `citrate-agent-runtime` `CapabilityGrant` + `AuditChain` (extended).

## Current state

Implementation is well past the MEM-S0 foundations. Shipped in tree:

- `mem-core` — ontology, content-only identity, Belnap confidence.
- `mem-store` — content-addressed DAG over a RocksDB `KvStore`, with per-tenant
  XChaCha20-Poly1305 crypto-shredding at rest.
- `mem-ingest` — git/docs/frontmatter/chain-state ingestion into the Derived plane.
- `mem-index` — HNSW vector index + local embedding model.
- `mem-query` / `mem-assert` — query surface and signed, append-only Asserted records.
- `mem-authz` — capability grants + audit chain.
- `mem-sync` — federation merge with chain anchoring.
- `mem-mcp` — MCP-accessible surface.
- `mem-gateway` — HTTP control-plane: OIDC/JWKS auth, ed25519-signed writes,
  org/registry, webhooks.
- `webapp/` (Memrizz) — Next.js 16 app with auth + API routes. Not a paid product;
  no billing surface.

The repo received its first security audit in 2026-06 (see `AUDIT_REF.md`); the
HIGH findings were remediated on merged branches. A greenfield v2 rebuild is still
planned once this cycle's lessons settle.
