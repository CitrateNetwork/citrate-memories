# Memrizz backend deploy — mem-gateway + inference

Operator runbook for `handoffs/MEMRIZZ_BACKEND_DEPLOY_HANDOFF_2026-06-15.md`.
Two services unlock the (already-deployed) Memrizz webapp:

1. **mem-gateway** — the Rust memory API (this crate's `mem-gateway` binary).
2. **inference credential** — a `cgk_` key + model id for `infer.citrate.ai/v1`.

## Topology (why it differs from the handoff's `127.0.0.1`)

The federation store (RocksDB) and the bge embedder live on the **DGX**
(`spark-2e01`), so mem-gateway runs there, bound to the DGX Tailscale IP
`100.68.173.64:8799`. Public TLS for `mem-gateway.citrate.ai` is terminated by
**Caddy on the droplet** (`citrate-rpc-1`, 142.93.58.145), which reverse-proxies
over Tailscale — identical to how `infer.citrate.ai` fronts the DGX. The webapp's
Vercel BFF calls the gateway server-to-server (no CORS, no browser access).

```
Vercel BFF ──HTTPS──▶ mem-gateway.citrate.ai (Cloudflare ▶ droplet Caddy)
                              │ reverse_proxy over Tailscale
                              ▼
                     DGX 100.68.173.64:8799  (mem-gateway, OIDC fail-closed)
                              │
                     federation.bge.memdag (RocksDB) + bge embedder
```

## 1. Build (done — on the DGX)

```bash
cd citrate-memories
cargo build -p mem-gateway --bin mem-gateway --release --features server,rocksdb,transformer
# -> target/release/mem-gateway
```

## 2. Store

The store is rebuilt from the cloned federation repos:

```bash
cargo run -p mem-ingest --example backfill --release --features rocksdb,transformer -- \
  /home/saul/Projects/Citrate-Labs \
  ./data/federation.bge.memdag \
  bge
```

This is a **plaintext** bge store. For encryption-at-rest (handoff security
note), convert it with the `reencrypt` example before go-live and point `--store`
at the `.enc.memdag` result (the binary's `open_rocksdb_auto` then opens it
encrypted automatically). The owner controls the keyring seed.

## 3. Secrets + JWKS

```bash
sudo mkdir -p /etc/memrizz /var/lib/memrizz
curl -s https://auth.citrate.ai/jwks -o /etc/memrizz/jwks.json   # re-fetch on key rotation

sudo install -m 600 /dev/stdin /etc/memrizz/mem-gateway.env <<'EOF'
MEM_GATEWAY_ASSERTER_SEED=<generate a strong, stable secret>
MEM_CONNECT_SECRET=<MUST byte-match the value set on Vercel (memrizz)>
EOF
```

`MEM_CONNECT_SECRET` is already set on Vercel prod; copy that exact value here or
BYOM (`POST /mcp/u/:sub`) clients 401.

## 4. Bootstrap the founding Org Owner (REQUIRED)

With OIDC on, no memberships are seeded — a real user with no membership in
`citrate-federation` gets **403** on every Org route. Seed the owner once with
Saul's OIDC `sub` (from auth.citrate.ai / the avatar hover in Memrizz):

```bash
./target/release/mem-gateway --org citrate-federation \
  --store ./data/federation.bge.memdag \
  --bootstrap-owner '<SAUL_OIDC_SUB_VERBATIM>' --bind 127.0.0.1:8799
# it persists the OrgOwner membership to control.json, then serves; Ctrl-C and
# start under systemd, or just leave it. Additional members are added later from
# the webapp Admin console (or more --bootstrap-owner runs).
```

`--bootstrap-owner` is idempotent and also creates the Org row. (Equivalent to
hand-writing `control.json` per the handoff §1e.)

## 5. Run under systemd

`deploy/mem-gateway.service` → `/etc/systemd/system/`, then
`systemctl enable --now mem-gateway`. It runs OIDC-on (issuer/aud/JWKS in the
unit); dev-auth is ignored while OIDC is on.

## 6. Public hostname (droplet Caddy + Saul's Cloudflare)

- **Droplet:** append `deploy/Caddyfile.mem-gateway` to `/etc/caddy/Caddyfile`,
  `caddy validate` + `systemctl reload caddy`.
- **Saul → Cloudflare:** add `mem-gateway.citrate.ai` → the droplet
  (142.93.58.145), same proxy setting as `infer.citrate.ai`. (Separate record
  from `memrizz.citrate.ai`.)

## 7. Verify

```bash
# liveness (note: /api/health, not /health)
curl -s https://mem-gateway.citrate.ai/api/health        # {"ok":true,"service":"mem-gateway"}
# fail-closed: no bearer → 401 (not 404, not 200)
curl -s -o /dev/null -w '%{http_code}\n' https://mem-gateway.citrate.ai/api/orgs/citrate-federation/layout
# with a real id_token from a logged-in session → 200, scene with node_count>0
curl -s https://mem-gateway.citrate.ai/api/orgs/citrate-federation/layout -H "Authorization: Bearer <id_token>" | jq .node_count
```

> Blocked on Saul's `citrate-identity` redeploy: until the `memrizz` OIDC
> client stops returning `invalid_client`, nobody can log in, so the `200`
> verification can't run end-to-end. The `/api/health` + `401` checks work now.

## Job 2 — inference credential

The `cgk_` key store is the **inference-gateway on the droplet** (RocksDB,
`citrate-inference-gateway-local-proxy.service`). Mint/list on the droplet, then
read the served model id:

```bash
# on the droplet (key-admin CLI of the gateway binary; see GATEWAY_APIKEY_HANDOFF.md)
#   <gateway-bin> create "memrizz"   # prints cgk_… ONCE — capture it
curl -s https://infer.citrate.ai/v1/models -H "Authorization: Bearer cgk_XXXX" | jq -r '.data[].id'
# the exact id string becomes MEMRIZZ_MODEL
```

## Last mile — set on Vercel (memrizz prod)

| Vercel env (production) | Value |
|---|---|
| `MEM_GATEWAY_ORIGIN` | `https://mem-gateway.citrate.ai` |
| `MEMRIZZ_INFERENCE_API_KEY` | the `cgk_…` key |
| `MEMRIZZ_MODEL` | the model id from `/v1/models` |

`MEM_GATEWAY_FORWARD_BEARER=1` and `MEMRIZZ_INFERENCE_URL` are already set.
`vercel env add <NAME> production` then `vercel deploy --prod`.

## Route contract note

`/api/health`, fail-closed auth, recall/search/neighbors/verify/review/assert and
the BYOM MCP endpoint are fully specified. The `layout` **scene JSON shape**
(`{node_count, nodes:[{id,x,y,kind,repo,title,status}], edges:[{from,to,kind}]}`,
PCA-2D projection) is this gateway's own definition — reconcile it against the
deployed webapp's BFF expectations (or the Memrizz source on the Mac Studio)
and adjust `src/scene.rs` if field names differ.
