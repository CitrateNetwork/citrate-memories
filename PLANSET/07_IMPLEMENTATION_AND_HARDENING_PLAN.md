---
created: 2026-06-14T00:00:00Z
branch: main
author: Claude Fable 5, directed by Larry Klosowski (@SaulBuilds)
status: active
sprint: MEM-S6 (WP-6.3 packaging + front-end track) → MEM-S7 (webapp build)
supersedes: none
extends: 06_WEBAPP_FRONTEND_SPEC.md
purpose: >
  The single implementation + hardening + cryptography planset for the Memrizz
  webapp. Pins the on-brand design prototype as the 1:1 source of truth, maps it to
  the real backend surface, and scopes every connection to be hardened and tested to
  the federation's SECREM bar with the chain's quantum-safe cryptography guarantees.
---

# Memrizz — Implementation, Hardening & Cryptography Plan

> **Read order.** `06_WEBAPP_FRONTEND_SPEC.md` is the *brief* that produced the
> design. This doc (`07`) is what we build from it: it reconciles the design that
> actually landed with the *real* backend, then scopes the work, the security
> hardening, the cryptographic guarantees, and the test strategy. The pixel-level
> design contract lives in `webapp/design-prototype/UI_IMPLEMENTATION_SPEC.md` and the
> prototype source itself in `webapp/design-prototype/`.

---

## 0. Executive summary

The Claude design team delivered **Memrizz** — the on-brand HTML/CSS/JS prototype of
the citrate-memories webapp. It is preserved verbatim in
[`webapp/design-prototype/`](../webapp/design-prototype/) (entry `Memrizz.html`,
engine `constellation.js`, full token system in `colors_and_type.css` +
`memrizz.css`, eight reference screenshots, and a 1,810-line extracted
implementation spec). This plan turns it into a production Next.js app wired to
`mem-gateway`, hardened to the federation's SECREM-02 bar, and brought up to the
chain's quantum-safe cryptographic guarantees.

**Three findings shape everything below:**

1. **The design landed on a 2.5D projected-canvas constellation, not WebGL/R3F.**
   `06`'s §5 speculated `react-three-fiber` + UMAP. The prototype instead renders ~900
   nodes / ~1,500 edges with a hand-rolled orbit-camera **2D-canvas** engine (baked
   additive glow sprites, depth-sorted painter's draw, particle edge-flow). We honor
   **what shipped**: port `constellation.js` faithfully to a typed canvas module. This
   is *lighter and more portable* than R3F — keep it. (R3F stays a deferred M4 option
   only if node counts blow past ~5k.)

2. **The backend the webapp can talk to is narrower than `06` assumed.** `mem-gateway`
   exposes a **read-only** HTTP/JSON API and **dev-auth only** (an `x-dev-sub` header
   behind `MEM_GATEWAY_ALLOW_DEV_AUTH=1`; it fails closed to 401 otherwise — real
   OIDC/JWKS is unbuilt). Every **write** the design needs (assert, propose_edge,
   confirm_edge, merge_diff) exists **only over `mem-mcp` JSON-RPC**, not HTTP. So
   "harden all connections" is not just front-of-house: it requires **building the
   gateway's missing write path + real authentication first** (§3). These are the
   "What's blocking the gateway?" items the prototype itself jokes about.

3. **Nothing in the memories tier is post-quantum today.** Current crypto is classical
   and good — XChaCha20-Poly1305 per-tenant crypto-shred + Ed25519-signed capability
   grants/assertions + BLAKE3 content-IDs and audit chain — but the user's requirement
   ("same quantum-resistant guarantees as the rest of the chain, end-to-end across
   squads/teams/orgs") is **only partially met**. The chain *does* have a real PQC
   codepath (QSSP: hybrid Kyber-768 + X25519 → AES-256-GCM, in `citrate-chain` storage
   at-rest). §4 scopes adopting it for memories and defines what "E2E across
   squads/teams/orgs" concretely means and costs — because today's model is
   capability-gated crypto-shredding, **not** per-recipient/group envelope encryption.

**Status of the ask, honestly:** the design research and 1:1 spec are **done**; the
planset and specs are **this document** (done); the implementation is **scoped into
WPs (§6) and scaffolded** — a pixel-perfect app + hardened gateway + PQC E2E is a
multi-sprint build, not a one-shot, and is staged accordingly.

---

## 1. The 1:1 design pin (what "follow it to the T" means)

The binding visual contract is `webapp/design-prototype/UI_IMPLEMENTATION_SPEC.md`.
Implementation rule: **port the prototype's tokens and DOM/canvas output verbatim;
re-architect only the internals** (JSX-in-browser → typed React components; plain-JS
engine → typed module; mock data → gateway data).

### 1.1 Design tokens — copy exactly, do not re-derive
- **Brand:** Citrate green `#8ecc09` (+ deep `#5a8205`, dark `#2f4502`, tint `#e8f3c6`);
  Citrate yellow `#ffbd10` (+ deep/dark/tint). Field dark evergreen `#0a1810`.
- **Dark chrome:** `--panel rgba(9,22,14,.74)`, `--ondark #eef0e6`, `--hair
  rgba(205,231,214,.12)`, `--glass-blur blur(18px) saturate(1.1)`.
- **Material lanes (node color = kind):** Code `#8ecc09` · Docs `#ffc83a` · Specs
  `#5fa8e6` · Configs `#34c7b0` · Tests/Audit `#f0743a` · Claims `#b58cff`.
- **Trust rings:** Derived `#cfe9b0` · Confirmed `#ffd24a` · Asserted `#9fc0e8` ·
  Proposed `#b58cff`.
- **Type:** Display = *Source Serif 4* (variable, `opsz`); body = *Geist*; mono =
  *Geist Mono*. Full scale `--t-3xs 11px` … `--t-7xl 112px`; tracking/line-heights per
  spec §1.2. **Self-host these fonts** (do not hit Google Fonts at runtime — CSP +
  privacy; see §3.4).
- **Radii** `0/6/8/12/999`; **spacing** 8pt grid; **motion** ease/dur tokens per §1.6.
- **Action:** lift `colors_and_type.css` into `app/globals.css` (CSS vars) +
  `tailwind.config` theme extension so both Tailwind utilities and raw vars resolve to
  the identical values. The prototype's `memrizz.css` layout/chrome rules port to
  CSS modules / component styles.

### 1.2 Layout & components — the full shell (port 1:1)
Fixed `100vh`, no page scroll. Z-index ladder per spec §8. Regions, with exact dims:
NavRail (60px) · TopBar (56px) · LeftRail "Constellation" (268px / 52px collapsed) ·
Canvas (fill) · RightDock "Ask/Inspect" (392px) · TimeScrubber (bottom-center) ·
Minimap (132px) · SelectionHUD (lasso) · HUD (stats) · route surfaces (Review,
Connect, Audit, Settings, Profile) · CommandPalette (⌘K) · MemoryPicker · toasts ·
guided overlay. Component inventory and per-component pixel specs: spec §2–3 (treat as
the acceptance checklist).

### 1.3 The constellation engine — port `constellation.js` faithfully
2.5D orbit camera (`yaw .5, pitch -.32, dist 15.2, zoom 1`), perspective projection
(`_project`), four morphing layouts with the exact constants — Lattice (`LG 2.55`,
`TIME_H 6.4`, `TR_G 1.7`), Galaxy (lane×tenant cluster centers), Islands (14 orbital
centers `rad 6.6`), River (sinusoidal, time-as-Z). Baked glow sprites (`_glow`),
depth-sorted additive draw, edge styles table (spec §4.2), particle flow, time-morph
(`_effStatus`), hit-test/hover/lasso, fly-to easing `cubic-bezier(.2,0,0,1)` 1.0s,
auto-rotate `tyaw += dt*.045`, DPR-capped at 2, `prefers-reduced-motion` honored.
Performance budget: 60fps on M-series; bloom-emitter cap 36–150 by quality tier;
rAF + setInterval watchdog. Implement as a framework-agnostic `ConstellationEngine`
class driven by a thin React island, so it never blocks first paint.

### 1.4 Data-shape reconciliation (design mock → real ontology → gateway JSON)
The prototype's node/edge shapes map cleanly onto `mem-core` and the gateway:

| Prototype field | Real source | Notes |
|---|---|---|
| `kind` (Commit, Adr, Finding, …) | `mem-core::NodeKind` | identical enum; design has all 19 kinds |
| `lane` (6) | derived from kind | the KINDS→lane map (spec §6.1) **is** the canonical taxonomy — encode it once, server-side |
| `plane` Derived/Asserted | `MemoryNode.plane` | 1:1 |
| `trust` (4 tiers) | `MemoryNode.trust_tier` | 1:1 (`DerivedDeterministic`…`InferredAdvisory`) |
| `status` | `MemoryNode.status` | 1:1 |
| `tenant` | `repo` (engine tenant) | gateway `/orgs/:org/tenants/:tenant/...` |
| `hash` `ctr:7f3a9c2e` | `ContentHash` prefix | gateway node id prefix; `verify` resolves |
| `belnap` | `confidence: Vec<BelnapValue>` == Both | contradiction flag |
| `deg`/`sizeW` | edge count | gateway `layout` endpoint computes |
| `pos/tpos` (x,y,z) | **gateway `layout` endpoint** | server computes scene; client morphs |
| edge `kind`/`quarantined` | `EdgeKind` + quarantine flag | 1:1 |

**Decision:** the 3D positions are computed **server-side** by the gateway `layout`
route (already exists, returns a constellation scene) and the four layout *modes* are
computed **client-side** from each node's `(laneIdx, tf, trustIdx, tenantIdx, semantic
jitter)` — exactly as the prototype does — so mode switches are instant and need no
round-trip. The gateway supplies the semantic (bge) jitter seeds for Galaxy mode.

---

## 2. Architecture — three runtimes, one hardened seam

Per `06` §2, unchanged and correct:

```
Browser (OIDC RP) ──auth'd HTTP/JSON + SSE──▶ mem-gateway (Rust/axum) ──in-proc──▶ engine (10 crates, RocksDB)
   Next.js app                                  • Org-resolve → CapabilityGrant
   • constellation (2.5D canvas)                • read API (recall/search/as_of/node/
   • Ask (AI SDK RAG)                             verify/neighbors/analogy/layout)
   • HIC review / inspect                       • WRITE API (assert/propose/confirm/    ← BUILD (§3)
   • admin / audit / connect                      merge_diff)  [today: MCP-only]
                                                • SSE deltas + audit tail              ← BUILD
                                                • MCP-over-HTTP (BYOM)                  ← BUILD (M2)
```

The Next.js **BFF (route handlers / server actions) is the only thing that talks to
`mem-gateway`**; the browser never holds a gateway credential. The gateway is the
single authz+audit chokepoint (Org boundary first, then `CapabilityGrant` scope, fail
closed). This mirrors how `citrate-explorer` fronts its off-Vercel indexer.

---

## 3. Connection hardening plan (to the federation SECREM bar)

Two halves: **(A) close the gateway's backend gaps** so the connections the design
needs exist at all, and **(B) apply the federation house-style hardening** to every
connection. Both follow the **SECREM-01 red-test-first WP protocol**: a connection WP
is *done* only when — (1) re-verified, (2) an adversarial test failed first against the
unpatched path, (3) the fix fails closed, (4) test count ≥ baseline (the ratchet), (5)
surviving validation-weakening mutants killed.

### 3.A Backend gaps to close (these ARE "what's blocking the gateway")

| ID | Gap (verified in code) | Fix | Milestone |
|---|---|---|---|
| **G-1** | ~~HTTP API is **read-only**.~~ **DONE 2026-06-14.** | Implemented POST routes in `mem-gateway/src/http.rs`: `assert`, `edges/propose`, `edges/confirm` — each write-scoped (`Op::Write`, Org-boundary first), signed via `AppState.asserter` (mem-assert), mutating the Org's isolated store, with mutations **and denials** recorded to a tamper-evident `AuditChain`. No signer → 503; read-only grant → 403 (audited). Webapp BFF + POST route handlers wired (auth-gated, rate-limited). 3 gateway tests (write-scoped success / read-only denied / no-signer 503) + 1 BFF test. *(merge_diff follows the same pattern — deferred.)* | M1 ✓ |
| **G-2** | ~~**No real authentication.**~~ **DONE 2026-06-14.** | Implemented: `crate::oidc::OidcVerifier` verifies RS256 bearer JWTs against the authority JWKS (iss+aud+exp, RS256 pinned vs alg-confusion); wired into `AppState.with_oidc` + `authn` (OIDC takes precedence, dev-auth ignored when set, fails closed). Bin reads `OIDC_ISSUER`/`OIDC_AUDIENCE` + `OIDC_JWKS_JSON`\|`_FILE` and refuses to start fail-open. 7 tests (positive + wrong-aud/iss/expired/garbage/unknown-kid/bad-jwks + authn precedence). BFF flips on via `MEM_GATEWAY_FORWARD_BEARER=1`. | M0 ✓ |
| **G-3** | ~~`ControlPlane` is **in-memory JSON**.~~ **DONE 2026-06-14.** | Implemented durable atomic persistence on `ControlPlane`: `load_or_default(path)` (fresh on first boot, LOUD on corrupt) + `save_atomic(path)` (temp→fsync→rename, so a crash mid-write never leaves a half-written plane). Bin loads from `MEM_GATEWAY_CONTROL_PATH` at startup and persists after provisioning; admin mutations re-save through the same path. 1 round-trip+atomicity test. *(Backend stays JSON-on-disk; swap to RocksDB CF / Postgres behind the same two methods when scale needs it.)* | M0 ✓ |
| **G-4** | ~~**No multi-Org provisioning.**~~ **DONE 2026-06-14 (member mgmt + Org creation).** | Delegation-tree routes: `GET/POST /api/orgs/:org/members` + `DELETE /api/orgs/:org/members/:sub` — attenuation-gated onboarding (`can_delegate`), `parent`-recorded, persisted (G-3), F-7 cascade-revoke, audited. **Org creation + store-init:** `POST /api/orgs` validates+slugs a unique id, **initializes the Org's isolated encrypted store** (per-Org RocksDB + keyring via `open_org`, `#[cfg(rocksdb)]`-gated so it's testable without it), binds the caller as founding Owner, persists, audits. Webapp BFF + Admin-console "New Org" form wired. 7 gateway tests (member: onboard/denied/attenuation/cascade; org: init+owner / duplicate-409 / bad-name-400). | M2 ✓ |
| **G-5** | **F-5** principals are bare Ed25519 pubkeys, not bound to OIDC `sub`/wallet; **F-7** delegation-revocation cascade exists in `org.rs::revoke_cascade` at engine level but isn't wired to identity. | Bind `CapabilityGrant.recipient` to the real OIDC `sub` (F-5); expose cascade-revoke through admin API with a preview (F-7). | M1 (F-5) / M2 (F-7) |
| **G-6** | **No SSE**: no live graph deltas / audit tail. | Add `text/event-stream` endpoints (graph deltas on watermark change, audit append feed), Org-scoped. | M1 (audit), M2 (graph) |
| **G-7** | ~~**No MCP-over-HTTP / BYOM bridge.**~~ **DONE 2026-06-14 (client + server).** | **Client:** `POST /api/orgs/:org/connect/token` mints a short-lived (15-min) scoped JWT (HS256, `MEM_CONNECT_SECRET`) reflecting the caller's readable tenants; the Connect page renders the endpoint + copy-paste config (Claude Desktop/Code/Cursor/generic). **Server:** `POST /mcp/u/:sub` on `mem-gateway` — a non-streaming Streamable-HTTP MCP endpoint that verifies the connect token, resolves the Org + capability grant, and **reuses `mem_mcp::MemoryMcpServer`** so every tool call (recall/search/neighbors/verify/as_of/analogy/critique + assert/propose/confirm/merge_diff) is grant-scoped and written to the SAME audit chain as the HTTP routes. `Asserter` made `Clone` to hand the shared identity to a per-request session. 2 gateway tests (valid token → tools/list + real recall; bad-token 401 / subject-mismatch 403). **Follow-up:** a per-session connected-clients registry + per-client revoke. | M2 ✓ |

Each backend gap gets a TLA+ obligation where it touches authz/write invariants (§5.4)
and folds into the MEM-S6 WP-6.1 Tier-1 audit scope (citrate-security issue #6).

### 3.B House-style hardening — clone, don't reinvent
The federation has no shared package; the patterns are **copy-pasted per repo** from
`citrate-explorer` (the reference RP). Clone these into the Memrizz app, **at current
versions** (jose 6, not the drifted jose 4):

1. **Auth seam (the load-bearing pattern).** Clone `citrate-explorer/src/lib/auth/*`
   verbatim: `session.ts` (the *only* identity check routes call), `config.ts`,
   `cookies.ts`, `discovery.ts`, `types.ts`, `client.tsx`, `adapters/privy.server.ts`,
   **and the tests** `session.oidc.test.ts`, `session.web1.test.ts`, `session.sr0.test.ts`,
   `token-storage.guard.test.ts`. Non-negotiables it encodes:
   - `resolveServerAuthMode`: **unset/unknown auth mode → `oidc`** (reject everything
     until JWKS configured) — never fail open (the WEB-1 fix).
   - `verifyOidc`: `jwtVerify` against `createRemoteJWKSet(OIDC_JWKS_URL)` requiring
     **both** `OIDC_ISSUER` and `OIDC_AUDIENCE`; if either unset, refuse all tokens
     (FUA-EXPLORER-01). Never pass `undefined` to verify. Assert issuer is `https://`.
   - Token from **Authorization bearer OR httpOnly SameSite=Strict cookie** — never
     `localStorage` (FUA-EXPLORER-04). Cookie is transport, not trust.
   - **Owner key = OIDC `sub`, verbatim, case-sensitive** (NOT the wallet) so
     email/passkey identities are first-class (SR-0). `requireOwner(req)` is the
     one-call gate; null → 401.
   - Privy path **dynamically imported** so no Privy code is referenced unless selected.
   - **Register Memrizz as a new OIDC client** in `citrate-identity/src/config.ts`
     (trusted first-party, auto-consent). Discover endpoints via
     `/.well-known/openid-configuration` (panva's authorize is `/auth`, JWKS `/jwks`,
     userinfo `/me`) — never hardcode.
   - **Org resolution lives in Memrizz**, not citrate-identity: `sub` → Org(s)+role
     mapping in the gateway control-plane; a `sub` with no membership lands on
     "request access / create an Org", never on data.
2. **CSP + headers.** Clone `src/proxy.ts` per-request **nonce CSP** + `lib/security/csp.ts`:
   `script-src 'self' 'nonce-…' 'strict-dynamic'` (no `unsafe-inline`), `object-src
   'none'`, `frame-ancestors 'none'`, `base-uri 'self'`, `upgrade-insecure-requests`;
   `connect-src` an **env-derived allowlist** (the gateway origin + OIDC issuer + SSE,
   Privy only when configured). Plus static headers in `next.config.ts`: HSTS preload,
   `nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: strict-origin-when-cross-origin`,
   `Permissions-Policy`, `poweredByHeader:false`. (Documented exception:
   `style-src 'unsafe-inline'`.) **Self-host fonts** so no `fonts.googleapis.com` is
   needed in `connect-src`/`font-src`.
3. **Rate limiting.** Clone the Upstash-REST + in-memory-fallback two-tier limiter;
   **key on the platform-trusted right-most XFF hop** (`x-vercel-forwarded-for`), never
   left-most (FUA-EXPLORER-02). Apply to all BFF routes; add **per-account budget caps**
   on the BYOM/MCP endpoint and on `search`/`recall` budgets (the "sponsored-resource
   drain" class, WEB-2/3, FUA-GATEWAY-01).
4. **Secret custody.** All secrets server-only `process.env`; **throw on unset**
   (`assertProductionConfig` style — do NOT default to `""` like explorer's
   `API_KEY_PEPPER`, HYG-DRIFT). Only `NEXT_PUBLIC_*` reaches the client (gateway
   origin, OIDC issuer/client-id/scope). No gateway credential in the browser.
5. **No gasless writes** — Memrizz does not transact on-chain (chain-anchor read is
   the gateway's job), so **no EIP-2771 relayer route** is needed. If a future "anchor
   now" action transacts, it goes through a server-only relayer with per-from+IP+daily
   caps (the WEB-2 kit) — flagged, not built.
6. **Fail closed, visibly, everywhere.** Every auth/scope/budget decision defaults
   denied; a denied action says so plainly and is **audited**. This is the dominant
   federation finding class ("fail-open on missing env/config") — pre-empt it.
7. **CI gates (clone all).** `ci.yml` (pnpm `--frozen-lockfile` → typecheck → lint →
   test → build, hermetic), `ratchet-check.yml` (four ratchets vs
   `.agentile/coverage/baseline.json`), root `secret-scan.yml` (gitleaks, full history)
   + `.gitleaks.toml`, the Semgrep tripwires in `scripts/semgrep/*` (incl.
   default-mock-auth and threshold-without-consistency). Pin third-party actions to
   commit SHA (SECREM-02 Phase 0.6).

### 3.4 Per-connection hardening matrix (the acceptance surface)
Every connection the app makes, and its required controls:

| Connection | AuthN | AuthZ | Rate/budget | Audit | Fail-closed test |
|---|---|---|---|---|---|
| Browser → BFF (read) | OIDC cookie/bearer | `requireOwner` | IP limiter | n/a (gateway audits) | unset OIDC → 401 |
| BFF → gateway read routes | service identity | Org boundary → grant scope | budget caps | gateway audit chain | missing grant → 403 |
| BFF → gateway **write** (assert/confirm) | OIDC sub bound (F-5) | **write** scope + signer | per-account cap | every mutation audited | read-only grant → 403 |
| Browser → SSE (audit/graph tail) | cookie | Org scope | connection cap | tail is the audit itself | cross-org → no events |
| BYOM MCP-over-HTTP | short-lived token from grant | == user's grant, per-tool | per-client cap + revoke | every tool call | expired token → 401 |
| Crypto-shred (forget) | OIDC sub, Org-Owner | Maintainer + type-to-confirm | n/a | double-audited | non-owner → 403 |

---

## 4. Cryptography & privacy guarantees plan

> The user's bar: **"the same quantum-resistant cryptography and end-to-end security
> across squads, teams, and organizations that the rest of the chain has."** This
> section states honestly where we are, what the chain actually provides, and the
> concrete roadmap to meet the bar — because today's memories model is **classical and
> capability-gated, not post-quantum and not per-recipient E2E.**

### 4.1 Ground truth — current crypto (all classical)
- **At rest:** per-tenant **XChaCha20-Poly1305** AEAD crypto-shred
  (`mem-store/src/shred.rs`). Master key per tenant from OS RNG; cipher+nonce keys via
  `blake3::derive_key`; deterministic SIV nonce `keyed-blake3(node_id‖plaintext)[..24]`
  (so Derived rebuilds stay byte-identical); AAD binds `tenant‖node_id`. **Node
  payloads sealed; structure (ids, edges, repo, watermark) is cleartext** so the DAG
  federates. Forget a tenant = destroy its DEK.
- **Signatures:** **Ed25519** (`ed25519-dalek` v2) on capability grants
  (`mem-authz/grant.rs`), asserted nodes/edges/diffs (`mem-assert`), gateway grant
  minting.
- **Identity / integrity:** **BLAKE3** content-addressed node IDs, the persistent
  hash-chained audit log, merkle `tenant_root`.
- **Cross-tenant sharing:** **grant intersection** (signed Ed25519 `CapabilityGrant`),
  **not key sharing** — there is no KEM-DEM-per-recipient, no group envelope, no proxy
  re-encryption.
- **Org isolation:** **physical** — one RocksDB store + keyring per Org; cross-Org is
  refused at the gateway boundary before any store access. (Verified by
  `engines_are_physically_isolated_per_org`.)
- **Spec/code drift to fix:** `02_ARCHITECTURE.md` says AES-256-GCM; code uses
  XChaCha20-Poly1305 (a deliberate, documented choice). Reconcile the doc.

### 4.2 What the chain provides that we must match — QSSP
`citrate-chain/core/storage/src/crypto/quantum_safe.rs` implements **QSSP**: hybrid
**Kyber-768 (ML-KEM) + X25519** KEM → **AES-256-GCM** DEM, domain-separated SHA3-512
combine with a key-commitment check, **versioned envelope headers for crypto-agility**.
This is real, shipped, and defends "Harvest-Now-Decrypt-Later." Chain *signatures* stay
classical (Ed25519 / secp256k1); Dilithium3/SPHINCS+ exist only as `Future:` enum
variants — so **PQ signatures are aspirational federation-wide**, and we match the
chain by **not** over-promising PQ signatures, while **adopting QSSP for confidentiality**.

### 4.3 The target model — "quantum-resistant E2E across squads/teams/orgs"
Decompose the requirement into four concrete, independently-shippable guarantees:

- **C1 — Quantum-safe at rest (confidentiality):** wrap each tenant/Org DEK under
  **QSSP (Kyber-768 + X25519 → AES-256-GCM)** instead of leaving DEKs in a local
  keyring. Reuse the chain's `quantum_safe.rs` envelope crate directly (it is already a
  workspace sibling). Payload AEAD can stay XChaCha20-Poly1305 (fast, deterministic
  nonce for Derived rebuild) — **the PQ boundary is the key-wrap**, exactly as the
  chain layers it. *Outcome: a harvested store is not decryptable by a quantum adversary
  without the PQ-wrapped DEK.* **This is the headline "same as the chain" guarantee.**
- **C2 — Group/recipient E2E for squads/teams/orgs:** introduce a **key hierarchy** —
  Org root → team → squad → repo-tenant DEK — and **envelope each level's key to its
  members' public keys** via HPKE-style hybrid (Kyber+X25519) KEM. Membership change =
  re-wrap that level's key to the new member set (lazy, on next write). **Delegation =
  re-encrypt the child key to the delegatee** (proxy-re-encryption or simple re-wrap on
  grant). This is what turns "capability-gated read" into **true E2E**: the gateway can
  route ciphertext it cannot read for content it isn't a member of. *Decision needed
  (`⟦DECIDE⟧`): how blind is the gateway?* Two tiers:
  - **Tier-E (server-mediated, default):** gateway holds Org-level keys, enforces
    squad/team scoping by re-wrapping — strong isolation, gateway is in the TCB.
  - **Tier-Z (client-side E2E, opt-in):** sensitive repos encrypted client-side; the
    gateway only ever sees ciphertext + structure; search over those repos degrades to
    structural/metadata only (no server-side bge over plaintext). Reserved for the
    highest-sensitivity tenants.
- **C3 — PQ-ready signatures (authenticity, crypto-agile):** keep Ed25519 as the
  load-bearing signer **today** (matches the chain) but adopt the chain's **versioned
  envelope** so assertions/grants carry an algorithm tag. Define a **hybrid signature**
  slot (Ed25519 **+** ML-DSA/Dilithium) behind a feature, shipped when the chain ships
  PQ signatures — no flag day, just a version bump. F-5 (identity-bound principal) is
  the prerequisite: bind the signing key to the OIDC `sub`/wallet.
- **C4 — Org/tenant isolation & crypto-shred (already strong):** keep physical per-Org
  store separation + per-Org keyring; C1/C2 make "forget" a **PQ-wrapped-key
  destruction** so a forgotten tenant is unrecoverable even by a future quantum
  adversary holding the harvested ciphertext.

### 4.4 Crypto roadmap (staged, with the honest cost)
| Phase | Guarantee | Work | Risk/cost |
|---|---|---|---|
| **K0 (now)** | Reconcile spec↔code; document the *real* model in `02`. | Doc + `04` BDD for shred round-trip. | low |
| **K1 (M1)** | **C1**: QSSP-wrap per-Org/tenant DEKs (reuse chain crate). | New `mem-store` keywrap path + migration; envelope versioning. | med — key-mgmt migration; needs HSM/KMS custody decision (`⟦DECIDE⟧`). |
| **K2 (M2)** | **C3**: crypto-agile signature envelope + F-5 binding. | `mem-assert`/`mem-authz` version tags; identity binding. | med |
| **K3 (M3)** | **C2 Tier-E**: squad/team/org key hierarchy + re-wrap on membership/delegation. | Key-hierarchy crate; cascade re-wrap = F-7 sibling. | **high** — the real E2E lift; needs threat model + audit. |
| **K4 (M4)** | **C2 Tier-Z** (opt-in client-side E2E) + hybrid PQ signatures when chain ships them. | Client crypto, search-degradation UX, Dilithium feature. | high — UX + perf tradeoffs. |

**Bottom line for the user:** we can deliver **C1 (quantum-safe at rest, identical
primitive to the chain)** and **C4 (PQ crypto-shred)** in M1–M2 — that is the literal
"same quantum-resistant cryptography as the chain." Full **E2E across squads/teams/orgs
(C2)** is a real cryptosystem build (K3/K4), correctly staged, not hand-waved. The UI
already encodes the trust model honestly (plane/trust/quarantine), so it will *show*
these guarantees as they land.

---

## 5. Test & verification strategy

### 5.1 The four ratchets (never decrease — a drop is a BLOCKER)
Tests · TLA+ specs · Semgrep tripwires · Rule-12 frontmatter coverage. Seed
`webapp/.agentile/coverage/baseline.json` at first green CI; gate with
`ratchet-check.yml`. Canonical count: `pnpm vitest run --reporter=json | jq '.numTotalTests'`.

### 5.2 Connection test matrix (red-test-first — every row fails first against unpatched)
For each connection in §3.4: an adversarial test that the **unpatched** path is
exploitable (forged token accepted, cross-org leak, read-only grant writing, unbounded
budget, expired BYOM token working), then the fail-closed fix, then prove no regression.
Mirror explorer's `session.web1.test.ts`/`session.sr0.test.ts` shape for the seam.

### 5.3 1:1 visual verification (the "to the T" gate)
- **Token parity test:** assert every CSS var equals the prototype's value (parse
  `colors_and_type.css`, diff against the app's computed vars) — fails if a hue drifts.
- **Component/structure parity:** Playwright DOM + computed-style snapshots of each
  shell region (NavRail/TopBar/LeftRail/RightDock/Scrubber) at the spec's exact dims.
- **Constellation behavior:** unit-test `_project`, each layout's target positions vs
  the prototype's constants, `_effStatus` time-morph, hit-test radius, fly-to easing.
- **Perf gate:** headless fps probe ≥ 60fps at the synthetic 900-node graph; auto-Lite
  trigger test.
- **a11y:** `prefers-reduced-motion` disables particles/auto-rotate; keyboard reaches
  every route; ⌘K palette nav; AA contrast assertions on the token pairs (spec §8).

### 5.4 Formal (TLA+) — extend existing specs
The repo already has `Authz.tla`, `Ingestion.tla`, `SupersededDag.tla`. Add obligations
for the new write path: **(a)** no write succeeds without a write-scoped grant + valid
signature; **(b)** Org boundary holds before scope check; **(c)** delegation attenuation
(child scopes ⊆ parent) and **revocation cascade** (F-7) removes the whole subtree;
**(d)** crypto-shred is irreversible and Org-scoped. Each maps to a `04_FEATURES_BDD`
Gherkin feature and a vitest/Rust test.

### 5.5 E2E (Playwright) — the three core journeys
**See** (load → constellation renders → filter → inspect a node → verify verdict),
**Ask** (question → grounded answer → click citation → fly-to + Inspect), **Steward**
(open Review → confirm a proposal → audit toast with seq# → audit log shows it). Plus
the fail-closed journeys (no-membership user, denied tenant, stale watermark banner,
offline gateway).

---

## 6. Sprint / work-package breakdown

Aligned to `06` §13 phasing (M0–M4); each WP carries its tests + ratchet impact +
crypto/hardening obligations. **Gateway backend WPs (G-*) gate the UI WPs that need
them.**

### MEM-S7 · M0 — Backend seam + app skeleton (no hero UI yet)
- **WP-7.0** Scaffold Next 16 / React 19 / App Router / pnpm / `src/` / Tailwind 4 /
  Vitest; port design tokens 1:1 (`globals.css` + tailwind theme); self-host fonts.
- **WP-7.1** Clone auth seam + CSP + rate limiter + CI gates (§3.B); register OIDC
  client in citrate-identity. **Tests:** the four seam tests pass; CSP guard test;
  unset-config → 401 red test.
- **WP-7.2 (G-2,G-3)** Gateway: real OIDC/JWKS verification; durable ControlPlane.
- **WP-7.3** Token parity + shell skeleton (NavRail/TopBar/LeftRail/RightDock empty
  states) — proves 1:1 chrome before the engine.
- **K0** crypto-doc reconciliation.

### M1 — See + Ask + Steward (the demoable MVP)
- **WP-7.4** Constellation engine port (all 4 layouts, glow/edges/particles, camera,
  lasso, time-morph, minimap, HUD) wired to gateway `layout` + filters. Perf + behavior
  tests (§5.3).
- **WP-7.5** Inspect tab: node detail, plane/trust/status, verify verdict, neighbors,
  blast-radius, contradiction card — wired to `node`/`verify`/`neighbors`.
- **WP-7.6** Ask tab: AI SDK RAG with model picker, recall/search/neighbors/verify/
  as_of/analogy/critique as tools, citations↔graph fly-to, show-your-work, completeness
  heads-up, freshness banner.
- **WP-7.7 (G-1,G-5,G-6)** Gateway **write routes** (assert, propose, confirm) bound to
  OIDC `sub` (F-5); audit-tail SSE. Red-test-first per row.
- **WP-7.8** HIC Review Center (proposals/contradictions/supersession/critic) + Add-a-
  memory + time-scrubber as_of + audit toast with seq#.
- **K1** crypto **C1**: QSSP DEK-wrap.

### M2 — Operate
- **WP-7.9 (G-4)** Admin console: people/roles, delegation tree, scope picker
  (attenuation-aware), Org provisioning. **(G-7)** BYOM MCP-over-HTTP connect page +
  per-client tokens/revoke + budget caps.
- **WP-7.10** Audit viewer (integrity badge, live tail, filters), Ops (checkpoints/
  recovery, CRDT sync, chain-anchor status), memory-diff import/export with DiffPreview.
- **K2** crypto **C3** + **F-7** cascade-revoke in admin.

### M3 — Platform + Forget
- **WP-7.11** Platform-Operator console (Org lifecycle, no memory content); crypto-shred
  high-friction flow; Org billing/quota seams.
- **K3** crypto **C2 Tier-E** (squad/team/org key hierarchy + re-wrap).

### M4 — Polish + OSS + audit
- **WP-7.12** Federation Islands + Storyline River polish, full shader pass, a11y
  hardening, Lite-mode tiling; Docker bundle = OSS artifact; **Tier-1 audit** (MEM-S6
  WP-6.1) over gateway + BYOM + crypto; OSS visibility flip.
- **K4** crypto **C2 Tier-Z** + hybrid PQ signatures when chain ships them.

---

## 7. Risk register
| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Treating "harden connections" as front-end-only and missing that writes/auth don't exist yet | — | high | §3.A makes the backend gaps the gating WPs |
| Over-promising PQ E2E; shipping classical and calling it quantum-safe | med | high | §4 stages C1–C4 honestly; C1 is the real "same as chain" win |
| Auth seam drift (jose 4 vs 6, Privy 2 vs 3) when copy-pasting | high | med | clone at current versions; add a seam-parity test |
| Constellation perf on weak GPUs | med | med | port the prototype's Lite tiers + auto-degrade; perf gate |
| Fail-open on missing config (the dominant federation finding class) | med | high | fail-closed defaults + red tests for every control |
| Key-management migration for QSSP wrap (K1) | med | high | versioned envelope + migration + HSM/KMS custody `⟦DECIDE⟧` before K1 |
| 1:1 drift from the design over time | med | med | token-parity + DOM-snapshot tests as a ratchet |

## 8. Decisions reserved (`⟦DECIDE⟧`)
- Gateway-blindness tier for C2 (Tier-E default vs Tier-Z opt-in per repo).
- DEK custody for K1 QSSP wrap: HSM / cloud KMS / software keyring.
- ControlPlane durable backend: RocksDB (engine-local) vs Postgres/Neon (SaaS-native).
- Whether `Operator` members may self-confirm proposals or always HIC-to-admin.

## 9. Traceability
- 1:1 design contract → `webapp/design-prototype/UI_IMPLEMENTATION_SPEC.md` (+ source).
- Capability→surface → `06_WEBAPP_FRONTEND_SPEC.md` Appendix A (still valid).
- Engine ontology/MCP → `02_ARCHITECTURE.md`; gateway routes → `mem-gateway/src/http.rs`.
- Hardening lineage → SECREM-01 sprint, recomputation-gate ADR, `citrate-explorer`
  auth seam + CSP; gitleaks gate (root `.gitleaks.toml`).
- Crypto lineage → `mem-store/src/shred.rs`, `mem-authz/src/grant.rs`,
  `citrate-chain/core/storage/src/crypto/quantum_safe.rs` (QSSP).

---
© 2026 Citrate Inc.. Apache-2.0. Agentile-compliant (Rule-12 frontmatter; four ratchets).
