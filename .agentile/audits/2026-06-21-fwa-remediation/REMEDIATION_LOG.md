---
created: 2026-06-21T00:00:00Z
author: "Claude Opus 4.8 (1M context) — RM-MEM remediation agent"
status: complete
remediation_id: 2026-06-21-fwa-remediation
audit_id: 2026-06-20-federation-wide-audit
chunk: FWA-C10
target: citrate-memories
pinned_audit_sha: a4852d382500d3447223932e0e2534f07169748e
branch: remediation/fwa-2026-06
standard: Agentile-Audit v0.2 (red-test-first close-gate)
---

# FWA-C10 remediation log — citrate-memories federation merge path

Closes the FWA-C10 federation-wide audit findings on `mem-sync::merge_bundle`
and the gateway authorship gap. **Root-cause fix:** the federation merge path now
routes through the SAME per-repo authorization gate as the MCP `merge_diff` path
(`mem-mcp/src/lib.rs:665-695`), so the two write paths cannot diverge again. No
new write path can reach the node store without `grant.check(.., Op::Write, ..)`
plus the existing per-item Asserted signature/plane-tier verification.

## FWA-C10-01 (HIGH) — forged Derived nodes into any tenant — CLOSED-with-proof

- **Red→green test:** `mem_sync` `tests::fwa_c10_01_forged_derived_node_rejected_without_grant`
  (inverted from the audit PoC `poc_derived_plane_poisoning.rs`).
  - BEFORE (gate disabled / HEAD behaviour, captured by removing the
    `authorize_bundle(...)?` call): `Ok(MergeOutcome { nodes_added: 1, ... })` —
    the forged Derived node at `DerivedDeterministic` lands in `victim-tenant`.
    Test FAILS (asserts denial).
  - AFTER (fix in place): `Err(SyncError::Denied(_))`, `victim.node_count()`
    unchanged, forged id absent. Test PASSES.
- **Fix:** `crates/mem-sync/src/lib.rs`
  - new `fn resource_for` (L77) — maps `repo` → `repo:{repo}/memory`, identical
    to the MCP namespace.
  - new `fn endpoint_repo` (L178) — resolves an edge endpoint's owning repo from
    the bundle's own nodes, else the local store.
  - new `fn authorize_bundle` (L198) — collects every node repo + edge-endpoint
    repo and requires `grant.check(.., Op::Write, now_ms)` for each, BEFORE any
    write; fails closed (whole-bundle refusal) on any denial or unresolvable
    endpoint.
  - `pub fn merge_bundle` (L239) — signature changed to
    `(store, bundle, grant: &CapabilityGrant, now_ms: u64)`; calls
    `authorize_bundle(...)?` first.
  - new `pub fn trusted_local_grant` (L338) + `pub fn merge_bundle_trusted` (L370)
    — explicit, signed, greppable `*`-grant for in-process trusted ingest
    (self-merge / re-import / deterministic ingestor) so even the trusted path
    flows through the one gate (no second unguarded code path).
  - new `SyncError::Denied` / `SyncError::UnresolvableEndpoint` variants (L51).
  - Callers updated: `examples/sync.rs` (operator-local import → `merge_bundle_trusted`),
    `transport.rs` (`serve_one`/`handle_conn`/`pull_and_merge` now require a grant).
- **Test-count delta:** `mem-sync` tests.rs 6 → 16 (+10).
- **Mutation kill-rate:** `cargo mutants -p mem-sync --features http -f
  crates/mem-sync/src/lib.rs --re 'authorize_bundle|endpoint_repo|resource_for'`
  → **6/6 caught (100%)**.
- **Tripwire:** in-tree `tests::tripwire_merge_bundle_is_authorized_and_is_the_sole_write_entry`
  (asserts `merge_bundle` takes a grant + calls `authorize_bundle` + is the only
  sanctioned merge entry point) — verified RED when the gate is removed. Plus
  semgrep `.agentile/tripwires/mem-sync-merge-authz.yml` for CI.
- **Closing SHA:** see commit on `remediation/fwa-2026-06` (recorded in return).
- **CODE-QUALITY:** removes the two-parallel-write-path divergence the audit
  named as the root cause; one authz primitive now serves both paths.
- **DOCUMENTATION:** module + fn docs rewritten — the "Derived trusted by
  construction" claim is now scoped to in-process ingest, with the federation
  boundary explicitly gated.

## FWA-C10-02 (HIGH) — unsigned Derived Supersedes flips victim decision — CLOSED-with-proof

- **Red→green test:** `tests::fwa_c10_02_unsigned_derived_supersedes_rejected_without_grant`
  (inverted from `poc_supersession_injection.rs`).
  - BEFORE: `Ok(MergeOutcome { superseded: 1, ... })`, victim target →
    `Superseded`. Test FAILS.
  - AFTER: `Err(SyncError::Denied(_))`, victim target stays `Active`. Test PASSES.
- **Fix:** same `authorize_bundle` gate (edge endpoints are authorized, mirroring
  the FUA-MEMORIES-03 edge-endpoint authz loop on `merge_diff`). No supersession
  reaches `apply_supersession` for a repo the peer cannot write.
- **Coverage / variants added (same root cause):**
  `fwa_c10_authorized_peer_merges_normally` (positive control),
  `fwa_c10_read_only_grant_cannot_merge`, `fwa_c10_expired_grant_fails_closed`,
  `fwa_c10_mixed_repo_bundle_refused_wholesale` (no partial poison),
  `fwa_c10_edge_endpoint_repo_is_authorized`,
  `fwa_c10_unresolvable_edge_endpoint_fails_closed`,
  `fwa_c10_asserted_signature_check_still_applies_under_authz`.
- **Test-count delta:** included in mem-sync +10.
- **Mutation kill-rate:** covered by the same 6/6 authz run.
- **Tripwire:** same as C10-01.
- **CODE-QUALITY / DOCUMENTATION:** as C10-01.

## FWA-C10-03 (MED) — unauthenticated `/merge` transport — CLOSED-by-construction

- **Fix:** `crates/mem-sync/src/transport.rs` — `serve_one`, `handle_conn`,
  `pull_and_merge` now take `grant: &CapabilityGrant` and pass it to
  `merge_bundle`. Because `merge_bundle` itself now REQUIRES a grant, the
  transport cannot be wired into any binary without an explicit authorization
  decision — the type system enforces fail-closed. Module docs updated: the old
  "auth … Not yet here" note is replaced with the per-repo authz gate; mTLS /
  connect-token peer-binding + TLS-at-edge remain the documented operator/v2
  step.
- **Status rationale:** the in-process gate (authz on every merged repo) is the
  load-bearing control and is now mandatory; the network-auth/TLS binding is a
  deployment concern documented as a hard precondition. No production binary
  wires the transport at HEAD (audit-confirmed), so this is closed at the code
  layer with the residual operator step documented.
- **Test:** transport tests updated to supply a grant
  (`push_merges_into_a_peer_over_http`, `pull_fetches_a_tenant_bundle_over_http`)
  and still pass under `--features http`.
- **CODE-QUALITY:** no unauthenticated merge entry point can compile.
- **DOCUMENTATION:** transport module docstring now states the gate + the
  outstanding TLS/peer-binding precondition explicitly.

## FWA-C10-04 (MED) — authorship = shared gateway key — CLOSED-with-proof

- **Red→green test:** `mem_assert`
  `tests::fwa_c10_04_authorship_is_bound_to_principal_not_gateway`.
  - Asserts two principals get DISTINCT, deterministic authors, each ≠ the raw
    gateway author, and `blame()` names the principal. (Pre-fix, all principals
    shared `app.signing_key`'s pubkey, so this is unsatisfiable on HEAD.)
- **Fix:** `crates/mem-assert/src/lib.rs` new `Asserter::for_principal(gateway_sk,
  sub)` (L75) — derives a deterministic per-principal ed25519 sub-identity
  (domain-separated blake3 of the gateway secret + `sub`). Unique per principal
  (→ `blame()` identifies the actor), deterministic (→ content-addressing /
  idempotent re-merge preserved), unforgeable without the gateway secret (no
  per-user key escrow needed in v1). Wired in `crates/mem-gateway/src/http.rs`:
  `/assert` handler (uses the `sub` returned by `gate`) and the BYOM MCP path
  (token-verified `sub`).
- **Test-count delta:** `mem-assert` 9 → 10 (+1).
- **Mutation kill-rate:** the single `for_principal` mutant is unviable (no
  `Default` for `Asserter`); determinism + uniqueness are covered functionally by
  the new test. BLOCK-NOTE: not a numeric kill-rate — the function is a pure
  derivation with no branch to mutate meaningfully.
- **Tripwire:** the per-principal-distinctness assertion is the regression guard
  (any reversion to the shared gateway key makes the test fail).
- **Scope note:** full SIWE/OIDC-wallet-bound identity (citrate-identity) remains
  the documented v2 upgrade; this closes the attribution gap without it.
- **CODE-QUALITY:** authorship is now principal-bound at both write handlers.
- **DOCUMENTATION:** `for_principal` doc explains the derivation, its guarantees,
  and the v2 path.

## Verification summary

- `cargo build --workspace` — OK.
- `cargo test --workspace` — all green (mem-sync 18 incl. http transport;
  mem-assert 10; no regressions elsewhere).
- Test count monotone non-decreasing; no existing test weakened or deleted.
- Mutation (authz core): 6/6 caught.
- RED captured by removing `authorize_bundle(...)?` (C10-01/02 PoCs + tripwire all
  fail) then restored; GREEN after restore.
