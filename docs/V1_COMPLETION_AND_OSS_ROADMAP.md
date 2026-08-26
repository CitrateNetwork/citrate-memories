# citrate-memories — v1 completion → OSS library + Agent SDK roadmap

**Created:** 2026-08-25
**Status:** proposed (draft for owner review)
**Decision context:** refresh v1 in place (not a v2 greenfield); make it the canonical
memory source of truth again; then port it three ways — internal (our own agents),
open-source library shipped **ahead of the chain** alongside the Agent SDK, and later a
Commissary product.

This document is the "what it takes to finish this version" handle. It is grounded in a
2026-08-25 reconnaissance of the deployed service, the git/manifest state, and a
three-part read of the codebase (crate completion, chain/OIDC coupling, OSS/SDK readiness).

---

## 0. Where v1 actually stands (verified 2026-08-25)

**Backend is live and healthy** (the "OIDC-blocked" handoffs are stale):
- `mem-gateway.citrate.ai/api/health` → `200 {"ok":true,"service":"mem-gateway"}`
- Gated routes fail closed: no/juck bearer → `401`
- `auth.citrate.ai` OIDC discovery + JWKS → `200`; `infer.citrate.ai` up (401 without key)

**Code maturity is high.** 10 crates, ~278 unit tests, clean layering, Apache-2.0, v0.0.1.
Zero stub debt across the tree: no `todo!`/`unimplemented!`/`unreachable!`/`FIXME`/`TODO`.
The only `panic!`s and the single `#[ignore]` are in test code.

**Default builds are network-free.** All heavy/native/coupled surfaces are behind
non-default Cargo features: `rocksdb` (mem-store), `transformer`/candle (mem-index),
`chain`/`http`/ureq (mem-sync, mem-ingest), `server`/axum (mem-gateway).

**The core runs fully standalone — zero chain, zero OIDC.** Confirmed: no crate hardcodes
an RPC URL, chainId, JWKS URL, tailnet IP, or DGX path outside tests/examples. Chain
anchoring is optional; the gateway supports OIDC / dev-auth (`x-dev-sub`) / fail-closed
no-auth. This means the OSS-ahead-of-chain story is ~90% there by architecture already.

**The public contract** (what the OSS lib and SDK expose) is stable:
- 11 MCP tools: `memory.{recall,search,neighbors,as_of,verify,critique,analogy,propose_edge,confirm_edge,assert,merge_diff}`
- Gateway REST: `GET /api/orgs/:org/{layout,recall,search,neighbors,verify,review}`,
  `POST /api/orgs/:org/assert`, `POST /mcp/u/:sub`, `POST /webhook/github`, `GET /api/health`

### Already done this session
- Fixed the broken git remote (empty-token → `gh` credential helper); push/pull work; `main` in sync.
- Reviewed + merged **PR #16** (bge offline-model dir + fresh-store BGE bootstrap — the
  embedded-client packaging). `main` now at `1f08c0b`. Tests green (29/29 mem-mcp + mem-index).

### Open access gaps (owner-side)
- **Tailnet ops path is broken from the laptop.** Even with Tailscale up, the DGX
  (the DGX (private tailnet host/IP)) is not an enrolled peer and the droplet peer
  (`citrate-rpc-1`) shows offline 86d. Production serves publicly, but store ops
  (rebuild/re-encrypt/inspect the DAG, mint the `cgk_` key, restart the gateway) can only
  run from `larrys-mac-studio` or on the boxes until the tailnet is reconciled.
- **Local MCP is unwired.** `.mcp.json` (labs root) points at a nonexistent
  `target/release/examples/mcp_connect`. The real daemon is `target/release/mem-mcp`
  (socket daemon, positional `<store> <sock>`, needs `CITRATE_MEM_STORE_KEY` for the
  encrypted store). Until rewired, our own agents are not reading/writing the DAG.

---

## Phase 0 — Reactivate as canonical (small, mostly ready)

| # | Action | Owner | Notes |
|---|---|---|---|
| 0.1 | Bump manifest pin `e8d9408` → `1f08c0b` | me | `citrate-federation/manifest.toml:381`; apply on a clean branch (repo is mid-`planset/su-sizeup`) |
| 0.2 | Rewire labs `.mcp.json` → real `mem-mcp` daemon + socket | me + owner | needs `CITRATE_MEM_STORE_KEY` (owner-held keyring seed) |
| 0.3 | Reconcile Tailscale enrollment so an ops box reaches DGX+droplet | owner | or confirm ops runs from `larrys-mac-studio` |
| 0.4 | Confirm deployed gateway is serving current `main` build | owner (tailnet) | health is green; build parity unverifiable from laptop |

## Phase 1 — Internal port (our own agents read/write the canonical DAG)

The highest-leverage port and mostly config. Outcome: every Citrate agent (and this CLI)
recalls storyline/code-shape and asserts signed memory against the live DAG.
- Stand up the local `mem-mcp` daemon over the encrypted store; fix `.mcp.json`.
- Verify the 11 `memory.*` tools resolve end-to-end from an MCP client.
- Refresh `docs/TEAM_INSTALL_GUIDE.md` for the daemon-based (not examples-based) wiring.
- Decide snapshot-vs-live read model for concurrent clients (see `scripts/mcp-stdio.sh`
  single-writer note).

## Phase 2 — Bring v1 to "complete" (close the last gaps)

- **The one stubbed surface:** `mem-gateway`'s `chain` feature ("Stubbed until the chain
  client is wired") — decide **wire it** (surface the 40204 anchor in `/ops`) **or drop it**
  from v1. It is optional ops/observability, not core.
- **Integration-test thinness:** strong unit coverage (~278) but only one real `tests/`
  integration file (mem-query). Add `#[tokio::test]` integration coverage for the gateway
  auth modes + MCP-over-HTTP happy/again fail-closed paths.
- **Genericize federation-coupled defaults → env vars** (also required for OSS, Phase 3):
  `DEFAULT_ORG="citrate-federation"` (`mem-gateway/src/main.rs:19`), webhook allowlist
  `ALLOWED_OWNER="CitrateNetwork"` (`mem-gateway/src/webhook.rs:24`), git remote default
  `git@github.com:CitrateNetwork` (`ingest_worker.rs:92`), mirror path
  a personal `$HOME` cache path (`ingest_worker.rs:119`) → `MEM_DEFAULT_ORG`,
  `MEM_ALLOWED_OWNER`, `MEM_GIT_REMOTE`, `$HOME`-based cache, with neutral defaults.

## Phase 3 — OSS extraction (public repo, shipped ahead of the chain)

**MUST-FIX before PRIVATE→PUBLIC (Rule 13 sign-off required):**
1. **Add `LICENSE` + `NOTICE`.** README/Cargo declare Apache-2.0 but no license text file exists.
2. **Reconcile copyright.** README says "© Citrate Inc."; Cargo authors say "Citrate
   Network / Citrate Inc.". Pick one entity; add consistent SPDX headers.
3. **Sanitize `deploy/` (tracked).** Leaks the DGX tailnet IP + host,
   the droplet IP, and a `$HOME` path in `Caddyfile.mem-gateway`, `mem-gateway.service`, `deploy/README.md`.
   Replace with placeholders / ship as `.example` templates.
4. **Delete/exclude the tracked handoff** `handoffs/MEM_GATEWAY_P3_P4_UNBLOCK_HANDOFF_2026-07-19.md`
   — leaks DGX host, tailnet IP, droplet topology, SSH notes. Audit the whole `handoffs/` convention.
4a. **History scrub is mandatory (not just HEAD).** Sanitizing the working tree (done in the
   OSS-prep PR) removes leaks from HEAD, but the real infra values (the DGX tailnet IP + host, the droplet IP, and a `$HOME` path) remain in prior commits. The PRIVATE→PUBLIC
   flip must therefore be a **curated/squashed export or a fresh-history public mirror**, not
   a raw history flip of this repo. Treat the git history as leaked until scrubbed.

**DONE (OSS-prep PR, infra-free slice):** LICENSE+NOTICE added; `deploy/` sanitized to
placeholder templates; defaults genericized to env-first fail-closed fallbacks
(`MEM_DEFAULT_ORG`/`MEM_DEFAULT_STORE`/`MEM_ALLOWED_OWNER`, XDG/`$HOME` mirror path);
`.gitleaks.toml` allowlist for the test RSA fixture. mem-gateway 21 tests green, clippy clean.

**SHOULD-FIX:**
5. Strip private sibling-repo path refs in `.agentile/AGENT_ENTRY.md`, README, PLANSET
   (`../citrate-federation/...`) — won't resolve for public cloners.
6. Add a `.gitleaks.toml` allowlist for the throwaway test key
   `crates/mem-gateway/src/testdata/oidc_test_rsa.pem` (`#[cfg(test)]`-only).
7. Swap demo personas off the real `@citrate.ai` domain in `webapp/design-prototype/*` → `example.com`.

**crates.io publish prep:**
8. Add `version = "0.0.1"` to every internal `mem-* = { path = ... }` dep and publish in
   dependency order: core → store/index → assert/authz → ingest → query → sync → mcp → gateway.
   (The single biggest mechanical blocker; the graph must be released together.)

**Positioning:**
9. Rewrite README to lead with the standalone pitch ("a memory server for AI agents / git
   for agents") and a self-contained quickstart (`cargo build -p mem-gateway`; run local;
   connect via `scripts/mcp-connector.py`) that assumes no federation checkout.
10. Publish the 11 `memory.*` tools + REST routes as the documented public API contract —
    this doubles as the spec the SDK memory module implements.

## Phase 4 — Agent SDK integration

- **Add a `memory` module to `citrate-sdk-js` (`@citratelabs/sdk`) first** — it is the
  canonical SDK, already has a `gateway` module + generated types and identity auth
  plumbing. Thin typed client over `/api/orgs/:org/*` + a BYOM/MCP passthrough, mapping 1:1
  to the 11 tools (reads: recall/search/neighbors/asOf/verify/review/analogy/layout;
  writes: assert/proposeEdge/confirmEdge/mergeDiff). Reuse the SDK's existing auth.
- **Mirror to `citrate-sdk-python`** (`citrate-labs-sdk`) after the JS surface stabilizes.
- **Add a memory adapter to `citrate-agent-runtime`** — it already hosts an MCP server +
  `adapters/{hermes,openclaw}` slot; memory recall/assert hooks slot in there (or a new
  `agent-memory` crate calling the Rust `mem-mcp` client directly).
- No memory/MCP client exists anywhere in the federation yet; `scripts/mcp-connector.py`
  is the reference wire contract to implement against.

## Phase 5 — Commissary product (later)

Hosted/paid tier. Net-new surface: billing (there is none today), per-tenant isolation
hardening beyond the current org scoping, catalog listing, and core-node routing. Scope
after the OSS + SDK land and teach us the multi-tenant edges.

---

## Sequencing recommendation

Phase 0 + 1 now (reactivate + internal canonical) → Phase 2 (finish v1) in parallel with
Phase 3 hardening (they overlap on genericizing defaults) → flip public + publish crates
(Phase 3) timed with the Agent SDK memory module (Phase 4) so the OSS lib and SDK ship
together, ahead of the chain. Commissary (Phase 5) follows.
