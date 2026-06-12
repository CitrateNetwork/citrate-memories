---
created: 2026-06-08T00:00:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
---

# TLA+ specifications (WP-0.5)

Formal models of the safety-critical cores of citrate-memories. See
`../PLANSET/03_TLA_SPECS.md` for the full spec plan and rationale.

| Spec | Proves | Mirrors in code |
|---|---|---|
| `SupersededDag.tla` | nodes grow-only; supersession stays **acyclic**; "latest" is well defined | `mem-store::MemoryDagStore::would_cycle_supersedes` |
| `Ingestion.tla` | cursor **monotone**; consumed range **idempotent** on replay; no dup/loss | `mem-ingest` cursor loop (MEM-S1) |
| `Authz.tla` | grants gate access; cross-tenant analogy needs **both** read grants (no leak via analogy, R3) | `mem-authz` grant check (MEM-S2) |

Each `*.tla` has a matching `*.cfg` with small finite bounds — enough to expose
ordering/cycle/expiry bugs without state-space blowup.

## Running

```bash
# needs a JRE + tla2tools.jar (set TLA_TOOLS or drop the jar here)
./check.sh
```

> **Status (2026-06-11, Lane D): TLC-GREEN.** All three specs model-check clean
> under TLC (`check.sh` exit 0). The CI gate is wired
> (`.github/workflows/ci.yml` → job `tla-model-check`: Temurin 21 + tla2tools
> 1.8.0 + `check.sh`); any invariant violation fails the build. This closes the
> MEM-S0 WP-0.5 exit criterion.
>
> **What the gate caught (the reason it exists):** on first real TLC run,
> `SupersededDag` **failed** `Acyclic`. Root cause was a bug in the spec's
> `Reach` helper — it seeded `visited = {}` and only folded in `nxt`, so the
> one-step successors (the initial frontier) were dropped from the reachable
> set; the cycle-guard then admitted a 2-cycle. **The deployed Rust code was
> correct** (`reachable_via` pushes each first-seen successor into its result),
> so this was a spec-modeling bug, not a production vulnerability — but it is
> exactly what "specs pass internally / TLC bypassed" had been hiding. Fixed by
> seeding `visited` with the frontier; `SupersededDag` also gained
> `CHECK_DEADLOCK FALSE` (the finite-node-set-exhausted terminal state is a
> legitimate stop for a safety-only spec, not a deadlock).

## Notes on the models

- **Cycle-safe reachability.** `SupersededDag` uses an iterative transitive
  closure (`ReachFrom`) that terminates on any graph, so a hypothetical cycle is
  reported as an invariant violation rather than hanging TLC.
- **Non-tautological authz.** `Authz` is operational: `readsOk`/`analogyOk` are
  history variables. `AuthorizeAnalogy` records both component reads alongside the
  analogy, so `AnalogyImpliesReads` genuinely fails if a future edit lets an
  analogy through on a single grant — it is a regression-catcher, not a
  restatement of the guard.
- **Mutation check (recommended in CI):** temporarily delete the acyclicity guard
  in `Supersede` (or a component read in `AuthorizeAnalogy`) and confirm TLC
  reports a violation — proving the invariant has teeth.
