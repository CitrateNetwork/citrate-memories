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

> **Status:** the specs are authored and bound; local model-checking needs a Java
> runtime (not installed on this machine yet). The CI gate (MEM-S0 exit criterion)
> installs Temurin + tla2tools and runs `check.sh`; any invariant violation fails
> the build. Until the CI job lands, treat these as reviewed-but-not-yet-TLC-green.

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
