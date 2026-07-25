---
title: MCP 2026-07-28 Readiness and Migration Plan
created: 2026-07-19
author: Saul Loveman + Claude Opus 4.8
status: living document
scope: every MCP surface owned by the Citrate federation
---

# MCP 2026-07-28 Readiness and Migration Plan

## TL;DR

The Model Context Protocol ships its largest revision since launch on **2026-07-28**
(release candidate locked 2026-05-21). It is a set of real breaking changes, not a
version bump. The headline is a **stateless protocol core**: the `initialize`
handshake and `Mcp-Session-Id` are removed, and per-request metadata replaces the
one-time session exchange.

Our exposure is concentrated in **one service**: `citrate-memories/mem-mcp`, which is a
custom Rust JSON-RPC server pinned to protocol `2024-11-05` with a stateful,
per-connection capability grant. Our two Python servers ride the `mcp` SDK and are
already close to the new model. Our HTTP front door, `mem-gateway`, already does OIDC
verification, which lines up with the new authorization direction.

Nothing we run uses the three deprecated primitives (roots, sampling, logging), so the
deprecations cost us nothing. The work is: make `mem-mcp` stateless, express the grant
as a per-request handle, adopt JSON Schema 2020-12 for tool schemas, and make the
gateway the OAuth-aligned front door for the whole team.

> A note on the source. The internal reference to "MCP-728" (from the mainbranch.dev
> write-up) is that author's shorthand for the **2026-07-28** spec release. It is not an
> official identifier and the article does not cite the spec. Its substance was
> cross-checked against the official release-candidate post and changelog (links below)
> and it holds up.

**Sources**
- Official RC post: https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/
- 2025-11-25 changelog: https://modelcontextprotocol.io/specification/2025-11-25/changelog
- Third-party summary (accurate, non-authoritative): https://mainbranch.dev/articles/mcp-728-repo-readiness/

---

## 1. What changed on 2026-07-28

### Breaking

1. **Stateless core / session removal.** The `initialize` / `initialized` handshake and
   the `Mcp-Session-Id` header are gone. Protocol version, client info, and capabilities
   move into `_meta` on **every** request. A remote server can now sit behind a plain
   round-robin load balancer and route on an `Mcp-Method` header rather than sticky
   sessions. Servers that need statefulness return an explicit **handle** from a tool
   call, which the client passes back as an argument on later calls.
2. **Server-to-client requests only mid-call.** A server may only make a request to the
   client while it is actively handling a client request. Long-lived SSE is replaced by
   an `InputRequiredResult` the client answers by re-issuing the call with echoed
   `requestState`, so any instance can pick up the retry.
3. **Tasks becomes an opt-in extension.** `tasks/list` is removed. Lifecycle is
   handle-based: a `tools/call` returns a task handle, and the client drives it with
   `tasks/get` / `tasks/update` / `tasks/cancel`.
4. **Error code standardization.** Missing-resource moves from the MCP-custom `-32002`
   to the JSON-RPC standard `-32602` (Invalid Params).

### New (opt-in unless noted)

- **Extensions framework, first-class.** Reverse-DNS identifiers, independent versioning,
  capability negotiation. Two official extensions: **MCP Apps** (sandboxed HTML UI whose
  actions route through the same JSON-RPC audit path as tool calls) and **Tasks**.
- **JSON Schema 2020-12** as the default dialect for tool `inputSchema` / `outputSchema`
  (composition, conditionals, `$ref`; external `$ref` must not auto-dereference).
- **Caching metadata** (`ttlMs`, `cacheScope`) on list and resource-read responses.
- **W3C Trace Context** (`traceparent` / `tracestate` / `baggage`) documented in `_meta`.
- **Authorization hardening** toward OAuth 2.0 / OpenID Connect: `iss` validation
  (RFC 9207), Client ID Metadata Documents, credentials bound to the issuer.
- **Formal lifecycle policy.** Active to Deprecated to Removed, minimum 12 months between
  deprecation and removal.

### Deprecated (still function for 12+ months)

- **Roots** (use tool params / resource URIs / server config)
- **Sampling** (call the LLM provider directly)
- **Logging** (stderr for stdio; OpenTelemetry for structured observability)

---

## 2. Primitive-by-primitive: what it means for us

| Primitive / area | 2026-07-28 change | Do we use it? | Our exposure |
| --- | --- | --- | --- |
| initialize handshake | removed; metadata into `_meta` per request | Yes (mem-mcp returns protocolVersion/capabilities/serverInfo) | **High** — direct refactor of mem-mcp |
| Session / `Mcp-Session-Id` | removed; stateless | mem-mcp keeps a per-connection grant (implicit session) | **High** — grant must become a per-request handle |
| Tools | JSON Schema 2020-12; unchanged call shape | Yes (11 tools in mem-mcp; ~40 across Python servers) | **Medium** — schema upgrade, no call-shape break |
| Error codes | `-32002` to `-32602` | mem-mcp already uses `-32602`/`-32601`/`-32700` correctly | **Low** — already aligned |
| Tasks | opt-in extension, handle-based | No | **Opportunity** — good fit for re-ingest / re-encrypt |
| Resources / prompts | present, cache metadata added | No | None |
| Roots / sampling / logging | deprecated | No | None |
| Authorization | OAuth2 / OIDC hardening | Gateway does OIDC; mem-mcp uses a SIWE-style grant | **Medium** — map grant onto the new pattern |
| MCP Apps | new UI extension | No (Memrizz is a separate webapp today) | **Opportunity** — future in-agent UI |

---

## 3. Our MCP inventory and per-service readiness

### 3.1 `mem-mcp` (citrate-memories) — the one that needs work

- **Stack:** Rust, custom JSON-RPC 2.0 (no SDK), newline-delimited stdio framing.
- **Transport:** stdio shim (`mcp_connect`) onto a single-writer daemon (the `mem-mcp` binary, formerly the `mcp_serve` example)
  over a Unix socket. The daemon holds the RocksDB lock and takes rolling checkpoints.
- **Protocol version:** `2024-11-05` (three revisions behind).
- **State model:** stateful. A `CapabilityGrant` is bound to the per-connection server
  instance; every tool call runs `grant.check(resource, op, now)` and appends to a
  hash-chained audit log. Resource scope is `repo:{repo}/memory` with Read/Write ops.
- **Primitives:** tools only (11): `memory.recall`, `memory.search`, `memory.neighbors`,
  `memory.as_of`, `memory.verify`, `memory.critique`, `memory.analogy` (read);
  `memory.assert`, `memory.merge_diff` (write); `memory.propose_edge`,
  `memory.confirm_edge` (edge lifecycle).
- **Already spec-aligned:** `-32602` for invalid params, `-32601` for unknown method,
  `-32700` for parse errors; notifications (no `id`) get no response.
- **Gaps vs 2026-07-28:** advertises protocol `2024-11-05`; grant lives for the
  connection instead of being re-presented per request; tool schemas are hand-built JSON,
  not declared JSON Schema 2020-12; capabilities are the minimal `{ tools: {} }`.

### 3.2 `hermes` and `hermes-tools` (testing-hermes-design)

- **Stack:** Python, `mcp` 1.26.0 (FastMCP), stdio. Tools only. Stateless per request.
- **Readiness:** the SDK absorbs most of the transport change. Action is to move to an
  SDK release that targets 2026-07-28 once it lands, and to declare JSON Schema 2020-12
  tool schemas. No custom session logic to unwind.

### 3.3 `mem-gateway` (citrate-memories) — the team front door

- **Stack:** Rust / axum HTTP, behind Caddy. OIDC id_token verification (iss + aud + exp,
  RS256, JWKS). Physical Org isolation (one store per Org). Includes an `ingest_worker`
  module behind the `server` feature.
- **Readiness:** the OIDC posture already matches the new authorization direction. This is
  where the OAuth-aligned, stateless, load-balanced remote MCP surface should live for the
  whole team. The `iss` / Client ID Metadata Document hardening items apply here.

---

## 4. `mem-mcp` migration checklist (concrete)

Target: a server that satisfies 2026-07-28 while still answering `2024-11-05` clients
through the deprecation window. Gate the new behavior behind a capability flag; do not
break the shim path that Claude Code uses today.

1. **Version negotiation.** Read protocol version and client info from `_meta` on each
   request. If a client still sends `initialize`, answer it (back-compat) and advertise
   both `2024-11-05` and `2026-07-28`.
2. **Grant as a handle.** Mint the `CapabilityGrant` into an opaque, signed handle
   returned to the client (or accepted from `_meta`), and validate it per request instead
   of holding it on the connection. Keep the hash-chained audit append exactly as is.
   This is the single change that makes the server load-balanceable.
3. **JSON Schema 2020-12.** Emit real `inputSchema` (and `outputSchema` where useful) for
   all 11 tools, `type: "object"` root, no external `$ref` auto-dereference.
4. **Caching metadata.** Add `ttlMs` / `cacheScope` to `tools/list` and read responses;
   the graph is read-mostly, so most lists are cacheable per Org.
5. **Trace context.** Thread `traceparent` from `_meta` into the audit detail so a call
   correlates from agent to graph.
6. **Tasks (optional, later).** Model `reencrypt` / `repair_store` / `backfill` as the
   Tasks extension so long operations do not block a call.
7. **Keep the safety invariants.** Fail-closed audit, signed asserts, plane/tier policy,
   and physical Org isolation are unchanged by any of the above.

---

## 5. Impact on our plan to build MCP services on-chain

The direction of the spec is favorable, not hostile, to what we are building.

- **Stateless core suits a load-balanced federation.** A per-session daemon holding a
  RocksDB lock does not scale to a whole company. The handle pattern lets us front the
  graph with N stateless workers behind the gateway, which is the topology we already
  sketched (`scripts/mcp-stdio.sh` assumes the gateway owns the live store and clients
  read a snapshot).
- **Our grant model maps cleanly onto handles.** A signed capability grant is already a
  bearer of authority. Expressing it as a per-request handle is a smaller change for us
  than for teams whose auth lived only in the session.
- **OIDC at the gateway is the right trust boundary.** The new `iss` validation and
  Client ID Metadata Document work lands on the gateway, which already verifies id_tokens.
- **On-chain anchoring is unaffected.** Merkle anchoring and chain submission
  (`AuditChain.chain_anchor`, mem-sync WP-5.2) sit below the protocol layer and do not
  change.

Net: staying stateless-first and gateway-fronted puts us **ahead** of the 2026-07-28
line rather than behind it.

---

## 6. The "stacking updates" gap (separate from the spec work)

The graph is a point-in-time snapshot. On restore (2026-07-19), the freshness banner for
`citrate-memories` read `HEAD 1253426e (53 commits) ingested`, i.e. it lags the live
repo. The continuous feed (`PLANSET/06_AUTO_INGEST_FEED.md`, sprint MEM-S7) is designed
but **status: proposed** and not running: there is no cron or launchd job, and the
gateway `ingest_worker` is not scheduled. Closing this is P3: drive `ingest_worker` on a
timer plus webhook, watermark-gated and idempotent, so the graph tracks the
citratenetwork org as pushes land.

---

## 7. Distribution to the whole team (P4)

Decision (2026-07-19): stand up the hosted **mem-gateway** (HTTPS + OIDC) as the shared
backend, and support **both** entry points against it:

- **Memrizz webapp** for browser users.
- **Local agent install** for teammates who live in a frontier-model agent, via a plain
  `.mcp.json` pointed at the hosted gateway (no local build, no local RocksDB). A
  non-engineer guide will describe installing it by asking their agent, in prose.

Until the gateway is the front door, the local single-writer daemon restored in P0 is the
interim path for the owner's own machine. Note the topology constraint: the gateway and a
local write-daemon must not open the same store at once (RocksDB single-writer); local
clients read a snapshot, the gateway owns the live store.

---

## 8. Sequencing against the 2026-07-28 date

- **P0 (done, 2026-07-19):** mem-mcp connection restored, daemon rebuilt, audit chain
  verified (91 records), end-to-end handshake and search confirmed.
- **P1 (this doc):** analysis and migration checklist.
- **P2 (done, 2026-07-19):** mem-mcp carries the stateless profile behind a compat
  flag. See below.
- **P3 (code-complete 2026-07-19):** continuous auto-ingest (MEM-S7). Webhook path
  (WP-7.2/7.3) was already wired; the WP-7.4 reconciler now sweeps every federation
  repo (`ls-remote` HEAD vs watermark) on startup + interval, so completeness no
  longer depends on webhook delivery. Deployment (env + org webhooks) is the
  remaining step; branch-scope decision open (see handoff).
- **P4:** deploy gateway, wire both entry points, ship the non-engineer guide.

### P2 as shipped (mem-mcp)

Landed in `crates/mem-mcp/src/lib.rs`, back-compat, 4 new tests, full suite 23/23
green, `clippy -D warnings` clean, gateway still compiles:

1. **Protocol-version negotiation.** `initialize` returns `2026-07-28` when the
   client asks for it, otherwise the legacy `2024-11-05`. Old clients are untouched.
2. **Stateless per-request grant.** A request may carry a signed `CapabilityGrant`
   in `_meta["ai.citrate/grant"]`; the server verifies it (fail-closed, JSON-RPC
   `-32001` on a bad grant), authorizes that one call under it, then restores the
   connection-bound grant. This is the change that makes one instance serve many
   principals behind a load balancer. The capability is advertised at `initialize`
   under `capabilities.experimental["ai.citrate/statelessGrant"]`.
3. **Distributed tracing.** A `traceparent` in `_meta` is folded into the
   tamper-evident audit detail so a call correlates from agent to graph.
4. **JSON Schema 2020-12.** Every tool `inputSchema` now declares the 2020-12
   dialect and is closed to unknown properties (`additionalProperties: false`).

Not in P2 (belongs with the gateway, P4): minting signed grant handles from OIDC,
and exposing a spec-compliant Streamable HTTP transport so remote clients need no
shim. The seam for both is now in place.

The 12-month deprecation window means `2024-11-05` clients keep working, so we are not
forced to break anything on 2026-07-28. The goal is to land P2 before the SDKs and
clients move, so we are early rather than scrambling.
