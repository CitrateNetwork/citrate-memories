---
created: 2026-06-05T22:10:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
---

# citrate-memories — Architecture & Data Model

> The technical design. Companion to `00_OVERVIEW.md` (decisions) and
> `01_SCOPE_OF_WORK.md` (deliverables). Formal invariants are in `03_TLA_SPECS.md`.

## 1. Component map

```
crates/
├─ mem-core        // MemoryNode, Edge, ontology, content-hash identity, bitemporal
├─ mem-store       // MemoryDagStore<T> over reused KvStore + RocksDB; crypto-shred
├─ mem-index       // HNSW vector index; trust-tier filters; code-anchor index
├─ mem-ingest      // Derived-plane: git/.md/manifest → nodes/edges; trailer parser; cursor
├─ mem-assert      // Asserted-plane: signed assertions, memory-diff, quarantine, promotion
├─ mem-query       // recall (budget-shaped), as-of, analogize, verify, diff, blast-radius
├─ mem-authz       // CapabilityGrant ext, delegation chains, SIWE/OIDC resolver, analogy-layer authz
├─ mem-audit       // AuditChain ext (Memory events, resource_id); hot-log/cold-graph split
├─ mem-fed         // P2P sync; grow-only DAG merge + Belnap-join; chain-anchor
├─ mem-ml          // LoRA training on high-trust nodes; registry + gateway hot-swap
├─ mem-mcp         // MCP server + transport (stdio/HTTP) — the missing piece
└─ mem-cli         // operator CLI: ingest, backfill, verify, shred, anchor, status
deps (pinned via manifest):
  citrate-learning (embeddings, belnap, adapters)  — as-is
  citrate-consensus::dag_store::{KvStore, KvOp}     — trait import
  citrate-storage (RocksDB wrapper)                 — reuse
```

## 2. The two-plane model

| | Derived plane | Asserted plane |
|---|---|---|
| Source | git + `.md` + `manifest.toml` | agents / humans, over MCP |
| Determinism | **pure function of inputs**; identical on every machine | nondeterministic |
| Authority | **never canonical** (the `.md` is) | **canonical for its own content** (lives nowhere else) |
| Mutability | rebuildable from scratch | append-only, signed |
| Trust tier | `derived-deterministic` (highest) | `human-confirmed` › `agent-asserted` › `inferred-advisory` |
| Examples | Commit, ADR, Sprint, Claim, PinBump nodes; `implements`/`decides` edges from trailers | analogy edges, rationales, "tried X failed", memory-diffs, LLM-proposed edges (quarantined) |

Both planes share **one store, one ontology, one merge function**. An edge or node
records which plane and trust tier it belongs to; recall filters on it.

## 3. Ontology (versioned: `schema_version` on every record)

### 3.1 MemoryNode

```rust
struct MemoryNode {
    id: Hash,                 // = blake3(canonical_content || structure)  — NO embedding, NO timestamp
    schema_version: u16,
    plane: Plane,             // Derived | Asserted
    kind: NodeKind,
    repo: RepoId,             // tenant
    // bitemporal
    valid_from: Timestamp,    // when the fact became true in the world
    valid_to: Option<Timestamp>,
    observed_at: Timestamp,   // when the system learned it
    // provenance
    author: PrincipalRef,     // wallet/DID; for derived = "ingest@<cursor>"
    trust_tier: TrustTier,
    signature: Option<Sig>,   // required for Asserted; absent for Derived (rebuildable)
    // canonical pointer (Rule 9 — never copy the artifact)
    source_ref: SourceRef,    // {repo, path, git_sha, byte_range} | {dag_native}
    // latent side-data (advisory, versioned, NOT in id)
    embedding: Option<VersionedVector>,   // {model_id@ver, Vec<f32>}
    confidence: Vec<BelnapValue>,         // per-dimension, CRDT-merged
    // code anchoring (D3.2)
    anchors: Vec<CodeAnchor>,             // {repo, path, symbol, line_range}
    status: Status,           // active | superseded | archived
    // crypto-shred: payload-at-rest encrypted per-tenant; id/structure cleartext for the DAG
}

enum NodeKind {
    Commit, Pr, Sprint(SprintPhase), Adr, Audit, Finding, Handoff,
    Narrative(NarrativeKind /*journal|essay|case_study*/), Claim(ClaimStatus),
    ManifestChange, PinBump, DriftEvent, Benchmark, AgentAction,
    Blocker, TechDebt, WorkPackage, Rationale, AnalogyHypothesis,
}
```

### 3.2 Edge

```rust
struct Edge {
    from: Hash, to: Hash,
    kind: EdgeKind,
    plane: Plane,
    trust_tier: TrustTier,
    provenance: EdgeProvenance,   // {method: trailer|nlp|analogy|manual, asserter, at, evidence_ref}
    confidence: Vec<BelnapValue>,
    quarantined: bool,            // inferred edges start true; promotion clears it
    signature: Option<Sig>,
}

enum EdgeKind {
    TemporalNext,       // the chronological spine (selected-parent analogue)
    MergeParent,        // multi-parent DAG
    Supersedes,         // append-only correction (Rule 3); must stay ACYCLIC
    DependsOn,          // from manifest [[drift]]
    CausedBy, Motivates,
    Implements,         // sprint-step → commit/PR
    Decides,            // ADR → sprint/decision
    Refutes, Contradicts,   // drives Belnap toward Both
    DerivedFrom,        // LoRA-provenance style lineage
    AnalogousTo,        // cross-DAG latent edge (authz-gated)
    References,         // citation
    AnchoredTo,         // node → code span
}
```

### 3.3 The structured-communication "struct" (decision #9)

Deterministic, machine-parseable, produces **load-bearing** Derived edges. Two carriers:

**Git commit trailers** (parsed at ingest):
```
Agentile-Sprint: SELL-S2
Agentile-Implements: SELL-S2#step-3
Agentile-Decides: ADR-2026-06-04-x402-pricing
Agentile-Data-Source: chain RPC eth_getLogs (Rule 7)
Agentile-Test-Delta: +14
Agentile-Blocker: PIN_DGX (resolves)
```

**Fenced block in any `.md`** (sprint/ADR/journal/handoff):
````
```agentile
node: claim
claim-status: corrected
supersedes: <node-id-or-slug>
refutes: <node-id>
anchors: citrate-chain/core/learning/adapters.rs#compose_lora
why: spectral-norm bound was stated as exact; it is an upper bound
```
````

Role gates (which fields each actor must emit) map onto existing rules:
Engineer → `Data-Source`(R7)+`Test-Delta`(R2)+bench ref(R6); PM → sprint/exit/blockers;
Product → `claim-status`+flow; DevOps → `pin/drift/anchor`+visibility(R13); Admin → grants+sign-offs(R10/13).
CI **nudges** (warns) on missing required trailers; never blocks (decision #9).

## 4. Identity, determinism & rebuild

- `node_id = blake3(canonical_content_bytes || structural_metadata)`. **Excludes** embedding and timestamps so re-embedding and re-ingestion are stable, and two machines agree.
- The **Derived plane is a pure function** of `(git state, .md set, manifest)`. `mem-cli rebuild` reproduces it byte-for-byte; mismatch = bug (TLA+ `Determinism` invariant).
- Embeddings are `VersionedVector{model_id@ver, data}`; similarity queries refuse to mix model versions; an embedding-model upgrade triggers a re-embed migration (D6.2).

## 5. Storage, crypto-shredding, vector index

- `MemoryDagStore<MemoryNode>`: generic DAG over reused `KvStore` (`kv_get/put/delete/iter/write_batch`) backed by `RocksDB`. Column families: `mem_nodes`, `mem_edges_out`, `mem_edges_in`, `mem_tips`, `mem_height_idx`, `mem_anchors`, `mem_cursor`, `mem_keys` (wrapped per-tenant DEKs).
- **Crypto-shredding:** each tenant has a DEK; node *payload* (the indexable content + embedding) is sealed AES-256-GCM (explorer envelope pattern); node *structure* (id, edges, timestamps) is cleartext so the DAG stays traversable. Forget(tenant|node) = destroy DEK; structure remains as a tombstone, content is unrecoverable. Federation moves **ciphertext**.
- **Vector index:** HNSW over decrypted-in-memory embeddings per session; persisted index is encrypted. ANN returns candidates; trust-tier + authz filters applied *before* content is revealed.

## 6. MCP tool surface (`mem-mcp`)

| Tool | Policy | Description |
|---|---|---|
| `recall` | read | Budget-shaped storyline/context. Args: `{repo, query?, anchors?, token_budget, trust_floor, as_of?}`. Returns ranked, **provenance-carrying**, freshness-stamped nodes/summary. |
| `blast_radius` | read | Given `file:symbol`(s) about to change, return anchored decisions/incidents/blockers. |
| `verify` | read | Independently verify a subgraph: signatures, pointer-resolves, content-hash, trust tiers. |
| `as_of` | read | Reconstruct what the graph knew at time T (decision replay). |
| `analogize` | read | Cross-DAG analogies under grant intersection; shapes-by-default. |
| `assert` | write | Append a signed Asserted node/edge (rationale, "tried X", confirmation). |
| `diff` / `merge_diff` | write | Emit/merge a session subgraph (memory-diff handoff). `blame` for provenance. |
| `confirm` | write(elevated) | Promote a quarantined inferred edge to load-bearing. |

Every response includes `{watermark: "derived@<sha>, Ns behind HEAD", provenance[], trust_tiers[]}`.

## 7. Authorization (decision #3, extended)

- **Principal** from `citrate-identity` SIWE/OIDC (wallet/DID = `sub`).
- **`CapabilityGrant`** extended (verified-needed): `allowed_resources: Vec<ResourceScope{resource_id:"repo:Y/memory", can_read, can_write}>`, plus **delegation chain** `Vec<DelegationStep{delegator, at, sig}>` for human→supervisor→worker fleets, with **revocation cascade**.
- `check_grant(..., resource_id, operation)` enforces resource + read/write + expiry + revocation + signature (already real).
- **Analogy-layer authz (R3):** a cross-DAG `AnalogousTo` traversal requires the *intersection* of grants over both tenants; without write/content rights on the far tenant, only structural shape (redacted) is returned.
- All writes append to `AuditChain` (`MemoryWrite` + `resource_id`); reads sampled into the hot log.

## 8. Federation & convergence (decision #2, Belnap-CRDT)

- Each repo = a tenant DAG; a **federation meta-DAG** holds cross-repo and analogy edges.
- Sync is **grow-only**: nodes/edges are content-addressed and append-only → set-union merge is conflict-free.
- **Confidence merges via Belnap join** — a verified bounded, idempotent, commutative lattice → a state-based CRDT. Two replicas that saw different evidence converge deterministically (e.g., `True ⊔ False = Both` flags a real contradiction rather than silently picking a winner).
- Supersession is a partial order kept acyclic (TLA+ invariant), so "latest understanding" is well-defined per replica and converges.
- **Chain-anchoring (phase 2):** periodic merkle root of a tenant DAG → citrate-chain via the existing `AuditChain.chain_anchor` field. CRDT gives *convergence*; the anchor gives *finality + tamper-evidence + global ordering*.

## 9. Analogy engine (decision #7, coarse-to-fine)

1. **Shortlist:** HNSW nearest-neighbors over node/subgraph embeddings (meaning).
2. **Verify:** bounded **structural-motif** match over the candidates (the "shape of prior actions") — fixed templates (e.g., `Blocker→Adr→Commit→Benchmark(regression)`) and/or WL graph-kernel similarity. Never arbitrary subgraph isomorphism.
3. **Gate:** authz intersection (R3); emit `AnalogousTo` edge into **quarantine** with provenance + Belnap confidence; promote only on `confirm`.

## 10. ML track (decisions #6, training)

- **Training set = high-trust nodes only** (`derived-deterministic` + `human-confirmed`); quarantined/inferred excluded → no poisoning into weights.
- Each LoRA's `ProvenanceChain` (reused) records the **DAG snapshot hash** it trained on; holdout eval gates registration.
- Register adapter on-chain (LoRA registry); `citrate-inference-gateway` hot-swaps it for the **Citrate support chatbot**, which answers over `recall` (RAG) + chain-specialized LoRA. Compose-vs-retrain policy bounds catastrophic forgetting.

## 11. Cross-cutting non-functionals
- **Freshness** surfaced everywhere (R2). **Verifiability** first-class (R1, audit posture). **Determinism** of Derived plane is a tested invariant. **Zero unwraps** in prod (Rule 8). **No mocks/stubs/TODOs** (Rule 1). **Schema versioned** for migration.
