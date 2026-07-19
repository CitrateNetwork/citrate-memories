---
created: 2026-07-19T12:20:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8
status: proposed
sprint: MEM-S7-auto-ingest (extension)
---

# ADR-09 — In-flight branch layer (canonical vs. work-in-progress)

## Context

The WP-7.4 reconciler (shipped 2026-07-19) makes the graph **complete over every
repo's default branch**: nothing merged is ever missed. The owner wants more — the
graph should also reflect **work in flight on feature branches** (parallel agent
worktrees), so an agent in one repo can see what is being worked on elsewhere before
it merges — **without** polluting the trusted canonical truth. Decision (2026-07-19):
**default-branch stays canonical; branches are a separate, labeled layer.**

## The load-bearing insight

Commit nodes are **content-addressed** (`source_ref = GitCommit{repo, sha}` is
identity). A commit is therefore the **same node** whether it currently lives only on
a feature branch or has been merged to the default branch. So "canonical vs.
in-flight" is **not** a property of the commit node — it is **reachability from the
default-branch tip**, and it flips to canonical automatically when the branch merges.

This means we must **not** stamp an `in_flight` flag onto commit nodes (it would be
part of content identity, or require per-commit mutation on every merge). Instead we
express the layer **structurally**, using the graph's existing node+edge and
bitemporal/advisory machinery (the same fields `mem-sync` already treats as outside
content identity: `status`, `valid_to`, edge quarantine).

## Decision

Represent each non-default branch as a **Branch meta-node** with edges to the commits
that are unique to it; canonical/in-flight is derived from reachability, and recall
opts in to the in-flight layer explicitly.

1. **`NodeKind::Branch`** (new) — one node per `(repo, branch_name)`, carrying the
   branch tip sha and `observed_at`. Lives in the repo's tenant. It is a meta-node
   like `Tenant`/`ChainNetwork`, not a commit. Default branch gets **no** Branch node
   (it is the canonical spine; adding one would be redundant).
2. **`EdgeKind::BranchContains`** (new) — Branch node → each commit reachable from the
   branch tip but **not** from the default tip (`git rev-list default..branch`). These
   are the in-flight commits. Merged commits are reachable from default, so they carry
   no BranchContains edge and are already canonical — no dedup problem, no double
   counting.
3. **Canonical = reachable from default tip.** In-flight = has an incoming
   BranchContains edge and is not (yet) reachable from default. Recomputable from git
   at ingest; no per-commit mutable flag.
4. **Promotion on merge is free.** When a branch merges, its commits become reachable
   from default on the next default-branch ingest; the reconciler then finds the
   branch tip == an ancestor of default and **archives the Branch node**
   (`status = Archived`) and drops its now-canonical BranchContains edges. No commit
   node is ever rewritten.
5. **Recall/search default to canonical only.** A new optional arg
   `include_in_flight: bool` (default **false**) on `memory.recall` / `memory.search`
   / the gateway read routes. Off → today's behaviour exactly (trusted, merged truth).
   On → also surface Branch nodes and their BranchContains commits, clearly labeled
   `[in-flight: <branch>]` in the rendered output so a caller never confuses WIP with
   merged state.
6. **Per-branch watermark.** Extend the reconciler to `git ls-remote --heads` and
   track a `Watermark` per `(repo, branch)` (key `derived_branch_watermark:{repo}:{branch}`),
   so branch ingest is incremental and drift is a HEAD compare, same as default.
7. **Derived-only, still.** Branch and BranchContains are Derived (deterministic from
   git); the Asserted plane is untouched. Content-addressing keeps re-ingest
   idempotent.

## Work packages

**Status (2026-07-19):** B.1 done (`d5b3dc5`). B.2 done — branch ingest
(`ingest_branches`, `build_branch_graph`, per-branch watermarks, git primitives) +
the canonical-purity read filter (`tenant_nodes` excludes `Branch` nodes and Active
`BranchContains` targets). 3 tests incl. real-git end-to-end + the purity invariant.
Remaining: B.3, B.4, B.5.

| WP | Title | Deliverable |
|---|---|---|
| B.1 | `NodeKind::Branch` + `EdgeKind::BranchContains` in mem-core (additive variants, **no `SCHEMA_VERSION` bump** — see warning) + tests | in-flight vocabulary |
| B.2 | Ingest: enumerate branches, compute `default..branch` commits, emit Branch + BranchContains; per-branch watermark | branch ingest |
| B.3 | Reconciler: `ls-remote --heads` sweep; enqueue drifted branches; archive merged branch nodes | branch completeness |
| B.4 | Recall/search `include_in_flight` (default off) + `[in-flight: <branch>]` labeling; gateway routes + MCP tool schema | separated read path |
| B.5 | Adversarial/idempotency tests: merge promotes cleanly; abandoned branch archives on delete; re-ingest = 0 new; canonical recall never shows WIP | proof |

## ⚠ Do not bump `SCHEMA_VERSION`

`schema_version` is in the node **identity set** (`mem-core/src/node.rs`: the
content-id feeds `field_u16(1, self.schema_version)`). Bumping it from `1` re-hashes
the content-id of **every existing node**, orphaning the entire live graph. This
feature is **additive** — new `NodeKind::Branch` / `EdgeKind::BranchContains` variants
used only by new nodes — so existing Commit/Doc identities are untouched and
`SCHEMA_VERSION` **stays 1**. A bump is only ever warranted by a change to the
identity-bearing layout of *existing* node kinds, which this is not.

## Invariants (gate before merge)

- **Canonical purity:** with `include_in_flight=false`, no node reachable only from a
  non-default branch ever appears. (The trust guarantee.)
- **No dup on merge:** a commit that moves from in-flight to canonical is the same
  content-addressed node throughout; head_count/canonical recall are unaffected by
  whether it was previously seen on a branch.
- **Convergence:** any trigger order (webhook, reconcile, branch delete) yields the
  same graph for the same git state.

## Alternatives rejected

- **Per-commit `in_flight` advisory flag:** churns every commit on merge and risks
  leaking into content identity. Rejected for the structural (Branch-node) model.
- **Dump all branch commits into the tenant unlabeled:** simplest, but destroys the
  canonical-truth guarantee the owner explicitly wants. Rejected.

## Open tuning (not blocking)

- Branch allowlist/ignore (skip bot branches, `dependabot/*`, stale >N days).
- Whether `pull_request` webhook events should pre-empt the reconcile interval for
  faster in-flight visibility on active PRs.
