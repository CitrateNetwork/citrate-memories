---
created: 2026-07-11T00:00:00Z
branch: feat/e4-chain-ingest
author: Claude Fable 5 (E-4 agent), directed by @SaulBuilds
status: accepted
work_order: E-4 (citrate-federation planset 2026-07-11-core-upstream-gaps)
---

# ADR — Chain-state ingest: vocabulary, determinism, tenancy (E-4 WP-1)

> **One line.** citrate-chain state (network params, the deployed-contract
> catalog, watched-address events) becomes a first-class Derived-plane source,
> projected into the reserved **`chain-state`** tenant with the same guarantee
> as git ingest: the same source data produces **byte-identical node ids** on
> every machine, and every correction — re-org or redeploy — is a
> **supersession, never a mutation**.

## Context

Until now `mem-ingest` derived nodes only from git history and markdown
(`crates/mem-ingest/`). The chain code in `crates/mem-sync/src/chain.rs` only
*binds* a DAG root to a block (read-only checkpoint); it ingests nothing.
citrate-core v1 ships a per-user memory instance whose "chain-facts" tenant
should hold key chain state — this ADR defines the upstream vocabulary and
rules that ingestor obeys.

## Decision 1 — Tenant

All chain-derived nodes live in the reserved tenant **`chain-state`**
(`mem_ingest::chain::CHAIN_STATE_TENANT`), exactly parallel to the reserved
`federation` tenant (MEM-S4 WP-4.4). No schema change: tenant = the `repo`
string. The whole existing surface works on it for free — `memory_recall
repo="chain-state"` returns the catalog, authz scopes and crypto-shred apply
per the usual tenant rules, and the tenant is sync/anchor-able like any other.

Multiple chains coexist in the one tenant: every DagNative key is prefixed
`chain:<chainId>/…`, and per-chain cursors are keyed `chain_cursor:<chainId>`.

## Decision 2 — Node vocabulary

Four new `NodeKind`s (appended to the enum; discriminant strings are
identity-bearing and therefore frozen):

| Kind | Discriminant | One per | DagNative identity key |
|---|---|---|---|
| `ChainNetwork` | `chain_network` | chainId | `chain:<id>` |
| `ChainContract` | `chain_contract` | (group, name, **address**) | `chain:<id>/contract/<group>/<name>@<address>` |
| `ChainCheckpoint` | `chain_checkpoint` | witnessed (height, **hash**) | `chain:<id>/block/<number>@<hash>` |
| `ChainEvent` | `chain_event` | watched transaction | `chain:<id>/tx/<txhash>` |

Notes:

- **The address is identity-bearing for a contract.** A redeploy of
  `LearningPool` is a *new* node; the address-free prefix
  (`chain:<id>/contract/<group>/<name>`) is the *logical* key used only to
  detect the redeploy and mint the supersession.
- **The block hash is identity-bearing for a checkpoint.** Two forks at the
  same height are two nodes; "which one is canonical" is a *status* question
  (`Active`/`Superseded`), never an identity question.
- `ChainCheckpoint` here is a *node kind* (a witnessed block in the graph).
  `mem_sync::chain::ChainCheckpoint` remains the anchor-binding struct; the
  two serve different layers and deliberately share the name of the concept.
- `mem-gateway` renders all four kinds in a new `"chain"` material lane.

## Decision 3 — Edge vocabulary

Two new `EdgeKind`s (storage-key tags 15/16, append-only forever), plus reuse:

| Edge | From → To | Meaning |
|---|---|---|
| `Emits` (new, tag 15) | `ChainCheckpoint` → `ChainEvent` | the block emitted/contains this observed event |
| `Touches` (new, tag 16) | `ChainEvent` → `ChainContract` | the event involved this catalog contract (sender or recipient) |
| `References` (reuse) | `ChainContract` → `ChainNetwork` | "deployed-on" (evidence string `deployed-on`) |
| `TemporalNext` (reuse) | `ChainCheckpoint` → `ChainCheckpoint` | chronological spine, minted only when `parentHash` actually chains |
| `Supersedes` (reuse) | new node → orphaned/old node | re-org and redeploy corrections (Decision 4) |

`Emits` and `Touches` are **ingest-only**, like `TemporalNext`/`MergeParent`:
they are deliberately absent from `mem-mcp`'s proposable edge-kind list, so no
MCP client can assert them — they can only be derived.

Why `Emits` hangs off the checkpoint rather than the contract: every event has
a containing block, but a plain EOA transfer has no emitting contract. Binding
emission to the checkpoint guarantees a total, deterministic edge set;
contract involvement is expressed by `Touches`.

## Decision 4 — Determinism rules (the invariant, applied to chains)

1. **Same chain range ⇒ byte-identical node ids.** Node identity =
   `blake3(content ‖ structure)` per the core invariant (PLANSET/00). Nothing
   time-, host-, or embedding-dependent enters identity. Concretely:
   - catalog entries are BTree-sorted by (group, name) — source JSON order
     never leaks in;
   - all addresses and hashes are lowercased before entering a key or content
     (checksummed and lowercase spellings hash identically);
   - `now_ms` lands only on non-identity fields (`observed_at`, edge
     provenance); block-derived `valid_from` = block timestamp, which is
     itself chain data.
2. **Re-orgs are supersessions, not mutations.** A block at height H with a
   new hash is a NEW `ChainCheckpoint`; the still-`Active` checkpoint at H
   with a different hash is transitioned via the store's guarded
   `apply_supersession` (cycle/self/missing-target rejections are counted,
   never fatal). Nothing is deleted or rewritten — "what did we believe at
   time T" keeps working via the existing `as_of` machinery.
3. **Redeploys are supersessions.** A catalog entry whose (group, name)
   already exists `Active` at a different address supersedes the old node
   (evidence: `catalog redeploy: same (group, name), new address`).
   Both address generations remain in the graph.
4. **Fail-closed.** A malformed catalog, malformed block, bad hex quantity, or
   failed RPC round-trip returns `IngestError::Chain` and commits nothing.
   The live source verifies `eth_chainId` before any walk (same rule as
   `mem_sync::chain::fetch_chain_checkpoint`).
5. **Cursor after commit.** The event walk commits per block and advances the
   durable cursor (`chain_cursor:<chainId>`, `EventCursor { chain_id,
   next_block }`) only after that block's batch lands. Kill/restart resumes at
   the first uncommitted block; content-addressed ids make any overlap a
   no-op.
6. **No network in tests, ever.** Tests replay recorded
   `eth_getBlockByNumber` transcripts (`crates/mem-ingest/tests/fixtures/`)
   through the SAME parser (`parse_rpc_block`) the live source uses. The live
   `RpcBlockSource` reuses mem-sync's fail-closed JSON-RPC client and is
   feature-gated (`mem-ingest/chain`) so the default build is network-free.

Known limitation (deferred to WP-4): checkpoint supersession does not yet
cascade to the events of an orphaned block. The orphaned events remain
reachable only via the superseded checkpoint's `Emits` edges, which makes them
identifiable; the daemon-mode work package owns the cascade policy.

## Decision 5 — Trust and planes

Everything here is Derived plane, `TrustTier::DerivedDeterministic`,
`BelnapValue::True`, unquarantined — chain data observed over a verified-
chain-id RPC is a known-true fact exactly as a git commit's existence is. Any
*interpretation* of chain state (e.g. "this transfer funded X") is an
assertion and belongs to the Asserted plane, per the core invariant.

## Consumers (how to provision a chain-state tenant)

- **Static catalog (WP-2, shipped):** `mem_ingest::chain::ingest_chain_catalog`
  over a pinned `contracts/addresses/40204.json`-shaped file, or the
  `chain_ingest` example (`--features rocksdb`). Idempotent; the tenant
  watermark's head is the catalog's blake3 hash.
- **Event walk (WP-3, shipped as a library):**
  `mem_ingest::chain::ingest_chain_events` over any `BlockSource`; build a
  live source with `RpcBlockSource::connect(rpc_url, 40204)` under
  `--features chain`. Ingest the catalog first so `Touches` edges attach.
- **Daemon mode (WP-4, open):** follow-head loop + cascade policy.
- **Consumer doc with example config (WP-5, open):** citrate-core / Memrizz
  provisioning guide.

## Alternatives considered

- *Reuse `NodeKind::Tenant` for network params* — rejected: `Tenant` is
  documented as the federation meta-graph kind; overloading it muddies both.
- *Contract → event `Emits`* — rejected: EOA-only transfers would have no
  emitter, making the edge set partial (see Decision 3).
- *Mutating a checkpoint on re-org* — rejected outright: violates the
  append-only Derived plane and breaks `as_of` replay.
- *A second HTTP client in mem-ingest* — rejected: mem-sync's `rpc_call` is
  already fail-closed and audited; it is now `pub` (still `chain`-gated,
  still read-only) and reused.
