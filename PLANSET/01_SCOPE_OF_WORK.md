---
created: 2026-06-05T22:05:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
---

# citrate-memories — Scope of Work

> Companion to `00_OVERVIEW.md`. Defines what we build, in what order, what we
> explicitly do **not** build, the dependencies, and the risks we accept and
> mitigate. Sprints/WPs that execute this SOW live in `05_SPRINTS_AND_WPS.md`.

## 1. Goal

Ship a federated, MCP-accessible memory DAG that:

1. **Indexes** the existing Markdown/git/manifest truth deterministically (Derived plane).
2. **Records** agent/human knowledge that lives nowhere else (Asserted plane).
3. Returns **budget-shaped, provenance-carrying, trust-tiered** recall to agents.
4. Never lets an agent act on stale/contradicted/unverified data (freshness watermark + Belnap).
5. Federates **conflict-free** across the org without GitHub (Belnap-CRDT), anchorable to chain.
6. Trains chain-specialized **LoRAs** on high-trust memory and serves a support chatbot.

## 2. Deliverables (by phase)

### Phase 0 — Foundations & de-risk (the "can-this-even-work" phase)
- `D0.1` Repo scaffold (`citrate-memories`), Agentile-compliant: `LICENSE`, `NOTICE`,
  `AUDIT_TIER.md`, `SECURITY.md`, `.agentile/` shim, CI from `CitrateNetwork/.github`.
- `D0.2` Federation onboarding: `citrate-federation/repos/citrate-memories/` (owners, deps),
  `manifest.toml` entry + `[[drift]]` entries for the reused `citrate-chain` crates.
- `D0.3` `MemoryDagStore<T>` — generic content-addressed DAG over the reused `KvStore`/RocksDB.
- `D0.4` Node identity rule implemented & tested: `id = hash(content + structure)`, embeddings excluded.
- `D0.5` Spike: HNSW vector index over `EmbeddingVector`; local embedding model (GGUF/ONNX) bundled, deterministic enough for advisory similarity.
- `D0.6` TLA+ spec of the core safety invariants (see `03_TLA_SPECS.md`), model-checked.

### Phase 1 — Derived plane (deterministic index)
- `D1.1` Ontology v1 (node/edge schema, versioned) — see `02_ARCHITECTURE.md §3`.
- `D1.2` Ingestion pipeline: git history + `.md` (frontmatter) + `manifest.toml` → nodes/edges. Cursor-resumable (explorer pattern).
- `D1.3` Structured-trailer parser (git trailers + fenced ```agentile blocks) → load-bearing edges.
- `D1.4` Bitemporal model + supersession (append-only, acyclic) + freshness watermark on every read.
- `D1.5` **Full federation backfill** (all 27 repos: git history + every `.agentile`/handoff `.md`). Cold-start killer.
- `D1.6` Per-tenant crypto-shred encryption at rest; per-repo tenant DAGs.

### Phase 2 — MCP surface + authz (agents can use it)
- `D2.1` MCP **transport** (stdio + HTTP) wrapping the policy boundary — the missing piece.
- `D2.2` `CapabilityGrant` extension: `allowed_resources`, read/write, delegation chain + revocation cascade.
- `D2.3` SIWE/OIDC principal resolution via `citrate-identity`.
- `D2.4` MCP tools: `recall`, `assert`, `verify`, `as_of`, `diff`, `analogize` (see `02 §6`).
- `D2.5` `AuditChain` extension: `MemoryRead/Write` events + `resource_id`; hot-log/cold-graph split.

### Phase 3 — The differentiators (more than RAG)
- `D3.1` **Recall-budget API** — "storyline of repo X in N tokens", hierarchical, drill-down.
- `D3.2` **Code-anchored recall** — nodes bound to `file:symbol` spans; blast-radius surfacing.
- `D3.3` **Memory-diff handoffs** — session subgraph emit/merge/blame; replaces `HANDOFF_*.md`.
- `D3.4` **As-of / decision-replay** queries.
- `D3.5` **Self-critic agent** — surfaces decisions w/o rationale, claims never verified, code w/o memory.

### Phase 4 — Asserted plane intelligence (semantic + analogical)
- `D4.1` Trust-tiered edges + provenance + Belnap confidence on every edge; quarantine for inferred edges.
- `D4.2` LLM edge-proposal worker (advisory → quarantine → confirm/promote).
- `D4.3` Analogy engine: embedding shortlist → bounded structural-motif verify; **authz at the analogy layer** (grant intersection; shapes-not-content by default).
- `D4.4` Cross-DAG federation meta-graph + analogy across tenant DAGs.

### Phase 5 — Federation & ML
- `D5.1` P2P federation sync: grow-only DAG merge + Belnap-join confidence (CRDT). Encrypted transport.
- `D5.2` Chain-anchoring: periodic merkle root → citrate-chain (reuse `AuditChain.chain_anchor`).
- `D5.3` LoRA training pipeline on high-trust nodes; version against DAG snapshot; holdout eval.
- `D5.4` On-chain LoRA registry integration + inference-gateway hot-swap.
- `D5.5` Citrate support chatbot over memory recall + chain-specialized LoRA.

### Phase 6 — Hardening & (optional) open-source
- `D6.1` Tier-1 security audit (citrate-security); threat model; fuzzing on parsers/transport.
- `D6.2` Schema-migration tooling; embedding-model migration job.
- `D6.3` Native-app packaging (the "citrate-memories app in the lineup").
- `D6.4` OSS extraction decision (ADR + Rule-13 visibility flip if approved).

## 3. In scope
- Federation-internal memory for all Tier-1 repos.
- Both planes, all four v1 differentiators, the ML/chatbot track.
- Reuse-first: `core/learning` as-is; `KvStore`/RocksDB/`CapabilityGrant`/`AuditChain` extended.

## 4. Explicitly out of scope (v1)
- Replacing the Markdown methodology (Derived plane indexes it; it stays canonical).
- A public/multi-org memory marketplace (federation-internal only until D6.4).
- Non-Citrate embedding/inference providers (local model + gateway only — security posture).
- Real-time collaborative editing of memory (sync is eventual-consistent CRDT, not OT).
- Arbitrary subgraph isomorphism (analogy is bounded motifs + graph kernels only).

## 5. Dependencies & assumptions
- **Hard deps (reused crates):** `citrate-chain/core/learning`, `core/consensus::dag_store::{KvStore,KvOp}`, `core/storage` RocksDB wrapper. Pinned via `manifest.toml` `[[drift]]` (Rules 11/12).
- **Service deps:** `citrate-identity` (SIWE/OIDC) for principals; `citrate-inference-gateway` for chatbot serving + LoRA hot-swap (Phase 5).
- **Assumption:** local embedding model is deterministic *enough* across machines that embeddings remain *advisory* — identity never depends on them (enforced by D0.4).
- **Assumption:** federation sync transport exists or is built in D5.1 (no GitHub dependency).

## 6. Risk register (top risks → mitigation)

| ID | Risk | Sev | Mitigation |
|---|---|---|---|
| R1 | **Memory poisoning** — inferred/wrong edges mislead future agents, compounding | 🔴 | Trust tiers + provenance + Belnap; inferred → quarantine; provenance-carrying recall; train LoRA only on high-trust |
| R2 | **Stale index with false authority** | 🔴 | Freshness watermark on every response; incremental ingest; as-of queries |
| R3 | **Cross-tenant analogy leak** | 🔴 | Authz at analogy layer; grant intersection; shapes-not-content default; TLA+ spec |
| R4 | **Embedding nondeterminism breaks IDs/federation** | 🔴 | Identity = content+structure only; embeddings versioned advisory side-data |
| R5 | **Cold start / low adoption death-spiral** | 🟠 | Phase-1 full backfill (D1.5) so graph is rich at launch; passive capture (low write friction) |
| R6 | **Markdown extraction ambiguity / hallucinated edges** | 🟠 | Deterministic trailers for load-bearing edges; LLM advisory-only to quarantine |
| R7 | **Redaction vs immutability (compliance)** | 🟠 | Crypto-shredding (decision #10) |
| R8 | **Reuse coupling — `core/learning` API churns** | 🟠 | Pin via manifest; thin adapter layer; contribute changes upstream not fork |
| R9 | **Schema evolution breaks queries** | 🟡 | Versioned ontology + migration tooling (D6.2) from day one |
| R10 | **Structural analogy cost explosion** | 🟡 | Bounded motif templates + graph kernels; never exact subgraph iso |
| R11 | **LoRA training collapse / drift** | 🟡 | Snapshot-versioned training; holdout eval; compose vs retrain policy |
| R12 | **Audit-log sync amplification** | 🟡 | Hot-log/cold-graph split; batch/sample reads |

## 7. Definition of done (v1)
- All Phase 0–5 deliverables shipped; Phase 6 audit passed.
- TLA+ invariants in `03_TLA_SPECS.md` model-checked green.
- BDD features in `04_FEATURES_BDD.md` passing.
- Rules 1, 2, 8 hold (no mocks/stubs/TODOs in prod, monotone tests, zero unwraps).
- An agent in any repo can `recall` the repo's storyline within a token budget, with
  provenance + freshness, and `assert`/`diff` its own session knowledge — and a fresh
  agent measurably onboards faster (define metric in D2.4 acceptance).
