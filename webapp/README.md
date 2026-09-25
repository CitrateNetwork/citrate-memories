# Memrizz — citrate-memories webapp

> The human face of the citrate-memories knowledge DAG: a 2.5D memory **constellation**
> you can fly through, ask in plain language, and steward through an HIC (Human In Control)
> trust workflow. Next.js 16 / React 19, fronting `mem-gateway`.

**Status:** WP-7.0 → WP-7.3 + WP-7.2 (BFF) complete and verified green (`pnpm test`
→ 46, `pnpm typecheck`, `pnpm lint`, `pnpm build` all pass). The on-brand design is
locked; the rest is staged in
[`../PLANSET/07_IMPLEMENTATION_AND_HARDENING_PLAN.md`](../PLANSET/07_IMPLEMENTATION_AND_HARDENING_PLAN.md).

- **WP-7.0** — Next 16 / React 19 / App Router / Tailwind 4 scaffold; design tokens
  ported 1:1 into `src/app/globals.css`; fonts self-hosted via `next/font`.
- **WP-7.1** — auth seam (`src/lib/auth/*`), nonce CSP (`src/lib/security/csp.ts` +
  `src/proxy.ts`), seam tests cloned from `citrate-explorer`. Fail-closed verified:
  forged-token rejection, OIDC issuer+audience enforcement, `sub`-as-owner, no tokens
  in web storage, script-src lockdown.
- **WP-7.3** — the app-shell chrome ported 1:1 (NavRail / TopBar / LeftRail /
  RightDock / TimeScrubber / HUD / guided overlay) using the prototype's verbatim
  `memrizz.css` + exact DOM (`src/components/`). Design fidelity enforced by a
  token-parity test (`src/lib/design/token-parity.test.ts`). The `<canvas>` is live;
  the engine that draws into it is WP-7.4.
- **WP-7.2 (BFF)** — typed gateway client + auth-gated route handlers
  (`src/lib/gateway/*`, `src/app/api/*`): the browser → BFF → `mem-gateway` read API
  (orgs/tenants/layout/recall/node/verify/neighbors). Fail-closed verified end-to-end
  (health 503 with no gateway, 401 with no session) against a hermetic loopback
  gateway. Forwards the verified OIDC bearer by default (MEM-B-013); set
  `MEM_GATEWAY_DEV_SUB=1` only for a local dev gateway to send `x-dev-sub` instead. Four-ratchet baseline in `.agentile/coverage/baseline.json`.

- **WP-7.4** — the constellation engine ported 1:1 to a typed `ConstellationEngine`
  (`src/components/constellation/`), decoupled from the prototype's globals: orbit
  camera, perspective projection, four morphing layouts, glow sprites, edge particles,
  time-morph, fly-to, hit-test, lasso. Driven by a React island that fetches
  `GET /api/orgs/[org]/layout` (deterministic SAMPLE fallback when no gateway, clearly
  labelled). Selection opens the Inspect card; hover shows the tooltip. Pure math
  (projection/layout/graph-adapter) unit-tested. The remaining M0 hardening (rate
  limiter, webapp CI + ratchet, K0 crypto-doc reconciliation, OIDC client registration
  in citrate-identity) is also done.

- **Gateway G-2 (Rust) — DONE.** `crates/mem-gateway/src/oidc.rs`: real RS256
  bearer verification against the authority JWKS (issuer+audience+expiry, RS256
  pinned), wired into `AppState`/`authn` (OIDC takes precedence, fails closed) and
  the binary (`OIDC_ISSUER`/`OIDC_AUDIENCE` + `OIDC_JWKS_JSON`|`_FILE`). 32 gateway
  tests pass, clippy clean. To use it from the webapp, leave `MEM_GATEWAY_DEV_SUB` unset so the BFF forwards the caller's bearer
  (the default) rather than the `x-dev-sub` dev header.

- **Gateway G-3 + G-1 (Rust) — DONE.** **G-3:** `ControlPlane` now persists
  atomically (`load_or_default` / `save_atomic`, temp→fsync→rename) and is loaded at
  startup (`MEM_GATEWAY_CONTROL_PATH`). **G-1:** HTTP write routes — `assert`,
  `edges/propose`, `edges/confirm` — write-scoped, signed (`mem-assert`), mutating the
  Org's isolated store, with mutations + denials recorded to a tamper-evident
  `AuditChain`. Read-only grant → 403 (audited); no signer → 503. The webapp BFF +
  POST routes (`/api/orgs/[org]/tenants/[tenant]/assert`, `/api/orgs/[org]/edges/{propose,confirm}`)
  are wired (auth-gated, rate-limited). 36 gateway tests + 63 webapp tests, clippy clean.

- **WP-7.5/7.6/7.8 + G-4 — DONE.** **7.5:** the Inspector fetches the gateway
  `verify` + `neighbors`, renders the trust verdict + blast-radius (clickable → fly-to)
  + actions, via an imperative constellation handle. **7.6:** `/api/chat` grounds every
  answer in real memories (`search`→`recall` fallback) with citations that fly to nodes
  (LLM-synthesis seam present; retrieval-grounded without a provider). **7.8:** the HIC
  Review Center derives proposals/contradictions/supersessions from the scene and
  **Confirm** calls the write-scoped, audited `edges/confirm` route. **G-4:** member
  provisioning — `members` GET/POST + DELETE (attenuation-gated onboarding, F-7
  cascade-revoke, persisted via G-3), BFF wired.

- **WP-7.9 — Admin console (DONE).** `src/components/shell/admin.tsx` surfaces the
  G-4 member routes as a live People & Delegation surface (rendered for the `settings`
  nav): the roster, an attenuation-aware **invite** form (the gateway is authoritative —
  a 403 shows as a clear toast), and a **cascade-revoke** with a `+N` preview from the
  delegation tree. Fail-closed verified (members GET/DELETE → 401 without a session).

- **Audit-log viewer (DONE).** `src/components/shell/audit.tsx` (the `audit` nav)
  surfaces the tamper-evident blake3 hash-chained audit trail the write + provisioning
  routes feed: a live **integrity verdict** (the gateway re-verifies linkage on every
  read — a broken chain shows loudly), filters (actor/event/search), and a live-tail
  poll. Gateway route `GET /api/orgs/:org/audit` is Org-scoped (admin sees all, member
  sees own) and fail-closed (401 without a session).

- **`merge_diff` over HTTP (DONE).** `POST /api/orgs/:org/merge_diff` applies a signed
  session subgraph — write-gated on every repo the diff touches (node + edge-endpoint
  repos), size-capped, signature-verified wholesale, audited, layout-cache invalidated.
  3 gateway tests (writer applies / read-only denied / empty rejected) + BFF wired.
- **BYOM connect (DONE, client + server).** Client: `src/components/shell/connect.tsx`
  mints a real short-lived (15-min) scoped JWT and renders the endpoint + copy-paste MCP
  config. **Server (G-7):** `POST /mcp/u/:sub` on `mem-gateway` — a non-streaming
  Streamable-HTTP MCP endpoint that verifies the connect token, resolves the Org +
  grant, and reuses `mem_mcp::MemoryMcpServer` so every tool call is grant-scoped and
  audited to the same chain as the HTTP routes. Verified: valid token → tools/list +
  real recall; bad-token 401 / subject-mismatch 403.

- **Org creation + store-init (DONE).** `POST /api/orgs` on `mem-gateway` validates +
  slugs a unique id, **initializes the Org's isolated encrypted store** (per-Org RocksDB
  + keyring), binds the caller as founding Owner, persists (G-3), and audits. Surfaced as
  the Admin console's "New Org" form. 3 gateway tests (init+owner / duplicate-409 /
  bad-name-400); fail-closed (401 without a session).

- **Ops / durability (WP-6.5, DONE).** `GET /api/orgs/:org/ops` (admin-gated) reports
  store node/edge counts, the rolling-checkpoint inventory (recovery points), and
  per-tenant **anchor posture** (merkle root + whether the live state still matches the
  last anchor) via `mem-sync`; `POST .../ops/checkpoint` creates a recovery checkpoint
  and prunes. On-chain anchor surfaces behind the `chain` feature. Surfaced as the Admin
  console's "Durability" tab (counts, "Checkpoint now", per-tenant in-sync/drifted).
- **Ask synthesis (LLM, streaming, DONE).** `src/lib/ai/provider.ts` is an env-driven
  OpenAI-compatible provider (`MEMRIZZ_INFERENCE_URL` / `_API_KEY` / `MEMRIZZ_MODEL`)
  pointing at the self-hosted DGX model via `citrate-inference-gateway` (local-proxy)
  behind the DO Caddy front. `/api/chat` is an AI-SDK-v6 **UI-message stream**: a
  `data-citations` part (each flies to its node) then the answer **streamed
  token-by-token** from the model — grounded ONLY in the retrieved memories — with a
  single-shot retrieval-grounded fallback (never fabricated) when no model is
  configured. The Ask panel uses `useChat`.
- **Persistent conversations — server-side DB (DONE).** Conversations live in a secure
  **Neon Postgres** DB (Drizzle: `src/lib/db/{client,schema,conversations}.ts`), **not**
  localStorage. Every row is owned by the OIDC `sub` and scoped to the Org; the
  data-access layer never queries without both, and upsert guards the owner in its
  `where` (a guessed id can't hijack a row). Owner-scoped routes
  `GET/POST /api/conversations` + `GET/DELETE /api/conversations/[id]` (all fail-closed,
  rate-limited via `withOwner`). The Ask header has **+ new** and a **history drawer**
  (resume / delete). Migration in `drizzle/`; set `DATABASE_URL` then `pnpm db:migrate`.

> **Closed: G-1 (incl. merge_diff), G-2, G-3, G-4 (member-mgmt + Org creation), G-7
> (client + server).** Every surface is live: constellation, Ask (streaming LLM or
> retrieval-grounded, with persistent history), Inspect, Review, Admin
> (people/delegation/new-org/durability), Audit, Connect — and BYOM clients drive the
> graph over MCP. Remaining: CID-pinned model selection, server-side conversation
> persistence, and a per-session connected-clients registry.

## Layout
```
webapp/
├─ design-prototype/        # the on-brand Claude-design handoff — the 1:1 SOURCE OF TRUTH
│  ├─ Memrizz.html        #   entry; React-in-browser prototype
│  ├─ constellation.js      #   the 2.5D canvas engine to port (WP-7.4)
│  ├─ colors_and_type.css   #   design tokens (lifted into src/app/globals.css)
│  ├─ memrizz.css         #   layout/chrome/panels (port to component styles)
│  ├─ *.jsx                 #   app/ui/panels/review/connect/command/settings
│  ├─ screenshots/          #   8 reference renders
│  └─ UI_IMPLEMENTATION_SPEC.md  # pixel-level spec — the acceptance checklist
├─ src/
│  ├─ app/                  # Next App Router (globals.css = ported tokens)
│  └─ lib/                  # auth seam + security (cloned from citrate-explorer)
├─ next.config.ts           # static security headers (CSP is per-request in proxy.ts)
├─ package.json             # federation house style (Next 16, jose 6, AI SDK, vitest)
└─ tsconfig.json
```

## The contract
- **1:1 visual** — port the prototype's tokens and canvas/DOM output verbatim;
  re-architect internals only. Acceptance = `design-prototype/UI_IMPLEMENTATION_SPEC.md`.
- **Hardening** — clone the `citrate-explorer` auth seam + nonce CSP + rate limiter; fail
  closed everywhere; gitleaks + four-ratchet CI. See planset §3.
- **Crypto** — match the chain's quantum-safe story (QSSP key-wrap) staged in planset §4;
  the UI surfaces plane/trust/quarantine honestly.

## Next steps (WP order)
1. **WP-7.4** — port `constellation.js` to a typed `ConstellationEngine` module and
   feed it from `GET /api/orgs/[org]/layout` (the BFF route is live).
2. **Gateway (Rust)** — G-2 real OIDC/JWKS verification + G-3 durable ControlPlane in
   `crates/mem-gateway`; the BFF already forwards the bearer by default. G-1 write
   routes (assert/propose/confirm) unblock the HIC surfaces.
3. **WP-7.1 follow-through** — register Memrizz as an OIDC client in
   `citrate-identity/src/config.ts`; add `/auth/callback` + client provider
   (`src/lib/auth/client.tsx`) for the login flow.
4. **WP-7.5/7.6** — Inspector + Ask wired to `verify`/`neighbors`/`recall`.

## Dev
```bash
pnpm install
pnpm dev         # next dev (:3000)
pnpm test        # vitest — 35 tests (the ratchet)
pnpm typecheck && pnpm lint && pnpm build
```
Copy `.env.example` → `.env.local` (defaults to `mock` auth for local dev).

See the planset for the full M0–M4 / WP breakdown and the red-test-first protocol.
