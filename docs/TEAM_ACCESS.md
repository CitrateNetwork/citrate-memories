---
title: Citrate Memories — Team Access (entitlement, allowlist, agent setup)
created: 2026-09-28
branch: main
author: Larry Klosowski (with Claude Opus 4.8)
status: active
---

# Team access: how people get Citrate Memories

Access follows **identity**, not keys. An operator adds a teammate's email (or
wallet) to one allowlist; the teammate signs in with their Citrate account and is
**auto-provisioned** — no token is minted, copied, or pasted by anyone.

There are two layers, and both are keyed off the same login:

1. **Identity (`auth.citrate.ai`)** — who can sign in at all. Everyone uses their
   Citrate account (email + password with a verified-email step, passkey, or
   wallet).
2. **Authorization (this gateway)** — who is a member of the `citrate-federation`
   org and what they may read/write. This is the **team allowlist** below.

## Granting a teammate (the one thing an operator does)

Edit the live allowlist on the gateway host:

```
$MEM_TEAM_ALLOWLIST        # production: /var/lib/memrizz/team-allowlist.json  (0600, NOT in git)
```

Add an entry (schema in [`deploy/team-allowlist.example.json`](../deploy/team-allowlist.example.json)):

```json
{
  "name": "Jordan — Acme Robotics",
  "email": "jordan@acme.example",
  "role": "ReadOnly",
  "scopes": [ { "resource_id": "*", "can_read": true } ]
}
```

That's the whole grant. No restart is needed — the gateway re-reads the file the
next time an unprovisioned person authenticates. The teammate then signs in
(browser at https://memrizz.citrate.ai, or via their agent) and is provisioned on
first call. To confirm:

```bash
sudo journalctl -u mem-gateway -f | grep JIT-provisioned   # watch for their sub
```

### Roles

| Role | Grants |
| --- | --- |
| `OrgOwner` | Implicit `*` read+write over the whole org (no `scopes` needed). |
| `Member` | Exactly the `scopes` you list (read and/or write per resource). |
| `ReadOnly` | The `scopes` you list, with **write forced off** — read-only. |

`resource_id` is a repo tenant (e.g. `citrate-chain`) or `*` for all federation
knowledge. Give consumers `ReadOnly` + `{ "*": read }`. Grant `can_write`
(propose/assert) only to contributors, per repo.

### Matching rules (security)

- An entry matches on **verified email** (case-insensitive) **or** wallet address
  (lowercased). An **unverified** email never matches.
- The identity a person logs in with becomes an opaque `sub`; the allowlist maps
  their human identifier → that `sub` the first time, and the membership persists
  in `control.json` keyed by `sub`. Editing the allowlist later does not retroact
  on someone already provisioned — change their row in `control.json` (or remove
  the membership) to adjust an existing member.
- Fail-closed: a missing/broken allowlist provisions nobody; an unlisted person
  gets `403 no membership in org`.

### Removing / changing an existing member

The allowlist provisions on first login only. To revoke or change someone already
provisioned, edit `control.json` (`$MEM_GATEWAY_CONTROL_PATH`, production
`/var/lib/memrizz/control.json`): remove their membership object, or change its
`role`/`scopes`. Also remove them from the allowlist so they aren't re-provisioned
on their next login.

## How a teammate connects

Point them at [`clients/README.md`](../clients/README.md) and let them pick:

- **Browser (no install)** — https://memrizz.citrate.ai. Best for non-technical
  use. [`clients/browser/README.md`](../clients/browser/README.md)
- **Claude Code** — the `citrate` plugin marketplace + `/citrate-memories-login`.
- **Codex** — [`clients/codex/README.md`](../clients/codex/README.md)
- **Cursor / any MCP client** — [`clients/cursor/README.md`](../clients/cursor/README.md)

For agent paths, the person signs in **once** with a browser login at the
identity layer (`python3 mcp-connector.py login`, which the agent can run for
them). The connector stores a rotating credential locally and mints fresh
identity tokens per call — there is no long-lived key on disk to leak, and
nothing to paste.

### Non-technical members

They never touch a terminal. Either:
- use the **browser** path (zero install), or
- tell their agent **"set up Citrate Memories and sign me in"** — the agent runs
  the one-time login; they just complete the sign-in in the browser tab.

## How it works under the hood (for maintainers)

- **Connector** (`scripts/mcp-connector.py`, served at
  `https://mem-gateway.citrate.ai/connector.py`): a single stdlib-Python file. In
  `login` mode it runs an RFC 8252 loopback PKCE flow against `auth.citrate.ai`
  using the first-party `memrizz` client (already registered with a
  `http://127.0.0.1:3000/auth/callback` loopback redirect) and stores the refresh
  token under `~/.citrate/memories-auth.json` (0600). In bridge mode it refreshes
  an id_token as needed and POSTs each MCP JSON-RPC line to `/mcp/u/:sub` with
  `Authorization: Bearer <id_token>`.
- **Gateway** (`crates/mem-gateway`): the OIDC verifier reads `email` /
  `email_verified` / `wallet_address` in addition to `sub`; `gate()` (REST) and
  `byom()` (`/mcp/u/:sub`) both **JIT-provision** an unprovisioned caller from the
  allowlist (`crates/mem-gateway/src/allowlist.rs`), then persist the membership.
  `byom` accepts **either** an OIDC id_token (full membership authority) **or** an
  HS256 connect token (attenuated — the legacy webapp path). Everything is
  fail-closed and audited to the gateway audit chain.
- **Env** (systemd `mem-gateway.service`): `MEM_TEAM_ALLOWLIST` points at the live
  allowlist; unset ⇒ JIT off (unknown subs 403 as before).

## Server-side / headless agents (e.g. Hermes) — planned

The browser loopback login needs a local browser, so it does not fit an agent
running on a remote host with no access to the user's browser. The planned path
is an **RFC 8628 device-code flow** added to `citrate-identity`: the agent shows
"visit citrate.ai/device and enter code WXYZ," the user approves on their phone,
the agent polls for the token. Until that lands, a headless agent uses an
operator-issued connect token (the existing `/connect/token` path) as an interim.
This is tracked as a fast-follow.
