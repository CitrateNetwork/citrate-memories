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
- Remaining Phase 3: 3.2 (FUA-MEMORIES-01 neighbors tenant-scope), 3.3
  (FUA-MEMORIES-04/05/06), 3.4 (agent-runtime capsule CRITICAL re-open).
- Branch: `audit/secrem02-memories-write-plane`.
