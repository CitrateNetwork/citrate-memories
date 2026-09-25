# mem-gateway

The Memrizz backend seam — turns the headless citrate-memories engine into a
multi-Org SaaS surface. Spec: `../../PLANSET/06_WEBAPP_FRONTEND_SPEC.md`.

## Status: M0 core landed (security-critical foundation)

Built + tested (19 tests, clippy `-D warnings` clean on default + `rocksdb`):

- **Org control-plane** (`org.rs`) — `Org`, `Membership`, `Role` (Member / OrgAdmin /
  OrgOwner), and `ControlPlane` (the source of truth for who's in which Org). Includes
  the **delegation tree** (parent links) and the **revocation cascade**
  (`revoke_cascade` — removes a member + everyone delegated beneath them, F-7).
- **Authz gate** (`authz.rs`) — `authorize()` checks the **Org boundary first**
  (membership), then mints a **signed `CapabilityGrant`** bound to the OIDC `sub`
  (`derive_grant`, F-5) and runs the engine's audited scope check. `can_delegate()`
  enforces **attenuation** (a child can't exceed its delegator's scopes or role).
- **Engine registry** (`registry.rs`) — `OrgEngines` routes each Org to its **own
  isolated store** (proven by a collision test: two Orgs with the same repo name see
  only their own data). `open_org` (feature `rocksdb`) lazily opens per-Org stores.

## Status: HTTP read API + 3D-layout endpoint landed (constellation prototype unblocked)

Built + tested (25 tests, clippy `-D warnings` clean on default + `rocksdb` + `server`).

- **`http.rs`** (feature `server`) — axum read API, **fail-closed auth** (dev-auth via
  `x-dev-sub` behind `MEM_GATEWAY_ALLOW_DEV_AUTH=1`; OIDC bearer verification is the
  M1 wiring — until then every request is refused). Every route is Org-scoped and
  passes the core gate (Org boundary → tenant scope). Endpoints: `health`, `orgs`,
  `tenants`, `recall`, `search`, `as_of`, `nodes/:id`, `verify`, `neighbors`,
  `analogy`, **`layout`**.
- **`layout.rs`** — deterministic **PCA-to-3D** projection of the bge embeddings
  (axes from a strided sample, all nodes projected) + the scene assembler (position +
  material lane + plane + trust + status + contradiction + degree). Cached per Org by
  node-count.
- **`bin/mem-gateway`** (feature `server,rocksdb[,transformer]`) — the dev server;
  opens a real Org store, **warms the layout cache at startup**, serves.

### Run the dev server against the real graph

```bash
MEM_GATEWAY_ALLOW_DEV_AUTH=1 \
cargo run --release -p mem-gateway --bin mem-gateway --features server,rocksdb,transformer -- \
    --org dev-org --store ./data/federation.bge.enc.memdag --bind 127.0.0.1:8799

# the constellation scene (positions + visual attrs + edges) for the whole graph:
curl -s -H 'x-dev-sub: dev' http://127.0.0.1:8799/api/orgs/dev-org/layout | jq '.node_count, .edge_count'
curl -s -H 'x-dev-sub: dev' 'http://127.0.0.1:8799/api/orgs/dev-org/tenants/citrate-chain/recall?budget=5'
```

Verified live on the 9,564-node federation store: layout returns the full scene
(9,564 nodes / 5,684 edges, balanced PCA axis variance), **served in ~140ms** once
warmed; `recall`/`verify`/`neighbors`/`analogy` return real data; no-auth → 401, a
non-member → 403 (fail closed). **Cold layout build ≈ 23s** (one-time, hidden behind
the startup warm) — dominated by per-node edge I/O over the encrypted store, **not**
PCA; the optimization (single EDGES_OUT scan instead of N per-node lookups) is a noted
follow-up, not a blocker.

## Next (M1)

1. **SSE deltas** (`/stream`) — live graph + audit tail + notifications.
2. **MCP-over-HTTP (Streamable-HTTP)** BYOM endpoint, scoped per the same gate.
3. **Real OIDC bearer verification** (citrate-identity JWKS) replacing dev-auth.
4. **Control-plane persistence** (RocksDB control CF / Postgres) behind `ControlPlane`.
5. **assert/confirm/propose write path** (M1 Steward) + finish F-5 signing wiring.
6. **Layout cold-build speedup** (bulk edge scan) + per-member scene filtering.

## Intended HTTP contract (sketch — for the Next.js prototype to wire against)

All routes are **Org-scoped** and require an authenticated session (OIDC); the gateway
resolves Org + membership and audits every call. `:org` is the OrgId; `:tenant` is a
repo-tenant within it.

```
GET  /api/orgs                                  # orgs the caller belongs to
GET  /api/orgs/:org/tenants                     # repo-tenants the caller can read
GET  /api/orgs/:org/tenants/:tenant/recall?budget=
GET  /api/orgs/:org/tenants/:tenant/search?q=&budget=
GET  /api/orgs/:org/tenants/:tenant/as_of?t=&budget=
GET  /api/orgs/:org/nodes/:id                   # node inspector
GET  /api/orgs/:org/nodes/:id/verify            # provenance / trustworthiness
GET  /api/orgs/:org/nodes/:id/neighbors?budget=
GET  /api/orgs/:org/analogy?id=&budget=
POST /api/orgs/:org/tenants/:tenant/assert      # signed assertion        (write)
POST /api/orgs/:org/edges/propose               # quarantined proposal    (write)
POST /api/orgs/:org/edges/confirm               # HIC promote             (admin)
GET  /api/orgs/:org/review                       # HIC queues  (proposals/contradictions/...)
GET  /api/orgs/:org/layout                       # cached 3D positions (UMAP/PCA) for the constellation
GET  /api/orgs/:org/audit                        # hash-chained audit (integrity-verified)
GET  /api/orgs/:org/stream                        # SSE: graph deltas + audit tail + notifications
POST /api/orgs/:org/mcp                           # MCP-over-HTTP (BYOM), token-scoped to the caller's grant
--- admin ---
POST /api/orgs/:org/members                      # onboard (attenuation-checked)
DELETE /api/orgs/:org/members/:sub               # revoke (cascades)
GET  /api/orgs/:org/grants                        # delegation tree
--- platform operator (no memory content) ---
POST /api/platform/orgs                          # provision an Org
POST /api/platform/orgs/:org/suspend
```

Every mutating route fails **closed** and writes to the Org's audit chain.
