# mem-gateway

The Mnemosyne backend seam — turns the headless citrate-memories engine into a
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

## Next (M0 → M1)

1. **axum HTTP layer** over this core (tokio): the read API below + SSE deltas + the
   server-side UMAP/PCA layout endpoint.
2. **MCP-over-HTTP (Streamable-HTTP)** BYOM endpoint, scoped per the same gate.
3. **Control-plane persistence** (RocksDB control CF or Postgres) behind `ControlPlane`.
4. **assert/confirm signing path** (M1 Steward needs writes) + finish F-5 wiring.

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
POST /api/orgs/:org/edges/confirm               # HITL promote            (admin)
GET  /api/orgs/:org/review                       # HITL queues (proposals/contradictions/...)
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
