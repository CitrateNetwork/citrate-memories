---
created: 2026-06-10T00:00:00Z
branch: audit/secrem02-memories-write-plane
author: Fable 5 (Claude Code)
sprint: SECREM-02-followup-remediation
status: active
repo: citrate-memories
baseline_test_count: 110
---

# citrate-memories — SECREM-02 Remediation Log

> Coverage matrix: `citrate-security/planset/2026-06-10-followup-remediation.md`.
> Protocol: re-verify → red test → fail-closed fix → suite green → mutation pass.

## Phase 3 — WP 3.1 (agentic write-plane integrity)

| Finding | Sev | Red test(s) | Fix (file) | Suite (≥110?) | Mutation | Disposition |
|---|---|---|---|---|---|---|
| FUA-MEMORIES-02 | High | `mem-assert::tests::rejects_forged_trust_tier_and_plane` | Write boundary `assertable_plane_and_tier` in `verify_node`/`verify_edge`: assert path may only write the **Asserted** plane at **AgentAsserted/InferredAdvisory** tier. Rejects the post-signature `trust_tier` forgery (tier is excluded from `compute_id`) and a self-signed `Derived`-plane claim — `crates/mem-assert/src/lib.rs` | 111 ✓ | killed (drop the check → test FAIL) | **FIXED** |
| FUA-MEMORIES-03 | High | (dedicated mem-mcp harness = follow-up) | `call_merge_diff` now authorizes the repo of **every edge endpoint** (resolved from the diff's nodes, else the store) and rejects a no-op/empty diff — closing the zero-node-diff bypass that committed edges with no authz — `crates/mem-mcp/src/lib.rs` | 111 ✓ (no regression) | — | **FIXED (build-verified; test follow-up)** |

## Phase 3 — WP 3.2 (tenant read-scope) + 3.3 (index/DoS hardening)

| Finding | Sev | Red test(s) | Fix (file) | Suite | Mutation | Disposition |
|---|---|---|---|---|---|---|
| FUA-MEMORIES-01 | High | (mem-mcp harness = follow-up) | `call_neighbors` requires the resolved node to belong to the authorized repo (cross-tenant prefix reads as not-found) and drops cross-tenant neighbours — `crates/mem-mcp/src/lib.rs` | 112 ✓ | — | **FIXED (build-verified; test follow-up)** |
| FUA-MEMORIES-04 | Med | `mem-index::tests::rejects_non_finite_vector` | `check()` rejects NaN/Inf vectors before they poison cosine ranking (covers add + search) — `crates/mem-index/src/index.rs` | 112 ✓ | killed | **FIXED** |
| FUA-MEMORIES-05 | Med | (caps; harness follow-up) | `call_merge_diff` caps raw bytes (4 MiB) + node/edge counts (10k/50k) — `crates/mem-mcp/src/lib.rs` | 112 ✓ | — | **FIXED (build-verified)** |
| FUA-MEMORIES-06 | Med | `mem-ingest::tests::supersedes_trailer_from_unauthoritative_author_is_quarantined` | Authorial gating for trailer edges: authority-bearing edge kinds (`is_authority_bearing`) from unauthoritative authors are quarantined, not committed — `crates/mem-ingest/src/lib.rs` (commit `840ef94`) | ✓ | — | **FIXED** |

## Notes
- Baseline (Phase 0): **110**; mem-assert **7 → 8** (+1). Whole-repo **110 → 111**.
- TrustTier `Ord` is DerivedDeterministic < HumanConfirmed < AgentAsserted <
  InferredAdvisory, so the cap is an **allow-list** (AgentAsserted/InferredAdvisory),
  NOT a simple `≤` — a naive `≤ AgentAsserted` would wrongly admit the high-trust
  tiers.
- FUA-MEMORIES-03 lacks a dedicated red test only because mem-mcp has no test
  harness yet; the fix is build-verified and regression-free. **Follow-up:** stand
  up a mem-mcp Server test (store + scoped grant) and assert a zero-node edge-only
  diff is refused.
- Phase 3 closed for this repo: 3.1 (02/03), 3.2 (01), 3.3 (04/05/06 — 06 landed
  `840ef94` on main, 2026-06-10). 3.4 (agent-runtime) closed in its own repo
  (`5041081`). Outstanding follow-up: mem-mcp test harness for the
  build-verified-only rows (01/03/05) — fold into Phase 8.2/8.3.
- Branch: `audit/secrem02-memories-write-plane` (merged `e8d9408`); FUA-MEMORIES-06
  direct on main `840ef94`.
