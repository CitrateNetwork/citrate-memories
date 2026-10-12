# mem-gateway deploy — operator runbook (TEMPLATE)

Two things unlock the webapp/BFF against a running mem-gateway:

1. **mem-gateway** — the Rust memory API (this workspace's `mem-gateway` binary).
2. **inference credential** (optional) — an API key + model id for an
   OpenAI-compatible inference endpoint, if you wire LLM-backed features.

Fill every `<PLACEHOLDER>` for your own hosts. This runbook ships infra-neutral;
it does not assume any particular cloud, tailnet, or hostname.

## Topology

Terminate public TLS on an **edge host** (Caddy) and reverse-proxy to the
**backend host** where mem-gateway runs co-located with its RocksDB store and
embedder. If the backend is only reachable over a private network, proxy to its
private address.

```
Web BFF ──HTTPS──▶ <PUBLIC_HOSTNAME> (edge host, Caddy, public TLS)
                        │ reverse_proxy (optionally over a private net)
                        ▼
                 <BACKEND_ADDR>  (mem-gateway, OIDC fail-closed)
                        │
                 RocksDB store + embedder
```

## 1. Build (on the backend host)

```bash
cd citrate-memories
cargo build -p mem-gateway --bin mem-gateway --release --features server,rocksdb,transformer
# -> target/release/mem-gateway
```

## 2. Store

Build the store from your source repos with the ingest backfill example:

```bash
cargo run -p mem-ingest --example backfill --release --features rocksdb,transformer -- \
  <PATH_TO_REPOS> \
  ./data/federation.bge.memdag \
  bge
```

This produces a **plaintext** bge store. For encryption-at-rest, convert it with
the `reencrypt` example before go-live and point `--store` at the `.enc.memdag`
result (the binary's `open_rocksdb_auto` opens it encrypted automatically). The
operator controls the keyring seed (`CITRATE_MEM_STORE_KEY`).

## 3. Secrets + JWKS

```bash
sudo mkdir -p /etc/mem-gateway /var/lib/mem-gateway
curl -s <OIDC_ISSUER>/jwks -o /etc/mem-gateway/jwks.json   # re-fetch on key rotation

sudo install -m 600 /dev/stdin /etc/mem-gateway/mem-gateway.env <<'EOF'
MEM_GATEWAY_ASSERTER_SEED=<generate a strong, stable secret>
MEM_CONNECT_SECRET=<MUST byte-match the value your webapp/BFF uses>
EOF
```

If `MEM_CONNECT_SECRET` is set on the webapp side, copy that exact value here or
BYOM (`POST /mcp/u/:sub`) clients 401.

## 4. Bootstrap the founding Org Owner (REQUIRED)

With OIDC on, no memberships are seeded — a real user with no membership gets
**403** on every Org route. Seed the owner once with their OIDC `sub`:

```bash
./target/release/mem-gateway --org <ORG> \
  --store ./data/federation.bge.memdag \
  --bootstrap-owner '<OWNER_OIDC_SUB_VERBATIM>' --bind 127.0.0.1:8799
# persists the OrgOwner membership to control.json, then serves. Ctrl-C and
# start under systemd, or leave it. Add members later from the Admin console
# (or more --bootstrap-owner runs).
```

`--bootstrap-owner` is idempotent and also creates the Org row.

## 5. Run under systemd

Copy `deploy/mem-gateway.service` to `/etc/systemd/system/`, fill its
placeholders, then `systemctl enable --now mem-gateway`. It runs OIDC-on
(issuer/aud/JWKS in the unit); dev-auth is ignored while OIDC is on.

## 6. Public hostname (edge Caddy + DNS)

- **Edge host:** append `deploy/Caddyfile.mem-gateway` (placeholders filled) to
  `/etc/caddy/Caddyfile`, `caddy validate` + `systemctl reload caddy`.
- **DNS:** point `<PUBLIC_HOSTNAME>` at the edge host.

## 7. Verify

```bash
# liveness (note: /api/health, not /health)
curl -s https://<PUBLIC_HOSTNAME>/api/health        # {"ok":true,"service":"mem-gateway"}
# fail-closed: no bearer → 401 (not 404, not 200)
curl -s -o /dev/null -w '%{http_code}\n' https://<PUBLIC_HOSTNAME>/api/orgs/<ORG>/layout
# with a real id_token from a logged-in session → 200, scene with node_count>0
curl -s https://<PUBLIC_HOSTNAME>/api/orgs/<ORG>/layout -H "Authorization: Bearer <id_token>" | jq .node_count
```

## 8. Auto-ingest — keeping the graph current (MEM-S7)

There is **no separate cron/timer to install**: the poller ("Trigger 2" in
`PLANSET/06_AUTO_INGEST_FEED.md`) runs *inside* mem-gateway, because the gateway
is the store's single RocksDB writer. An external timer would either have to
stop the gateway to take the lock (recurring downtime) or duplicate this loop.

- **Reconciler (WP-7.4):** at startup and every `MEM_INGEST_RECONCILE_SECS`
  (default 300 s) it `git ls-remote`s each federation repo's `HEAD` + branch tips
  and compares them to the stored per-tenant `Watermark.head` / branch watermarks.
  Drifted repos are enqueued.
- **Repo set:** every repo already mirrored, every non-`archived` `[repos.*]`
  table in the federation manifest (read from the gateway's own
  `citrate-federation` mirror, or `MEM_INGEST_MANIFEST=<path>`; `off` disables),
  plus `MEM_INGEST_REPOS` (comma/space list). Names are validated against
  `MEM_ALLOWED_OWNER` (default `CitrateNetwork`) and a strict charset before
  reaching `git`. So a repo added to the manifest gets its first ingest within
  one sweep, with no webhook needed.
- **Drain (WP-7.3):** every 15 s the single writer coalesces the queue, then
  runs a **canonical pass** (default-branch `ingest_incremental` for every queued
  repo), then the in-flight **branch pass** (ADR-09). The branch pass can't delay
  default-branch freshness. Vectors already stored for a node id are reused
  (same model tag + dim), so re-deriving known commits/docs costs point reads,
  not embed calls.
- **Mirrors:** `MEM_INGEST_MIRROR_DIR` (default `~/.cache/mem-gateway/mirrors`),
  cloned from `MEM_INGEST_GIT_BASE` (default `git@github.com:CitrateNetwork`),
  hard-reset to `origin/HEAD`. Ingest never reads an operator's working clone.
- **Webhook (WP-7.2, optional, real-time):** `POST /webhook/github`, HMAC-verified
  with `MEM_INGEST_WEBHOOK_SECRET` (fail-closed when unset). This is optional:
  the reconciler alone bounds staleness to about one sweep plus the drain time.
- **Logs:** journald (`journalctl -u mem-gateway | grep -E 'reconcile|mem-ingest'`).
  Each canonical ingest logs `head <sha12>`. No secrets are logged.

**Freshness check (read-only, never touches the live lock):**

```bash
cargo build -p mem-ingest --example freshness --features rocksdb --release
SNAP=$(mktemp -d)/snap; cp -r ./data/federation.bge.memdag "$SNAP" && rm -f "$SNAP/LOCK"
./target/release/examples/freshness "$SNAP"     # repo<TAB>head<TAB>head_count<TAB>ingested_at_ms
git ls-remote git@github.com:CitrateNetwork/<repo>.git refs/heads/main   # compare
```

**Backup before maintenance:** stop the gateway (about 2 s), `cp -a` the store dir to
a timestamped sibling, then start it again. Never `cargo clean` or delete `target/`: the
unit's `ExecStart` is the dev build.

## Job 2 — inference credential (optional)

If you front an OpenAI-compatible inference endpoint, mint a key there and read
the served model id:

```bash
curl -s <INFERENCE_URL>/v1/models -H "Authorization: Bearer <API_KEY>" | jq -r '.data[].id'
# the exact id string becomes your model env
```

## Last mile — set on the webapp/BFF

| Env (production) | Value |
|---|---|
| `MEM_GATEWAY_ORIGIN` | `https://<PUBLIC_HOSTNAME>` |
| `<INFERENCE_API_KEY>` | your inference key (if used) |
| `<INFERENCE_MODEL>` | the model id from `/v1/models` (if used) |

## Route contract note

`/api/health`, fail-closed auth, recall/search/neighbors/verify/review/assert and
the BYOM MCP endpoint are fully specified. The `layout` **scene JSON shape**
(`{node_count, nodes:[{id,x,y,kind,repo,title,status}], edges:[{from,to,kind}]}`,
PCA-2D projection) is this gateway's own definition — reconcile it against your
webapp's BFF expectations and adjust `src/scene.rs` if field names differ.
