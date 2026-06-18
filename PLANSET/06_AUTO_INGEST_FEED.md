---
created: 2026-06-17T22:40:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8
status: proposed
sprint: MEM-S7-auto-ingest
---

# MEM-S7 — Live Federation Feed (secure auto-ingest)

> **Goal.** The knowledge graph stays the **canonical, never-stale truth** by
> ingesting every federation change automatically — no human or agent ever
> re-runs `backfill` to start a session. The owner's framing: an "RSS feed to the
> knowledge graph." This doc is the design + work packages; per-WP truth lives in
> the sprint file (Rule 4).

## The problem this closes

Today the graph is a **point-in-time snapshot**: `backfill` is run by hand, and a
recall's freshness banner reads `HEAD <sha> (N commits) ingested @ <ts>` — which
drifts the moment anyone pushes. On 2026-06-17 the store was 21 commits behind
`citrate-identity` and had **0 nodes for `citrate-comms`** (a repo added after the
last backfill). Stale-or-absent memory is worse than no memory: an agent acts on
contradicted information with false confidence. The fix is a **continuous,
incremental, secure** feed.

## What already exists (build on, don't rebuild)

- `mem_ingest::Ingestor::ingest(...) -> IngestReport` — derives Commit/Doc nodes +
  trailer edges for one tenant. **Content-addressed** (`source_ref`/`content` =
  identity), so re-ingesting unchanged history is a **no-op** — idempotent by
  construction.
- `mem_ingest::Watermark { repo, head, head_count, ingested_at_ms }` +
  `read_watermark` — already records "how current the Derived index is" per tenant.
  This is the cursor a change-feed needs; it makes "are we behind?" an O(1) check.
- `build_graph_gated` — authorial gating for trailer edges (SECREM-02 / FUA-MEMORIES-06).
- `mem-gateway` — already the **single-writer lock owner** of the RocksDB store and
  the OIDC-fail-closed HTTP front. It also owns a tamper-evident **audit hash-chain**.
- `mem-sync` — Belnap-CRDT federation merge + merkle anchoring (MEM-S5), for the
  multi-replica story; orthogonal to single-store freshness but shares invariants.

The **Derived = deterministic & rebuildable** invariant means auto-ingest only ever
writes Derived nodes; **Asserted** nodes (human/LLM, signed) are never created or
mutated by the feed. That keeps the core invariant intact under automation.

## Architecture — two triggers, one writer

```
          (real-time)                                  (backstop)
 GitHub org webhook  ──HMAC──▶  Webhook Receiver        Poller / Reconciler
   push / PR-merged /             (verify + allowlist     (cron: git ls-remote HEAD
    branch create-delete           + size cap + enqueue)    vs Watermark.head)
                                          │                      │
                                          ▼                      ▼
                                   Durable job queue  {repo, from_head?, to_head}
                                          │
                                          ▼
                            ┌──────────────────────────────┐
                            │  Single-writer Ingest Worker  │  ← owns the RocksDB lock
                            │  (co-located in mem-gateway)  │     (no second opener)
                            │  git fetch → Ingestor.ingest  │
                            │  → advance Watermark (atomic) │
                            │  → append to audit chain      │
                            └──────────────────────────────┘
                                          │
                              serves reads from the SAME store
                              → recall freshness is always current
```

**Why fold the worker into `mem-gateway`:** RocksDB is single-writer; the gateway
already holds the lock and serves reads. A sibling process can't open the store. So
the writer must BE the gateway (an internal ingest task + an authenticated
`POST /admin/ingest {repo}` the receiver/poller call), or the gateway must release
the lock for an external worker (worse). Co-location also means **the snapshot dance
disappears in production** — reads and writes hit one always-current store. (The
local-MCP snapshot in `scripts/mcp-stdio.sh` remains only for dev sessions that run
*alongside* the live gateway.)

### Trigger 1 — Webhook (real-time, primary)
- GitHub **org-level** webhook → `https://mem-ingest.citrate.ai/webhook` (Caddy on
  the droplet → DGX over Tailscale, mirroring `mem-gateway`/`infer`).
- Events: `push`, `pull_request` (closed+merged), `create`/`delete` (branches/tags).
- The receiver **validates only** — it never checks out or executes repo code. On
  accept it enqueues `{repo, after}` and returns 204 fast (GitHub's 10s budget).

### Trigger 2 — Poller (reconciliation, backstop)
- Cron (e.g. every 5 min): for each allowlisted tenant, `git ls-remote <repo> HEAD`
  vs stored `Watermark.head`. Divergent → enqueue. Catches dropped webhook
  deliveries, repos without a hook, and the very first ingest of a new repo.
- Cheap because the watermark makes it a HEAD compare, not a re-derive.

## Security model (the "secure" in "secure feed")

1. **Webhook authenticity** — verify `X-Hub-Signature-256` HMAC (shared secret in
   `/etc/memrizz/*.env`, same posture as `MEM_CONNECT_SECRET`) with a **constant-time**
   compare; **fail closed** when the secret is unset (mirror `kyc-routes.ts`). No
   valid signature → 401, queue untouched.
2. **Allowlist** — only `CitrateNetwork/<known-repo>` is accepted; foreign or unknown
   repos are dropped. Repo name is validated against a fixed set (no path traversal,
   no shell-interpolated names ever reach `git`).
3. **No code execution** — ingest does `git fetch` + reads commit metadata/tracked
   docs only; it never runs hooks, build scripts, or submodule fetch. Rule-7
   data-source trace per source.
4. **Bounded input** — payload size cap; malformed JSON rejected; per-repo
   **debounce/coalesce** so a push burst becomes one ingest (anti-DoS).
5. **Derived-only + authorial gating** — feed writes only Derived nodes;
   `build_graph_gated` still governs trailer edges; Asserted plane untouched.
6. **Crash-atomic watermark** — the watermark advances **only after** a successful
   ingest+flush (the TD-22 durable-batch pattern), so a crash mid-ingest re-runs the
   job and content-addressing dedups — never partial truth.
7. **Audit** — every ingest job (trigger, repo, from→to head, node/edge deltas) is
   appended to the gateway's tamper-evident audit chain.

## Work packages

| WP | Title | Deliverable |
|---|---|---|
| WP-7.1 | Incremental ingest entrypoint: `ingest_range(from_head?, to_head)` + watermark-gated; idempotency proof test (re-ingest = 0 new nodes) | D7.1 |
| WP-7.2 | Webhook receiver in `mem-gateway` (HMAC verify, allowlist, size cap, fail-closed) → enqueue | D7.2 |
| WP-7.3 | Durable job queue + single-writer ingest worker; crash-atomic watermark advance | D7.3 |
| WP-7.4 | Poller/reconciler (cron `ls-remote` vs watermark) — backstop + first-ingest of new repos | D7.4 |
| WP-7.5 | Per-repo debounce/coalesce + rate limit + audit-chain logging of every job | D7.5 |
| WP-7.6 | Adversarial + fuzz suite + TLA+ invariants (see below) | D7.6 |
| WP-7.7 | Ops: Caddy route + systemd; GitHub org-webhook provisioning runbook; HMAC secret mgmt | D7.7 |

**Exit.** A push to any federation repo is reflected in `memory.recall`'s freshness
banner within minutes with **no human action**; killing the gateway mid-ingest loses
no data and double-delivery creates no duplicate nodes; the adversarial suite is green.

## Adversarial test matrix (WP-7.6 — owner explicitly requested)

| Attack / fault | Expected behaviour |
|---|---|
| Forged webhook (bad/missing HMAC) | 401, queue + store untouched |
| Replayed delivery (same `X-GitHub-Delivery`) | idempotent; 0 new nodes (content-addressed) |
| Oversized / malformed payload | rejected at the size/JSON guard, fail-closed |
| Foreign or unknown repo in payload | dropped by allowlist; never reaches `git` |
| Repo name with shell/path metacharacters | rejected by validator; `git` never sees it |
| Force-push / history rewrite (new head ∉ descendants of watermark) | detected; full re-derive for that tenant; never panics |
| Webhook + poller race (both enqueue same repo) | single-writer + dedup → convergent, one ingest |
| Worker crash mid-ingest | job retried; watermark only advanced on success |
| Secret unset / misconfigured | endpoint fails closed (503/401), never silently accepts |

**TLA+ invariants (gate before merge):** watermark monotonicity (head_count
non-decreasing except on detected rewrite); Derived-only (no Asserted node ever
written by the feed); convergence (any trigger order → same graph for the same git
state).

## Open decisions (not blocking the plan)
- Queue backend: Redis list (gateway already optional-Redis) vs on-disk WAL.
- Webhook receiver in-process in `mem-gateway` vs a thin sibling that calls
  `/admin/ingest` (leans in-process for the single-lock reason).
- Debounce window (per-repo coalesce interval) — tune in WP-7.5.
- bge embedding throughput on push bursts — may need an embed-queue; benchmark in WP-7.1.

## Cross-repo / drift (Rules 11/12)
- GitHub org webhook config → `citrate-federation` ops + this repo's runbook.
- HMAC secret provisioning alongside `MEM_CONNECT_SECRET` / `MEM_GATEWAY_ASSERTER_SEED`.
- No new code dependency on other repos (reads their git only); a `[[drift]]` entry
  is still filed for the operational coupling to the org webhook.
