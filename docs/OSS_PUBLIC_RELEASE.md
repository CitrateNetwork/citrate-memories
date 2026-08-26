# OSS public release — curated export

**Status:** runbook (owner-executed at the PRIVATE → PUBLIC ship)
**Approach:** publish a **new, separate public repo built from an allowlist** —
NOT a scrub of this private repo. The private `citrate-memories`, its full
history, its DAG data, the webapp, plansets, and strategy all stay intact and
private. Only explicitly allowlisted paths ever reach the public tree, on a fresh
history.

## Why curated export, not history-scrub

Scrubbing the private repo's history is error-prone (a missed string leaks
forever) and still ships internal design/strategy. A curated export is
**fail-closed**: `scripts/build-public-release.sh` copies a git-tracked file only
if it matches `oss/public-allowlist.txt`. Anything not listed — `data/` (the
federation's actual knowledge), `webapp/`, `PLANSET/`, `.agentile/`, `handoffs/`,
`audits/`, `mutants.out*`, internal `docs/` — cannot leak, even if added later.

## What's public (owner decision, 2026-08-26)

- **The full engine** — all 10 `mem-*` crates (`core/store/index/query/assert/
  authz/ingest/sync/mcp/gateway`). The goal is to establish the "git for agents"
  standard ahead of the chain; the engine is the offering, not the jewel.
- `Cargo.toml`, `Cargo.lock`, `LICENSE`, `NOTICE`, `.gitignore`, `.gitleaks.toml`.
- `deploy/` — sanitized operator templates (no real infra; see PR #18).
- `scripts/mcp-stdio.sh`, `scripts/mcp-connector.py`.
- A standalone `README.md`, substituted from `oss/README.public.md` (the private
  README's PLANSET/handoff links never ship).

## What stays private

`data/` (already untracked), `webapp/`, `PLANSET/`, `.agentile/`, `handoffs/`,
`audits/`, `mutants.out*`, `specs/`, the internal `docs/` (roadmap, readiness,
this runbook), and `.github/` (write fresh public CI separately).

## How to publish

```bash
# 1. Build the curated tree (fresh history, allowlisted files only)
scripts/build-public-release.sh                 # -> ../citrate-memories-public
# The script runs a leak backstop (infra strings + gitleaks) and refuses on any hit.

# 2. Review the tree, and run the full test suite against it
cd ../citrate-memories-public
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 3. Create a NEW public GitHub repo, then push
git remote add origin git@github.com:CitrateNetwork/<public-repo>.git
git push -u origin main
```

Only after the leak scan is clean, the suite is green, and the Rule-13
PRIVATE→PUBLIC review sign-off is recorded.

## Keeping it in sync

Re-run `build-public-release.sh` whenever the engine changes; it always rebuilds
the public tree from the current allowlist + tracked files. Adding a new public
path = one line in `oss/public-allowlist.txt`. Adding a new private area needs no
action — it is excluded by default.
