---
created: 2026-06-05T22:00:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
---

# citrate-memories — Planset Overview

> **One line.** A Rust, MCP-accessible **knowledge DAG** that gives every agent (and
> human) the *storyline* and *code-shape* of any federation repo in seconds —
> without reading the code — and never lets them act on stale, contradicted, or
> unverified information. "Git for agentic operators," off-GitHub.

This is the index document for the `citrate-memories` planset. Read it first, then
the numbered docs in order.

| # | Doc | What it is |
|---|---|---|
| 00 | `00_OVERVIEW.md` | This file — vision, locked decisions, architecture, reuse map |
| 01 | `01_SCOPE_OF_WORK.md` | Phases, deliverables, in/out of scope, dependencies, risk register |
| 02 | `02_ARCHITECTURE.md` | Components, two-plane data model, ontology, authz, crypto, MCP surface |
| 03 | `03_TLA_SPECS.md` | Formal spec plan + concrete TLA+ modules for the safety-critical cores |
| 04 | `04_FEATURES_BDD.md` | Gherkin features for every v1 capability |
| 05 | `05_SPRINTS_AND_WPS.md` | Sprint plan + work packages, Agentile-compliant |

## Why this exists

Agentile today is **Markdown + git + one `manifest.toml`**. The source of truth is
strong and auditable, but it is **not queryable**: answering "what's the state of
repo X, what decisions shaped this function, what did we know when we shipped this?"
means grepping prose across 27 repos. Tokens cost time and money; agents re-read the
same ground every session and still miss the storyline. `citrate-memories` turns the
federation's lived history into a fast, trustworthy, latent-space-aware graph that
agents query over MCP.

It does **not** replace the Markdown methodology. It **indexes** it (Derived plane)
and **extends** it with knowledge that lives nowhere else (Asserted plane).

## The 11 locked decisions (quorum 2026-06-05)

| # | Decision | Choice |
|---|---|---|
| 1 | Source-of-truth relationship | **Projection/index over Markdown** (refined by #8) |
| 2 | Storage substrate | **Standalone Rust + RocksDB merkle-DAG**, sync via federation, **chain-anchor later** |
| 3 | Authorization | **Reuse `CapabilityGrant` + SIWE/OIDC**, add resource scopes + delegation |
| 4 | v1 ambition | **Research-grade from start** (semantic + analogical + LoRA + chatbot) |
| 5 | Embeddings | **Local in-process model bundled** (GGUF/ONNX), d≤1024, no network |
| 6 | Chatbot / LoRA | **Open-weight (Gemma/Llama) via inference-gateway + LoRA hot-swap** from on-chain registry |
| 7 | Analogy | **Coarse-to-fine**: embedding shortlist → structural-motif verify |
| 8 | Graph shape | **Two-plane**: Derived (deterministic, rebuildable) + Asserted (signed, append-only, canonical-for-its-own-content) |
| 9 | Struct enforcement | **Hybrid**: deterministic trailers/blocks → load-bearing; LLM proposes advisory edges to **quarantine**; CI nudges |
| 10 | Redaction | **Crypto-shredding** (per-tenant encrypt-at-rest; forget = destroy key) |
| 11 | v1 differentiators | **All four**: recall-budget API, code-anchored recall, memory-diff handoffs, as-of/decision-replay + self-critic |

## The core invariant (everything hangs off this)

> **Derived = deterministic & rebuildable. Asserted = nondeterministic but signed &
> append-only. Any LLM/heuristic output is an _assertion_, never a _derivation_.**

This single rule keeps the shared graph reproducible across every teammate's machine
(the Derived plane is a pure function of git + `.md`), while still giving agents a
place to record live reasoning (the Asserted plane), and prevents non-deterministic
model output from ever silently diverging the shared memory.

## Architecture at a glance

```
   git + .md + manifest                 agents / humans
   (canonical, per repo)                (over MCP, SIWE-authed)
            │                                   │
            │ deterministic ingest              │ signed assertions / memory-diffs
            ▼                                   ▼
   ┌───────────────────────┐         ┌───────────────────────┐
   │   DERIVED PLANE        │         │   ASSERTED PLANE       │
   │  (rebuildable index)   │◀──link──│ (analogies, rationale, │
   │  nodes ← artifacts     │         │  confirmations, diffs) │
   └───────────────────────┘         └───────────────────────┘
            │  both planes share one store, one ontology, one merge function
            ▼
   ┌──────────────────────────────────────────────────────────────┐
   │  MemoryDagStore<MemoryNode>  (generic DAG over reused KvStore) │
   │  RocksDB · per-tenant crypto-shred · HNSW vector index         │
   │  Belnap confidence (CRDT lattice) · bitemporal · provenance    │
   └──────────────────────────────────────────────────────────────┘
            │                                   │
   ┌────────▼─────────┐               ┌─────────▼──────────────┐
   │  MCP server      │               │  Federation sync (P2P) │
   │  + transport     │  authz:       │  grow-only DAG + Belnap │
   │  recall/assert/  │  CapabilityGrant │ join = conflict-free │
   │  analogize/verify│  + SIWE + deleg. │ chain-anchor (phase2)│
   │  diff/as-of      │               └────────────────────────┘
   └──────────────────┘
            │
   ┌────────▼───────────────────────────────────────────────┐
   │  ML layer: LoRA trained on high-trust nodes →           │
   │  on-chain LoRA registry → inference-gateway hot-swap →  │
   │  Citrate support chatbot (chain-specialized recall)     │
   └────────────────────────────────────────────────────────┘
```

## Reuse map (verified 2026-06-05)

| Need | Source | Verdict |
|---|---|---|
| Embeddings, cosine/euclidean | `citrate-chain/core/learning/embeddings.rs` | ✅ drop-in (dim 1–1024 configurable) |
| Confidence lattice (CRDT merge) | `core/learning/belnap.rs` | ✅ drop-in — verified bounded lattice, idempotent+commutative join |
| LoRA matrices + provenance + compose | `core/learning/adapters.rs` | ✅ drop-in — independent of consensus |
| KV abstraction + atomic batches | `core/consensus/dag_store.rs` (`KvStore`, `KvOp`) | ✅ import trait only |
| RocksDB wrapper, durability counters | `core/storage/src/db/rocks_db.rs` | ✅ reusable (decouple CF list) |
| Signed grants, expiry, revocation | `citrate-agent-runtime` `CapabilityGrant` | ⚠️ extend: +resource scopes, +read/write, +delegation chain |
| Tamper-evident hash-chain + chain-anchor | agent-runtime `AuditChain` | ⚠️ add `MemoryRead/Write` events + `resource_id` |
| MCP policy boundary | agent-runtime `McpServer` | ⚠️ **transport is missing** — net-new |
| DAG indexing patterns (cursor, superseded, blind-index) | `citrate-explorer` indexer | ✅ pattern reuse |
| Generic DAG node logic | — | ❌ net-new `MemoryDagStore<T>` |
| Vector index (ANN) | — | ❌ net-new (HNSW) |
| Embedding model (local) | — | ❌ net-new (bundle GGUF/ONNX) |
| Ingestion / trailers / extraction | — | ❌ net-new |
| Query engine + recall-budget | — | ❌ net-new |

Net: the **mathematics and the durable-storage primitives exist**; the
**systems-integration and product layer is the build**.

## Related (Rule 9 — link, don't copy)

- Methodology: `../../AGENTILE.md`, `../../docs/AGENTILE_RULES.md`, `../../docs/AGENTILE_WORKFLOW.md`
- Control plane: `../../citrate-federation/manifest.toml`, `../../citrate-federation/routing/topology.md`
- Reused crates: `../../citrate-chain/core/learning/`, `../../citrate-chain/core/consensus/src/dag_store.rs`
- Authz/audit: `../../citrate-agent-runtime/agent-legacy/src/mcp_server.rs`, `../../citrate-agent-runtime/agent/core/src/audit/chain.rs`
- Indexer patterns: `../../citrate-explorer/src/lib/indexer/ingest.ts`
- Identity seam: `../../citrate-identity/` (SIWE/OIDC)
