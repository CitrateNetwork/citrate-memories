---
created: 2026-06-12T00:00:00Z
branch: main
author: Claude Fable 5 (Lane D), directed by Larry Klosowski (@SaulBuilds)
status: active
sprint: MEM-S6 (WP-6.3 packaging + new front-end track)
purpose: Designer-ready front-end spec for the citrate-memories webapp — decomposes every capability into UI, defines auth/RBAC, and specs the 3D graph visualization. Hand to the Claude design team to produce an HTML prototype; implementation (pixel-perfect, Next.js) follows their on-brand work.
---

# Memrizz — citrate-memories Webapp Front-End Spec

> **Working codename: "Memrizz"** (the constellation of org memory). Final name
> is the team's call — alongside the federation's `CitrateScan` (explorer), this is
> the **memory** surface. Treat the name as a token to replace.

> **How to read this doc.** §1–3 are the product + architecture + auth framing the
> designer needs as constraints. §4 is the **functional decomposition** — every
> citrate-memories capability mapped to a UI surface (this is the bulk; it tells the
> designer *what must exist*). §5 is the **3D data-visualization spec** (the
> centerpiece). §6–9 are design-language, IA, component inventory, and states. §10–13
> are stack, security, OSS, and phasing. **Decisions reserved for Saul** are flagged
> `⟦DECIDE⟧` inline. **UX/UI choices are explicitly the designer's** — this doc says
> *what* and *why*, not *exactly how it looks*.

---

## 0. The one-paragraph pitch (for the designer)

citrate-memories is "git for agents" — a content-addressed knowledge **DAG** that
holds the entire story and code-shape of a 34-repo software federation (today:
**9,567 nodes / 6,079 edges**), so any agent or human can get the context of any
part of the org in seconds without reading the code. It already works headlessly
over MCP. **Memrizz is the human face of it**: a Next.js webapp where a
non-technical teammate logs in with their Citrate identity, *sees* the org's memory
as a living 3D constellation, asks it questions in plain language with the model of
their choice, watches the answer's sources light up, and — where a human's judgment
is required (confirming an AI's guess, resolving a contradiction, forgetting a
record) — does so through a calm, trustworthy Human-in-the-Loop interface. It must
feel like piloting a starship through your company's mind: gorgeous, fast, legible,
and never lying about what it knows or how sure it is.

---

## 1. Product framing

### 1.1 Who it's for (and the design consequence)

| Persona | Needs | Design consequence |
|---|---|---|
| **Non-technical teammate** (PM, ops, design, exec) | "What happened with X? Why did we decide Y? What depends on Z?" — answered in plain language with trustworthy citations | Plain-language first; the graph is *explorable* but never *required*; jargon (DAG, blue_score, Belnap) is hidden behind friendly affordances with progressive disclosure |
| **Technical teammate / agent operator** | Connect their own model via MCP; deep recall; propose edges; trace provenance | Power tools one layer down; BYOM connection manager; keyboard-driven |
| **Regular administrator** | Onboard team members, scope their access, run the HITL review queues for their tenants | Admin console scoped to their subtree; review inboxes |
| **Super administrator** | Everything: roles, tenants, ingestion, ops/recovery, the "forget" (crypto-shred) power | Full admin + dangerous-action surfaces with strong confirmation + audit |

### 1.2 The three core experiences (the product is these three, linked)

1. **See it** — the 3D memory constellation (the unmistakable hero).
2. **Ask it** — conversational RAG over the graph with a model the user picks
   (in-app) or brings (BYOM over MCP). Answers cite nodes; cited nodes light up and
   fly into view. The chat and the constellation are **one linked surface**.
3. **Steward it** — Human-in-the-Loop: confirm/reject AI proposals, resolve
   contradictions, supersede stale memory, and (admins) forget records — all
   recorded in a tamper-evident audit chain the user can see.

### 1.3 Non-negotiable product principles (these come straight from the engine's invariants — honor them in UI)

- **Never lie about certainty.** Every memory carries a **trust tier** and a
  **freshness** stamp. The UI must always show how sure and how current a thing is.
  A superseded or contradicted memory must be *visibly* so.
- **Two planes, always distinguishable.** **Derived** memory (deterministic, rebuilt
  from git/markdown — "the record") vs **Asserted** memory (signed human/agent
  claims — "what someone said"). The UI must make this legible at a glance, because
  it's the whole trust model.
- **AI output is a proposal, never a fact, until a human confirms.** Anything an LLM
  or the analogy engine produces enters **quarantined** and is advisory. Confirmation
  is a deliberate human act. This is the HITL spine.
- **Fail closed, visibly.** No access without a valid scope; a denied action says so
  plainly (and is audited). Missing config never silently opens a door.
- **Everything is audited.** Reads, writes, denials — a hash-chained log the user can
  inspect. Surface it as a feature (trust), not hide it.

---

## 2. Architecture & packaging (the seam the webapp sits on)

### 2.0 Multi-tenancy model — **SaaS, org-isolated** (decided 2026-06-13)

> ⚠️ **Terminology — read this first.** The engine already uses the word **"tenant"**
> to mean **a repo** inside one federation. The SaaS adds a new, higher isolation
> level. To avoid collision, this doc uses:
> - **Org** (a.k.a. **Workspace**) = the **SaaS customer** — the new top-level
>   isolation boundary. (What most SaaS apps call a "tenant.")
> - **Repo-tenant** = a repo's memory inside an Org's federation (the engine's
>   existing `tenant`/`repo:<name>/memory` concept). An Org *contains many
>   repo-tenants.*

Memrizz is **multi-tenant SaaS**: one platform deployment serves many Orgs, each
fully isolated. The isolation model:

- **One isolated memory store per Org.** Each Org gets its own encrypted RocksDB
  store + keyring + bge index + rolling checkpoints, owned by a per-Org engine
  instance the gateway manages. **Hard isolation by construction** — an Org's data is
  in a different store + sealed under a different keyring, so cross-Org leakage isn't a
  query-filter bug waiting to happen, it's physically separate. (This also keeps the
  engine's single-writer RocksDB lock + per-Org crypto-shred clean: forgetting Org A
  never touches Org B.)
- **`mem-gateway` is the multi-Org front door.** It resolves the caller's Org from
  their authenticated identity, routes to that Org's engine instance, and enforces the
  Org boundary *before* any repo-tenant scope check. Org resolution is the first gate;
  CapabilityGrant scoping is the second. ⟦DECIDE⟧ engine-instance topology:
  *Recommendation: a gateway process supervising N per-Org daemons (one DB lock each),
  with lazy spin-up + idle eviction; revisit a single multi-store process only if the
  per-process overhead bites.*
- **Three authority levels** (see §3): **Platform Operator** (us, the SaaS host) →
  **Org Owner** (the customer's super-admin) → **Org Admin** / **Member** (within an
  Org, the delegation tree). The Org Owner's `*` scope means `*` **within their Org**,
  never across Orgs.
- **Provisioning:** an Org is created (signup/invite), its store is initialized, its
  first Org Owner is bound, and its repos are connected (each becomes a repo-tenant
  via backfill). Billing/quotas are an Org-level concern ⟦DECIDE⟧ (out of scope for
  the design prototype; note the seam).
- **OSS still works:** a self-hoster runs the same bundle with a single Org (or their
  own set of Orgs) — multi-tenancy is additive, not a fork. The OSS artifact is the
  platform; running it for one Org is the degenerate case.

**Design consequences for the prototype:** an **Org/Workspace switcher** in the top
chrome (for users in >1 Org, e.g. consultants; most users see one); all data,
constellation, admin, and audit are **always Org-scoped**; the admin console gains an
Org-provisioning + Org-settings surface for the Org Owner, and a separate (minimal,
internal) Platform-Operator surface; "forget" is Org-Owner-scoped to their own
repo-tenants. The designer should treat **Org** as the implicit container of
everything — it is never mixed across Orgs on one screen.


The engine is Rust (9 crates) speaking MCP over stdio/Unix-socket via the
`mcp_serve` daemon. A browser can't speak that. So the package gains **one new
service** and the webapp is **three runtimes** (mirroring how `citrate-explorer` is
structured):

```
                ┌─────────────────────────────────────────────┐
  Browser  ───► │  Next.js app (Memrizz)                     │
  (RP)          │  • UI: 3D constellation, chat, HITL, admin   │
                │  • Server actions / route handlers (BFF)     │
                │  • OIDC RP (citrate-identity, PKCE)          │
                └───────────────┬─────────────────────────────┘
                                │  authenticated HTTP/JSON + SSE
                                ▼
                ┌─────────────────────────────────────────────┐
   NEW  ───►    │  mem-gateway  (Rust, axum)                   │
                │  • REST/JSON over mem-query + mem-mcp tools   │
                │  • MCP-over-HTTP/SSE endpoint (BYOM)          │
                │  • maps OIDC identity → CapabilityGrant       │
                │  • per-request authz + audit (reuses mem-authz)│
                │  • SSE: live graph deltas + audit feed        │
                │  • serves precomputed 3D layout (UMAP/PCA)    │
                └───────────────┬─────────────────────────────┘
                                │  in-process (owns the DB lock)
                                ▼
                ┌─────────────────────────────────────────────┐
                │  citrate-memories engine (the 9 crates)      │
                │  RocksDB store (encrypted) + bge + HNSW       │
                │  rolling checkpoints (WP-6.5)                 │
                └─────────────────────────────────────────────┘
```

**Why a new `mem-gateway` and not "just call the daemon":** the browser/BFF needs
HTTP+SSE, per-user authz mapping, a stable JSON contract, a remote MCP transport for
BYOM, and a place to serve the cached 3D layout. The gateway *is* the productization
of the headless daemon — it folds the single-writer daemon role (DB lock + bge model
+ rolling checkpoints) in, and adds the network surface. ⟦DECIDE⟧ build `mem-gateway`
as a new binary in the `mem-mcp` crate (closest to the existing daemon) vs a new
`mem-gateway` crate. *Recommendation: new crate `mem-gateway`, depends on
mem-mcp/mem-query/mem-authz — keeps the network surface auditable in isolation
(important for the Tier-1 audit, MEM-S6 WP-6.1).*

**BYOM (bring your own model) path.** The gateway exposes an authenticated **MCP
endpoint over the Streamable-HTTP transport** (the modern MCP remote transport).
The user's chosen MCP client (Claude Desktop/Code, Cursor, an OpenAI-compatible
agent, a local model) connects with a short-lived token minted from their
CapabilityGrant. Every tool call is scoped + audited identically to the in-app path.
This is how "query the graph with whatever model they want" is satisfied **without
the model ever touching the raw store** — it goes through the same authz + audit gate.

**In-app model choice.** For the conversational surface, model routing goes through
**Vercel AI SDK** provider routing (and/or `citrate-inference-gateway`): the user
picks Claude / GPT / a local / Citrate's own model from a dropdown; the app does
graph-grounded RAG (recall/search/neighbors as tools the model calls).

**Deployment / packaging (WP-6.3), multi-Org.** Ship as a Docker bundle: `mem-gateway`
(supervises the per-Org engine instances; owns the volume holding each Org's encrypted
store + checkpoints, isolated by Org) + the Next.js app, with citrate-identity as the
external OIDC issuer. The Next.js app deploys to Vercel; `mem-gateway` runs where the
data lives (a box/volume), like the explorer's off-Vercel indexer. The **same bundle is
the OSS artifact** (§12) — a self-hoster runs it for one Org or many. For the hosted
SaaS, `mem-gateway` scales by Org (lazy per-Org daemon spin-up + idle eviction; large
Orgs can pin a dedicated instance). Per-Org volumes + keyrings mean an Org's data can
be exported or forgotten as a unit.

---

## 3. Auth & RBAC (grounded in the real engine model — do not invent a parallel one)

### 3.1 Authentication — citrate-identity OIDC (same seam as explorer/dashboard)

Memrizz is a **generic OIDC Relying Party** behind one seam (matching the
`citrate-explorer-auth-seam` pattern): **Authorization Code + PKCE**, public client,
issuer `auth.citrate.ai`. Claims consumed: `sub` (stable principal), `wallet_address`
(the user's counterfactual smart-wallet or EOA), `signing_method`. **Fail closed:**
if `OIDC_ISSUER`/`OIDC_AUDIENCE` are unset, reject all tokens (the SECREM-02 1.4 fix
— never pass `undefined` to verify). Login screen = "Sign in with Citrate."

**Org resolution (multi-tenant).** The OIDC `sub` is a *global* principal; **Org
membership is Memrizz's own mapping** (`sub` → one-or-more Orgs + role), held in the
gateway's control-plane store, established at invite/provision time. On login the
gateway resolves the user's Org(s); if they belong to several, the Org switcher picks
the active one and every request carries the active Org, re-verified server-side. A
`sub` with no Org membership lands on a "request access / create an Org" screen, never
on someone else's data. (citrate-identity stays a pure identity issuer; it does not
need to know about Orgs — the SaaS boundary lives in Memrizz.)

### 3.2 Authorization — the webapp's RBAC **is** the `CapabilityGrant` model

The engine already has the authz primitives; the webapp productizes them. **Do not
build a second RBAC system** — map roles onto these existing structures:

- `CapabilityGrant { issuer, recipient, allowed_resources[], policy, expires_at_ms, revoked, delegation_chain[] }`
- `ResourceScope { resource_id, can_read, can_write }` — `resource_id` is
  `repo:<tenant>/memory` (or `*`).
- `PolicyProfile { ReadOnly, Guided, Operator, Maintainer }` — **four tiers already exist.**
- `DelegationStep { delegator, at_ms }` — the **parent-child chain** the user asked for.

### 3.3 The authority levels + RBAC, mapped (multi-Org aware)

**Org boundary is checked first, always.** Every grant below is scoped *within one
Org*; the gateway resolves Org from identity and refuses cross-Org access before any
scope check. `*` means "all repo-tenants **in this Org**."

| Role | Layer | PolicyProfile | Scopes | Can delegate? | Can do |
|---|---|---|---|---|---|
| **Platform Operator** | platform (us) | — (out-of-band) | platform | n/a | provision/suspend Orgs, platform health/ops, **never reads Org memory content** (isolation: ops-plane only, audited). The SaaS host role. |
| **Org Owner** (Super Admin) | per-Org | `Maintainer` | `*` **within the Org** | yes — issues root + admin grants in the Org | everything in their Org incl. **forget (crypto-shred)**, repo-tenant lifecycle, ingestion, Org ops/recovery, manage Org admins, Org settings/billing, OSS export. **Cannot** cross Orgs. |
| **Org Admin** (Regular Admin) | per-Org | `Operator` | subtree of repo-tenants (their `allowed_resources`) | yes — only a **subset** of their own scopes (attenuation) | onboard members under them, run HITL review queues for their repo-tenants, trigger ingestion for them, issue/revoke **delegated** grants in their subtree. **Cannot** shred, manage other admins, touch Org-level ops/billing. |
| **Member** | per-Org | `Guided` or `ReadOnly` | specific repo-tenants | no (leaf) | query, visualize, BYOM-connect, **propose** (quarantined) edges/assertions; confirmation is HITL-gated to admins by default ⟦DECIDE⟧ whether `Operator` members can self-confirm. |
| **Agent / BYOM session** | per-Org | inherits the connecting user's grant | == user's (same Org) | no | exactly what the user can, in the user's Org, every call audited under the user's principal. |

> The two admin tiers the user asked for = **Org Owner** (super admin) + **Org Admin**
> (regular admin), both *inside* an Org. The **Platform Operator** is the SaaS-host
> super-role above all Orgs — deliberately walled off from Org memory *content* (it can
> manage lifecycle but not read the graph), so "we host it" never means "we can read
> your memory."

**Parent-child / future onboarding = the `delegation_chain`.** When an admin
onboards a member, the new grant's `delegation_chain` records the admin as
`delegator`. Attenuation is enforced: **a child's `allowed_resources` must be a
subset of the parent's** (you cannot grant what you don't hold). **Revocation
cascades down the tree** (revoking a parent invalidates descendants) — this is the
F-7 "delegation-revocation cascade" the engine deferred to v2; **Memrizz is the
trigger to build it**, together with F-5 (binding the grant's principal to the real
OIDC `sub`/wallet instead of a bare pubkey). Flag both as backend prerequisites.

**UI consequence:** the admin console needs a **delegation tree** view (who granted
whom, what scopes, expiry), grant issue/revoke with a scope picker that *cannot
exceed the issuer's own scopes* (the UI enforces attenuation before the backend
re-checks it), and a revoke action that visibly shows the cascade ("revoking Dana
also revokes 3 members under her").

---

## 4. Functional decomposition → UI surfaces (the core of this doc)

Every capability the engine exposes, mapped to a surface. The designer turns each
into screens/components; this enumerates *what must be reachable and legible*.

### 4.1 The Constellation (3D graph explorer) — the hero surface  → full spec in §5

Exposes: the whole graph; node browsing; filtering; selection; blast-radius;
time-travel (as_of); analogy beams. It is the default landing view (with a calm
"guided" overlay for first-timers). Every other surface can deep-link into it
("show this in the constellation") and it can launch every other surface (click a
node → inspector).

### 4.2 Ask — conversational RAG (the AI/ML surface)

Engine tools behind it: `recall` (storyline), `search` (semantic, bge cosine),
`neighbors` (blast-radius), `analogy`, `verify`, `as_of`, `critique`.

- **Model picker** (in-app): Claude / GPT / local / Citrate model, via AI SDK provider
  routing. Show the chosen model + its cost/latency badge.
- **Chat thread** with streaming. The model is given the recall/search/neighbors
  tools; when it calls them, show a subtle "consulting memory…" affordance.
- **Citations are first-class.** Every claim in an answer cites node(s). A citation
  chip shows kind + trust tier + freshness; clicking it (a) opens the Node Inspector
  and (b) **flies the constellation to that node and lights it**. The chat ↔ graph
  link is the signature interaction.
- **"Show your work"** toggle: reveal which tool calls + which nodes produced the
  answer (HITL transparency — the user can audit the AI's grounding).
- **Self-critic inline** (`critique`): an answer can carry a "completeness" chip —
  if the critic flags gaps (truncated coverage, superseded sources, contradictions,
  adjacent unconfirmed proposals, stale index), surface them as a dismissible
  "heads-up" so the user knows what the answer might be missing. **This is a
  differentiator — most RAG UIs never tell you what they left out.**
- **Freshness banner** when the index watermark is stale ("memory last synced 3h ago").

### 4.3 Search & Recall (the fast, non-conversational path)

- **Command palette / omnibox** (⌘K): semantic search across a tenant or all
  readable tenants; results ranked with score + provenance + freshness; arrow-to-fly.
- **Storyline view** (`recall`): a tenant's recent memory as a readable timeline
  (newest first, budget-shaped), each item showing kind, trust, status (⚠ superseded
  surfaced), and a source link.
- **Scope selector**: which tenant(s) — constrained to the user's readable scopes.

### 4.4 Node Inspector + Provenance/Verify (the trust surface)

Engine tool: `verify`. When a node is selected anywhere:

- **Identity & content:** kind, title, tenant, plane (Derived/Asserted badge),
  trust tier (with a plain-language tooltip), status, `valid_from`/freshness.
- **Provenance / verify result:** is it **trustworthy**? signature posture (Derived =
  "trusted by construction"; Asserted = "signature valid/invalid/missing"), whether
  it is **superseded by** / **refuted by** other nodes (with links), whether a
  **Belnap contradiction** is recorded. Render `is_trustworthy()` as a single clear
  verdict chip with the reasons beneath.
- **Source pointer:** for Derived nodes, a link to the real artifact (repo/path/sha or
  git commit) — deep-link out to the explorer or repo. **Rule 9: we point, we don't
  copy** — the inspector links to canonical sources, never claims to be them.
- **Neighbors / blast-radius** (`neighbors`): in/out edges with kind + direction +
  whether quarantined ("proposed"); cross-tenant neighbors shown only if the user can
  read that tenant (grant intersection — silently hidden otherwise).
- **Actions** (RBAC-gated): "show neighbors in constellation", "find analogues",
  "propose an edge from here", "supersede this", "verify again".

### 4.5 Time-Travel (as_of / decision-replay) — a flagship interaction

Engine tool: `as_of` (Derived-plane snapshot at time T).

- A **timeline scrubber** along the bottom of the constellation. Dragging it sets T;
  the graph **morphs** to its state at T — nodes that didn't exist yet fade out, ones
  later superseded return to Active, the constellation visibly breathes through time.
- A "decision replay" mode: pick a decision (an ADR/Sprint node) and watch the memory
  that existed *when it was made* — answering "what did we know then?"
- Plain-language framing for non-technical users: "Rewind the memory to June 1st."

### 4.6 HITL Review Center (the human-judgment surface) — second-most-important after the constellation

This is where the two-plane / quarantine / trust model becomes a *workflow*. A
unified **inbox** with filtered queues:

- **Proposals queue** (`propose_edge`/quarantined edges + InferredAdvisory nodes):
  each card shows the two endpoints (mini-constellation preview), the **proposer**
  (which agent/model + when), the **evidence/rationale**, and the proposed relation.
  Actions: **Confirm** (promotes to load-bearing; a confirmed `Supersedes` applies the
  status transition, cycle-guarded) / **Reject** / **Ask for more evidence**. Confirm
  is a deliberate, audited human act. Bulk-confirm with care (multi-select + a
  summary).
- **Contradiction queue** (Belnap `Both`): nodes where assertions conflict — show the
  conflicting claims side by side, who asserted each, and a resolve flow.
- **Supersession review:** stale (Superseded) nodes and their replacements; confirm a
  pending supersession or flag a wrong one.
- **Self-critic findings:** completeness gaps surfaced by `critique`, as actionable
  cards ("this recall omitted 2 superseded sources").
- Every action writes to the audit chain and shows a confirmation toast with the
  audit sequence number (trust reinforcement).

### 4.7 Assert / Memory-Diff Handoffs (the write surface)

Engine: `assert` (signed Asserted node), `merge_diff` (apply a signed session
subgraph — the "git for agents" handoff).

- **Add a memory** (member+): a guided form to record a rationale/claim/note —
  becomes a **signed AgentAsserted** node (the UI signs via the user's
  identity-bound key, F-5). Clear framing: "this is *your assertion*, recorded as
  what you said — not the deterministic record."
- **Handoff import/export:** export the current session's new assertions as a
  signed memory-diff (downloadable JSON), and import/merge one from a teammate or
  agent — with a **diff preview** (what nodes/edges will land, what's rejected and
  why: bad signature, cycle, oversized) before applying. Mirrors a git PR review.

### 4.8 Federation / Org Overview (the meta-graph)

Engine: the reserved `federation` tenant — one `Tenant` node per repo + `DependsOn`
edges from the manifest drift map.

- **Org map**: the 34 tenants as a constellation-of-constellations; each tenant a
  cluster you can open. `DependsOn` edges = the dependency blast-radius across repos.
- **Per-tenant health card:** node/edge counts, freshness (last ingest), trust
  distribution, contradiction count, last chain-anchor.

### 4.9 Audit Log Viewer (trust + compliance)

Engine: the persistent blake3 hash-chained `AuditChain` (`MemoryEvent::Read/Write/Denied`).

- A filterable, **integrity-verified** event stream (actor, event, resource, detail,
  timestamp, sequence). A green "chain intact ✓ (N records)" badge; if the chain
  fails verification, a loud banner (tamper evidence is the point).
- Filters: by user, tenant, event type, time. Export for compliance.
- Live tail via SSE (new events stream in).
- ⟦DECIDE⟧ who sees the full audit (super admin) vs their own actions (everyone).

### 4.10 Durability / Ops (admin) — surfaces the work we just did

- **Checkpoint status (WP-6.5):** last checkpoint time, retained count, "create
  checkpoint now", and a **one-click recovery** flow (restore from a checkpoint) with
  strong confirmation. Make the durability guarantee *visible* ("recovery point: 4
  min ago").
- **CRDT sync / replicas:** peer replicas, last merge outcome (nodes added/merged,
  contradictions surfaced), divergence/convergence status.
- **Chain-anchor status:** the latest tenant-root → 40204 block binding (root,
  block #, hash, time) — "memory is notarized to the chain at block N."
- **Ingestion / backfill (admin):** trigger a re-ingest of a tenant from its repo,
  watch progress, see the resulting counts; re-embed control.

### 4.11 Forget / Crypto-Shred (super-admin only — the dangerous power)

Engine: per-tenant crypto-shred (destroy the key → the data is unreadable forever).

- A deliberately **high-friction** flow: type-to-confirm the tenant name, a clear
  statement of consequence ("this permanently forgets all of <tenant>'s memory; it is
  cryptographically irreversible"), super-admin only, double-audited. Show what will
  be forgotten (counts) before the act. This is the "right to be forgotten" made real
  — design it to be impossible to do by accident.

### 4.12 BYOM — "Connect your model" (the MCP gateway surface)

- A **connection manager**: "Connect a model via MCP." Generates the user's personal
  MCP endpoint URL + a short-lived token (bound to their CapabilityGrant), with
  copy-paste config blocks for Claude Desktop, Claude Code (`.mcp.json`), Cursor, and
  a generic Streamable-HTTP MCP client. A live "connected clients" list (which models
  are attached, last activity) with a per-client **revoke**.
- Clear scope display: "this connection can read tenants A, B and propose to B" — the
  user sees exactly what their model can touch, and every call shows up in their audit.

### 4.13 Admin Console (users, roles, grants, tenants)

- **People:** invite (sends a Citrate-identity onboarding), assign role, set parent
  (delegation), set per-tenant scopes (attenuated to the issuer's own).
- **Grants:** the delegation tree (visual), issue/revoke (with cascade preview),
  expiry management, see each grant's scopes.
- **Tenants:** the 34 repos, add/remove, ingestion control, per-tenant settings.
- **Settings:** index/freshness, model defaults, retention, OSS visibility (super
  admin).

### 4.14 Cross-cutting: notifications & live presence

- SSE-driven: "new proposal awaiting your review", "ingestion finished", "a teammate
  asserted a memory in your tenant", "contradiction detected". A calm notification
  center, not a firehose.
- Optional presence: who else is exploring (multi-session daemon already supports it).

---

## 5. The 3D Data-Visualization Spec (the centerpiece — make it out of this world)

> Goal: the moment someone opens Memrizz, they *get it* and want to fly around. It
> must be beautiful, **fast on a laptop**, and — critically — **honest** (the visuals
> encode real trust/plane/status, never decoration for its own sake).

### 5.1 What's being visualized

Up to ~10k nodes / ~6k edges today, growing. Each node has: a 768-d **bge embedding**
(semantic position), a **kind** (→ the "material type" the user named), a **plane**
(Derived/Asserted), a **trust tier**, a **status** (Active/Superseded/Archived), a
**Belnap confidence** (contradiction = `Both`), a **tenant**, a `valid_from` time,
and a **degree** (blast-radius). Edges have a **kind** and a **quarantined** flag.

### 5.2 Spatial layout — the XYZ space (multiple modes, toggleable)

The user asked for "xyz space with vectors based on whether it's documentation, code,
specs, configs, tests." Two complementary ways to honor that — ship both as modes:

**Mode A — Semantic Galaxy (default, the "wow").** Project the 768-d bge vectors →
**3D via UMAP** (or PCA for a faster/cheaper deterministic version), computed
**server-side** in `mem-gateway`, cached per store-state (invalidated on
watermark/node-count change — reuse the `TenantIndexCache` invalidation logic).
Semantically related memories physically cluster — you *see* the shape of the org's
knowledge. **Node kind drives material/color** (the user's "vector by type"), so the
galaxy is colored by code/docs/specs/configs/tests even though position is semantic.
Deterministic projection so the layout is stable across sessions (matches the
Derived-plane determinism ethos).

**Mode B — Structured Lattice (the "reason about it" view).** Explicit axes:
- **X = material type lane** — discrete columns: **Code · Docs · Specs · Configs ·
  Tests/Audit · Claims**. (This is literally the user's requested axis.)
- **Y = time** (`valid_from`) — older at the bottom, recent at top; the org's history
  rises.
- **Z = trust tier** (or tenant, toggleable) — DerivedDeterministic in front,
  InferredAdvisory in back, so the trustworthy core is closest.

**Mode C — Federation Islands.** Each tenant a floating island/cluster; `DependsOn`
edges are luminous bridges between them. Zoom into an island to expand its sub-graph.

**Mode D — Storyline River.** One tenant's `TemporalNext` spine as a flowing river of
commits/docs through time — the "storyline" recall made spatial.

**Node-kind → material-type mapping** (the canonical taxonomy for color/shader; the
designer sets exact hues on-brand):

| Material lane | Node kinds | Visual intent |
|---|---|---|
| **Code** | Commit, Pr | crystalline / electric — the built thing |
| **Docs** | Doc, Narrative, Handoff | warm / paper-glow — the written word |
| **Specs** | Sprint, Adr, WorkPackage, Rationale | blueprint / wireframe — the plan |
| **Configs** | ManifestChange, PinBump, DriftEvent | mechanical / hex — the wiring |
| **Tests/Audit** | Audit, Finding, Benchmark, Blocker, TechDebt | alert / ember — the checks (findings pulse) |
| **Claims** | Claim, AgentAction, AnalogyHypothesis | signed / shimmer — what someone asserted |
| (meta) | Tenant | a sun/anchor per island |

### 5.3 Encoding (every visual channel carries real meaning)

- **Color + material** ← node **kind** (the lanes above).
- **Glow / bloom intensity + halo** ← **trust tier**: DerivedDeterministic = bright
  solid core; HumanConfirmed = gold ring; AgentAsserted = softer; InferredAdvisory /
  quarantined = **translucent + flicker** (visibly "not load-bearing yet").
- **Status** ← Superseded = desaturated grey + a faint **ghost-trail** to its
  superseder; Archived = faded; Active = full presence.
- **Contradiction (Belnap `Both`)** ← a **red pulse + subtle glitch/chromatic
  aberration** shader. You can *feel* the unresolved conflict across the room.
- **Size** ← degree / blast-radius (central memories are bigger).
- **Edges:** kind → color + style; **particle flow** along the edge shows direction
  and liveness (TemporalNext = gentle time-stream; Supersedes = bold replacement
  arrow; DependsOn = structural beam; **AnalogousTo = dashed cross-tenant arc that
  shimmers** — the latent connections; Refutes/Contradicts = red). **Quarantined
  edges = dashed + translucent + a "pending" pulse** (advisory, not load-bearing).

### 5.4 Shaders & post-processing (the "light shaders, not too heavy" brief)

Budget-first. Target **60 fps at 10k nodes on Apple-silicon integrated GPUs**, with a
**graceful "Lite" mode** for weak machines (auto-detected via a quick GPU probe +
manual toggle).

- **Selective bloom** (postprocessing) — only on high-trust + selected + hovered
  nodes, so glow means something and the cost is bounded.
- **Fresnel / rim lighting** node material for the "energy orb" look (cheap, per-vertex).
- **GPU-instanced particle flows** on edges (one InstancedMesh, shader-driven motion;
  density auto-scales with distance/zoom — drop to static lines at far LOD).
- **Depth fog / atmospheric scattering** for spatial depth + focus.
- **Contradiction glitch** — a tiny, localized chromatic-aberration/noise shader only
  on `Both` nodes (a handful at a time — negligible cost).
- **Soft star-field / nebula backdrop** (a single shader plane, parallax) for the
  "space" feel without geometry cost.
- **NOT in budget:** full SSAO, real-time GI, per-node shadow maps, volumetric god-rays
  on everything. (A *selective* god-ray on the focused node only is OK.)

**Performance techniques (mandate these in implementation):** InstancedMesh for nodes
and for edge particles; **LOD** (near = full material; mid = simple lit sphere; far =
billboard/point); frustum + distance culling; merge static edges into one geometry;
cap simultaneous bloom emitters; throttle the particle sim; offload the UMAP/PCA
layout to the server (never compute embeddings layout in the browser); stream the
graph in tiles for very large stores. A persistent **fps/quality HUD** (dev) +
auto-degrade if fps drops.

### 5.5 Interactions (the flight controls)

- Orbit / pan / zoom (trackpad + mouse + touch); "fly to" animation on select/cite.
- **Hover** → lightweight tooltip (title, kind, tenant, trust, freshness).
- **Click** → Node Inspector (§4.4) + the node centers + its neighborhood highlights.
- **Filter rail** (left): tenant, material type, trust tier, status, plane, time
  range, freshness, "show quarantined". Filters dim/hide, never delete — the user
  always knows they're filtered.
- **Blast-radius focus:** select a node → everything except its N-hop neighborhood
  dims into the fog; a slider sets N.
- **Box/lasso select** → multi-select → (admin) bulk HITL actions.
- **Time scrubber** (§4.5) → the graph morphs through `as_of` states.
- **Analogy beams:** "find analogues" on a node → cross-tenant arcs animate in,
  ranked by the combined embedding+structural score.
- **Search-to-fly:** ⌘K result → camera flies and lights the node.
- **Linked from chat:** a citation lights + flies its node (the signature link).
- **Mini-map / overview cube** for orientation in big graphs.
- **Accessibility:** every constellation action has a non-3D equivalent (a list/table
  view, keyboard nav) — the 3D is the joy, never the only door. Respect
  `prefers-reduced-motion` (calm the particles/morphs).

### 5.6 Library guidance (for the designer's awareness; final call at implementation)

`react-three-fiber` + `three.js` + `@react-three/drei` (controls, instances, Html
labels) + `@react-three/postprocessing` (selective bloom). Consider `three-forcegraph`
/ `r3f-forcegraph` for the force-directed fallback layout and `deck.gl` only if we
need >100k points later. Layout (UMAP) server-side in Rust (or a small Python sidecar)
— **not** in the browser.

---

## 6. Design language & aesthetic direction (input, not prescription)

- **Tone:** "calm command deck." Confident, spacious, dark-first (a starfield wants
  dark), with the constellation as the light source. Non-technical users should feel
  *invited*, not intimidated — progressive disclosure everywhere (the jargon lives
  behind tooltips and "details" toggles).
- **The AI/ML + HITL edge:** make the machine's reasoning *visible and reviewable*
  (show-your-work, citations that light up, completeness heads-ups, the proposal
  inbox). The aesthetic of trust: certainty and freshness are always on screen; the
  difference between "the record" (Derived) and "what someone said" (Asserted) is
  unmistakable; AI proposals visibly shimmer as *pending* until a human commits.
- **Motion:** purposeful and physics-y (fly-to, morph-through-time, beam-in
  analogies). Honor `prefers-reduced-motion`.
- **Brand:** Saul brings the on-brand palette/type from the design team; this doc
  fixes *meaning→channel* mappings (color = kind, glow = trust, etc.) so the
  designer chooses the exact hues while preserving semantics.
- **Delight, earned:** the star-field, the bloom on a high-trust core, the analogy
  beams arcing across tenants, the timeline morph — moments of wonder that are also
  *informative*.

---

## 7. Information architecture / screen map

```
/login                         Sign in with Citrate (OIDC PKCE)
/                              Constellation (hero) + omnibox + filter rail
  ├─ right dock: Node Inspector (on select)
  ├─ bottom: Time scrubber (as_of)
  └─ overlay: first-run guided tour
/ask                           Conversational RAG (model picker, citations↔graph)
/review                        HITL Review Center (proposals · contradictions · supersessions · critic)
/storyline/:tenant             Recall storyline timeline
/org                           Federation / org overview (meta-graph)
/audit                         Audit log viewer (integrity-verified, live tail)
/connect                       BYOM — connect-your-model (MCP endpoint + tokens + clients)
/admin                         Org admin console (Org-scoped)
  ├─ /admin/people             users, roles, parent/delegation, scopes
  ├─ /admin/grants             delegation tree, issue/revoke (cascade preview)
  ├─ /admin/tenants            repo-tenants, ingestion/backfill, settings
  ├─ /admin/org                Org settings, provisioning, billing/quota   (Org Owner)
  ├─ /admin/ops                checkpoints/recovery, CRDT sync, chain-anchor  (Org Owner)
  └─ /admin/forget             crypto-shred  (Org Owner, high-friction)
/platform                      Platform-Operator console (Org lifecycle; NO memory content)
/me                            profile, my Orgs, my grants, my connected models, my audit
```

Global chrome: top bar with the **Org/Workspace switcher** (left-most — everything
below it is Org-scoped; most users have one Org), then repo-tenant scope switcher
(constrained to readable scopes), model picker, notifications, profile; ⌘K omnibox
anywhere; a persistent "explain this" help affordance. The current Org is always
unambiguous on screen.

---

## 8. Component inventory (reusables for the designer)

- **TrustChip** (tier + plain tooltip), **PlaneBadge** (Derived/Asserted),
  **StatusPill** (Active/Superseded/Archived), **FreshnessStamp**, **ContradictionFlag**.
- **NodeCard** (compact + expanded), **NodeInspectorPanel**, **VerifyVerdict**
  (trustworthy/not + reasons), **NeighborList**.
- **CitationChip** (links chat ↔ graph ↔ inspector).
- **ProposalCard** (endpoints preview + proposer + evidence + confirm/reject),
  **ContradictionResolver**, **CriticGapCard**, **DiffPreview** (memory-diff merge).
- **ScopePicker** (attenuation-aware), **DelegationTree**, **GrantRow**, **RoleBadge**.
- **ConstellationCanvas** (the R3F scene) + **FilterRail** + **TimeScrubber** +
  **LayoutModeSwitch** (Galaxy/Lattice/Islands/River) + **QualityToggle** (Lite/Full)
  + **MiniMap**.
- **ModelPicker**, **MCPConnectionCard**, **ConnectedClientsList**.
- **AuditStream** (live, integrity badge), **CheckpointStatus**, **AnchorStatus**,
  **SyncStatus**, **IngestionProgress**.
- **DangerConfirm** (type-to-confirm), **ToastWithAuditSeq**, **EmptyState**,
  **PermissionDenied**, **StaleWatermarkBanner**, **GuidedTour**.

---

## 9. States & edge cases (don't let the designer forget these)

- **Loading:** the graph streams in (skeleton constellation → nodes populate); never a
  blank black void with no feedback.
- **Empty scope:** a member with no readable tenants sees a friendly "ask your admin
  for access," not an error.
- **Permission denied:** plain, non-leaky ("you don't have access to this tenant") —
  and it's audited.
- **Stale index:** the freshness banner; offer "ask an admin to re-sync."
- **Contradiction present:** the review center badges it; the node glitches in 3D.
- **Huge graph / weak GPU:** auto-Lite mode + tile streaming + a "your device is in
  Lite mode" note with a toggle.
- **Quarantined everywhere:** make "proposed/advisory" unmistakable so nobody acts on
  an unconfirmed AI guess as if it were fact.
- **Offline gateway:** clear "memory service unreachable" with retry, never a silent
  half-state.
- **Forget irreversibility:** triple-clear before, audit after.

---

## 10. Tech stack & "bells and whistles"

- **Next.js (App Router, latest), React 19, TypeScript.** Server actions / route
  handlers as the BFF to `mem-gateway`. (Use the `vercel:nextjs` skill at implementation.)
- **shadcn/ui + Tailwind** for the 2D chrome; **react-three-fiber + three.js + drei +
  postprocessing** for the constellation. (`vercel:shadcn`.)
- **Vercel AI SDK** for the chat + model routing + tool-calling RAG. (`vercel:ai-sdk`.)
- **Streaming everywhere:** SSE for live graph deltas, audit tail, ingestion progress,
  notifications, presence.
- **MCP Streamable-HTTP** for BYOM (the gateway is the MCP server; the app is also an
  MCP host for the in-app chat).
- **Auth:** OIDC PKCE RP to citrate-identity (`vercel:auth` patterns; fail-closed config).
- **Perf:** RSC for the shell, client islands for the canvas; route-level code-split
  the 3D bundle (it's heavy — never block first paint on three.js).
- **Optional dazzle (in budget):** view-transitions for fly-to, subtle haptics on
  touch, sound-design hooks (off by default), a shareable "deep-link to this exact
  camera + selection + filter" URL state.

---

## 11. Security model for the webapp/dapp (it's on Citrate — treat it like a target)

Memrizz is a privileged window onto the org's entire memory; it must meet the same
bar SECREM-02 set for the federation. The designer needs to know these because they
shape flows:

- **Fail closed** on every auth/scope decision (the recurring federation finding
  class). Missing OIDC config → reject. No grant → no data. (Mirrors FUA-EXPLORER-01 /
  FUA-DASH-01 / the chatbot fixes.)
- **The BYOM MCP endpoint is an unauthenticated-control-surface risk if done wrong** —
  it MUST be token-gated, scoped to the user's grant, rate-limited, and audited (the
  node-agent/identity-SSE lessons). Short-lived tokens, per-client revoke.
- **Attenuation + cascade** enforced server-side in `mem-gateway`, not just in the UI
  (the UI prevents over-granting; the backend re-checks — defense in depth).
- **Every mutation audited**; the audit chain is surfaced *to users* as a feature.
- **Crypto-shred is irreversible** and super-admin-only with double confirmation +
  audit.
- **No secrets in the client.** OIDC is PKCE public-client; the gateway holds nothing
  the browser shouldn't. Tokens out of localStorage (CSP, the FUA-EXPLORER-04 lesson).
- **This webapp/dapp gets its own Rule-8 review + folds into the MEM-S6 WP-6.1 Tier-1
  audit** (already queued as citrate-security issue #6 — add the gateway + BYOM
  surface to its scope).

---

## 12. Open-source & Agentile alignment

- Ships under the federation's Apache-2.0, Agentile-compliant: Rule-12 frontmatter on
  docs, the test/spec/tripwire/frontmatter ratchets, no mocks/stubs in prod paths
  (Rule 11 — every UI element traces to a real engine capability, no fake data).
- The Docker bundle (gateway + app) is the **self-hostable OSS artifact** — any org
  points it at their own repos + their own citrate-identity (or any OIDC issuer behind
  the seam) and gets their own memory constellation. This is the MEM-S6 WP-6.4 OSS
  story made concrete.
- **The demo path:** in-house demo on the live 9,567-node federation graph →
  harden via the Tier-1 audit → OSS release with the visibility flip (WP-6.4 ADR + sign-offs).

---

## 13. What I'm handing the designer vs. what I build after

**For the designer (now):** this doc. It defines *what must exist*, *what each thing
means*, the auth/RBAC reality, the visualization semantics + performance budget, the
screen map, the component inventory, and the states. **UX/UI is theirs** — layout,
exact hues (within the meaning→channel mappings), typography, motion choreography, the
HTML prototype. The `⟦DECIDE⟧` items are flagged for Saul.

**For me (in parallel + after their prototype):**
1. **Build `mem-gateway`** (the backend seam) — **starting now**, independent of the
   visual design; it's the contract the prototype wires against. Includes from M0:
   **Org isolation** (per-Org engine instance routing + a control-plane store for
   Org/membership/role), OIDC auth + Org resolution, the REST/JSON read API over the
   engine, the audit/authz gate (Org boundary first, then CapabilityGrant scope), SSE
   deltas, the server-side UMAP/PCA layout endpoint, and (M1) the MCP-over-HTTP BYOM
   endpoint + the signing path for assert/confirm.
2. **Build F-5 + F-7** (identity-bound principals + delegation-revocation cascade) —
   the backend prerequisites the RBAC needs; Memrizz is their trigger.
3. **Wire the prototype** pixel-perfect + responsive against the gateway once the
   on-brand HTML lands.
4. **Rule-8 review + fold into the Tier-1 audit** before any non-local exposure.

### Phasing (MVP → full) — **decided 2026-06-13: M1 = See + Ask + Steward**

- **M0 (backend seam):** `mem-gateway` with **Org isolation from day one** + OIDC auth
  + Org resolution + read JSON API + the layout endpoint + the audit/authz gate. No UI.
  *(Starting now, in parallel with design.)*
- **M1 (See + Ask + Steward — the demoable MVP):**
  - **See:** Constellation (Galaxy + Lattice modes, Lite/Full), Node Inspector/Verify,
    Search/Recall.
  - **Ask:** conversational RAG with model picker + citations↔graph.
  - **Steward:** HITL Review Center (proposals · contradictions · supersession ·
    self-critic) + Assert + time-travel scrubber.
  - All strictly Org-scoped. This is the in-house demo — it shows the *whole trust
    story* (the quarantine→confirm HITL loop), not just read-only wow.
- **M2 (Operate):** Admin console (Org people/grants/delegation tree + Org
  provisioning), BYOM connect, audit viewer, ops (checkpoints/sync/anchor),
  memory-diff import/export.
- **M3 (Platform + Forget):** Platform-Operator surface (Org lifecycle), crypto-shred,
  Org billing/quota seams.
- **M4 (Polish + OSS):** federation islands + storyline-river modes, the full shader
  pass, accessibility hardening, the Tier-1 audit + OSS release.

> Note: M1 now includes the HITL Steward surface, so **F-5 (identity-bound principals)
> and the signing path for `assert`/`confirm` are M1 backend prerequisites**, not M2.
> The delegation-revocation cascade (F-7) can trail to M2 with the admin console.

---

## Appendix A — Capability → surface traceability (nothing dropped)

| Engine capability (tool/feature) | Primary surface(s) |
|---|---|
| `recall` (storyline) | Storyline view, Ask, omnibox |
| `search` (bge semantic) | Omnibox, Ask, Constellation search-to-fly |
| `neighbors` (blast-radius) | Node Inspector, Constellation focus |
| `as_of` (decision-replay) | Time scrubber, decision-replay mode |
| `verify` (provenance) | Node Inspector / VerifyVerdict |
| `critique` (self-critic) | Ask completeness heads-up, Review Center |
| `analogy` (cross-tenant) | Constellation analogy beams, Inspector "find analogues" |
| `propose_edge` / quarantine | Inspector action + Review Center proposals queue |
| `confirm_edge` (promote) | Review Center confirm (HITL) |
| `assert` (signed node) | Add-a-memory form |
| `merge_diff` (handoff) | Handoff import/export + DiffPreview |
| Two-plane (Derived/Asserted) | PlaneBadge everywhere; color/material in 3D |
| Trust tiers | TrustChip; glow/bloom in 3D |
| Belnap `Both` (contradiction) | ContradictionFlag; glitch shader; Review queue |
| Status (Active/Superseded/Archived) | StatusPill; desaturation/ghost-trail in 3D |
| Freshness watermark | FreshnessStamp; stale banner |
| CapabilityGrant / ResourceScope / PolicyProfile | Admin grants, ScopePicker, RoleBadge |
| DelegationStep (parent-child) | DelegationTree; cascade-aware revoke |
| AuditChain (hash-chained) | Audit Log Viewer; ToastWithAuditSeq |
| Federation meta-graph (Tenant + DependsOn) | Org overview, Islands mode |
| CRDT sync / merge | Ops sync status |
| Merkle + chain anchoring (40204) | Ops anchor status ("notarized at block N") |
| Crypto-shred (forget) | /admin/forget (super admin) |
| Rolling checkpoints (WP-6.5) | Ops checkpoint status + recovery |
| Multi-session daemon | presence; concurrent users |
| BYOM (MCP-over-HTTP) | /connect |
| Embeddings (bge) + HNSW | powers search + the Semantic Galaxy layout (server-side) |
