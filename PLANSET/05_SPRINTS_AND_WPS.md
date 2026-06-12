---
created: 2026-06-05T22:25:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
sprint: citrate-memories-program
---

# citrate-memories — Sprints & Work Packages

> The execution plan for the SOW (`01`). Agentile-compliant: each sprint becomes a
> file under `citrate-federation/repos/citrate-memories/sprints/active/` (single-repo
> work) or `citrate-federation/agentile/sprints/active/` (cross-repo work) with
> Rule-12 frontmatter, daily updates, exit criteria. Cross-repo deps go through
> `manifest.toml` `[[drift]]` first (Rules 11/12). This file is the program-level
> index; it is **not** the per-sprint truth (Rule 4 — that lives in each sprint file).

## Program status (2026-06-11, Lane D close-out)

| Sprint | Status | Where |
|---|---|---|
| MEM-S0 Foundations | **CLOSED** (TLC-green + CI gate landed; spec bug caught + fixed) | `sprints/completed/2026-06/` |
| MEM-S1 Derived plane | **CLOSED** | `sprints/completed/2026-06/` |
| MEM-S2 MCP + authz | **CLOSED** (SIWE binding F-5 + revocation cascade F-7 deferred → v2, trigger Lane C IDP-S3) | `completed/2026-06/` |
| MEM-S3 Differentiators | **CLOSED** (as_of/verify/self-critic landed; token-budget WP-3.1 + code-anchored WP-3.2 → backlog) | `completed/2026-06/` |
| MEM-S4 Trust boundary + analogy | **CLOSED** (2026-06-10) | `completed/2026-06/` |
| MEM-S5 Federation (CRDT/anchor/transport) | **CLOSED** on 5.1–5.4 (chain anchor read-side live; HTTP transport) | `completed/2026-06/` |
| MEM-S5.5 ML track (LoRA + chatbot) | **BLOCKED** (split from S5; inference-gateway training infra) | `sprints/active/` |
| MEM-S6 Hardening & OSS | **SCOPED** (Tier-1 audit → federation audit queue; execution post-lane) | `sprints/active/` |

**Explicit deferrals (backlog, not dropped):** WP-3.1 token-budget recall
shaping; WP-3.2 code-anchored blast-radius (needs anchor-ingestion); WP-2.3 SIWE
principal binding (F-5) + delegation-revocation cascade (F-7), both v2 on the
Lane-C IDP-S3 trigger; WP-5.3 durable on-chain anchor *write* (operator step,
needs anchor contract); WP-5.4 gossip/peer-discovery; MEM-S5.5 LoRA/chatbot.

## Phasing & sequencing logic

```
P0 Foundations ──► P1 Derived plane ──► P2 MCP+authz ──► P3 Differentiators ─┐
   (de-risk)         (backfill = cold-start killer)        (more than RAG)    │
                                                                              ▼
                              P4 Asserted intelligence ──► P5 Federation+ML ──► P6 Harden/OSS
```
Critical path: **P0 → P1 → P2** must be serial (each builds the substrate the next needs).
P3 and P4 can overlap once P2 lands the MCP surface. P5 needs P4's trust tiers (no
poison into LoRAs) and P1's backfill (rich graph). P6 gates any external/public use.

## Cross-repo work packages (federation sprints)
- `XR-1` manifest + `[[drift]]` entries pinning `citrate-learning`, `citrate-consensus`(dag_store), `citrate-storage`. → `citrate-federation`.
- `XR-2` `CapabilityGrant` + `AuditChain` extensions. → `citrate-agent-runtime` (companion PR).
- `XR-3` Inference-gateway LoRA hot-swap + (optional) embeddings endpoint. → `citrate-inference-gateway`.
- `XR-4` On-chain LoRA registry wiring. → `citrate-chain`.
- `XR-5` SIWE/OIDC resource-scope claims for memory. → `citrate-identity`.

---

## Sprint MEM-S0 — Foundations & de-risk
**Goal.** Prove the substrate: generic DAG over reused KV, content-only identity, vector index, core TLA+ green.
| WP | Title | Deliv | Notes / acceptance |
|---|---|---|---|
| WP-0.1 | Repo scaffold + Agentile onboarding | D0.1, D0.2, XR-1 | CI green; manifest entry + drift pins; `.agentile/` shim |
| WP-0.2 | `MemoryDagStore<T>` over `KvStore`+RocksDB | D0.3 | atomic batches; CFs from §5; prop-tested |
| WP-0.3 | Content-only identity + bitemporal | D0.4 | `id=blake3(content‖structure)`; BDD "re-ingest identical" |
| WP-0.4 | HNSW index + bundled local embedding model | D0.5 | advisory similarity; versioned vectors; no network |
| WP-0.5 | TLA+ `SupersededDag`, `Ingestion`, `Authz` | D0.6 | TLC green in CI gate |
**Exit.** Two ingests of a fixture repo are byte-identical; TLC green; HNSW returns sane neighbors.

## Sprint MEM-S1 — Derived plane + backfill
**Goal.** A deterministic, rebuildable index of all 27 repos, encrypted, with load-bearing trailer edges.
| WP | Title | Deliv |
|---|---|---|
| WP-1.1 | Ontology v1 (versioned schema) | D1.1 |
| WP-1.2 | Ingestion pipeline (git + frontmatter + manifest), cursor-resumable | D1.2 |
| WP-1.3 | Structured-trailer + fenced-block parser → load-bearing edges | D1.3 |
| WP-1.4 | Supersession + freshness watermark | D1.4 |
| WP-1.5 | **Full federation backfill** (history + all .agentile/handoffs) | D1.5 |
| WP-1.6 | Per-tenant crypto-shred encryption at rest | D1.6 |
**Exit.** `recall`-less query API returns the storyline of any repo from real data; rebuild reproduces it; shred works (BDD).

## Sprint MEM-S2 — MCP surface + authz
**Goal.** Agents reach the graph over MCP, authorized, audited.
| WP | Title | Deliv |
|---|---|---|
| WP-2.1 | MCP transport (stdio + HTTP) over the policy boundary | D2.1 |
| WP-2.2 | `CapabilityGrant` ext + delegation chains + revocation cascade | D2.2, XR-2 |
| WP-2.3 | SIWE/OIDC principal resolver | D2.3, XR-5 |
| WP-2.4 | MCP tools `recall/verify/as_of/diff/assert` (baseline) | D2.4 |
| WP-2.5 | `AuditChain` Memory events + hot-log/cold-graph split | D2.5, XR-2 |
**Exit.** A real Claude Code / agent session connects, authenticates via SIWE, `recall`s with provenance+freshness, `assert`s a node, and is denied an ungranted tenant (BDD). **Onboarding metric** baselined.

## Sprint MEM-S3 — Differentiators
**Goal.** The four things that make it more than RAG.
| WP | Title | Deliv |
|---|---|---|
| WP-3.1 | Recall-budget API (hierarchical, drill-down) | D3.1 |
| WP-3.2 | Code-anchored / blast-radius recall | D3.2 |
| WP-3.3 | Memory-diff handoffs (emit/merge/blame) | D3.3 |
| WP-3.4 | As-of / decision-replay | D3.4 |
| WP-3.5 | Self-critic completeness agent | D3.5 |
**Exit.** All Feature scenarios for D3.* pass; a `HANDOFF_*.md` workflow is reproduced as a memory-diff end-to-end.

## Sprint MEM-S4 — Asserted intelligence (semantic + analogical)
**Goal.** Trustworthy inferred knowledge without poisoning.
| WP | Title | Deliv |
|---|---|---|
| WP-4.1 | Trust tiers + edge provenance + Belnap confidence everywhere | D4.1 |
| WP-4.2 | LLM edge-proposal worker → quarantine → confirm/promote | D4.2 |
| WP-4.3 | Analogy engine (coarse-to-fine) + **analogy-layer authz** | D4.3 |
| WP-4.4 | Federation meta-graph + cross-DAG analogy | D4.4 |
**Exit.** Analogy non-leak scenarios pass (TLA+ `Authz.NoLeakViaAnalogy` holds against traces); inferred edges never load-bearing without `confirm`.

## Sprint MEM-S5 — Federation & ML
**Goal.** Conflict-free org-wide sync; chain-specialized chatbot.
| WP | Title | Deliv |
|---|---|---|
| WP-5.1 | P2P sync: grow-only DAG + Belnap-CRDT merge (encrypted) | D5.1 |
| WP-5.2 | Chain-anchoring (merkle root → chain via `chain_anchor`) | D5.2, XR-4 |
| WP-5.3 | LoRA training on high-trust nodes; snapshot-versioned; holdout | D5.3 |
| WP-5.4 | On-chain LoRA registry + gateway hot-swap | D5.4, XR-3, XR-4 |
| WP-5.5 | Citrate support chatbot (recall RAG + LoRA) | D5.5 |
**Exit.** Two replicas converge (BDD); a LoRA trained only on high-trust nodes serves via gateway; chatbot answers a chain/support query with citations.

## Sprint MEM-S6 — Hardening & OSS decision
**Goal.** Audit-ready; durable; decide open-source.
| WP | Title | Deliv |
|---|---|---|
| WP-6.1 | Tier-1 security audit + threat model + parser/transport fuzzing | D6.1 |
| WP-6.2 | Schema-migration + embedding-model migration tooling | D6.2 |
| WP-6.3 | Native-app packaging (lineup app) | D6.3 |
| WP-6.4 | OSS extraction ADR + Rule-13 visibility flip (if approved) | D6.4 |
**Exit.** Audit findings resolved/accepted; migrations tested; (if approved) public-flip ADR with sign-offs.

---

## Agentile compliance checklist (applies to every WP)
- [ ] Rule 0: read `AGENT_ENTRY.md` + owners before starting.
- [ ] Rule 1: no mocks/stubs/TODOs in prod paths.
- [ ] Rule 2: `cargo test --workspace` count monotone non-decreasing.
- [ ] Rule 5: Rule-12 frontmatter on every doc/ADR/sprint.
- [ ] Rule 7: data-source trace before each ingest source / endpoint.
- [ ] Rule 8: zero `.unwrap()` in prod crates.
- [ ] Rules 11/12: `[[drift]]` entry before any cross-repo dep.
- [ ] TLA+ gate: re-run affected spec; green before merge.
- [ ] Each WP closes by updating its sprint file (Rule 4), not this index.

## Open decisions deferred to execution (not blocking the plan)
- Concrete local embedding model choice (bge-small vs gte vs nomic) — benchmark in WP-0.4.
- P2P transport mechanism for D5.1 (reuse libp2p from `core/network` vs lighter gossip) — ADR in MEM-S5.
- Recall ranking function specifics (recency × trust × graph-centrality × anchor-relevance) — tune in WP-3.1.
- Onboarding-speed metric definition (tokens-to-context vs task-completion-time) — baseline in WP-2.4.
