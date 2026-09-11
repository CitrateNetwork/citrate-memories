# citrate-memories

*Part of the **[Citrate Network](https://citrate.ai)** — own the means of computation. · [Docs](https://docs.citrate.ai) · [Run a node](https://citrate.ai/download) · [Contribute → free membership](https://github.com/CitrateNetwork/.github/blob/main/CONTRIBUTING.md)*

> A federated, MCP-accessible knowledge DAG — "git for agents" — that gives any agent or human the storyline and code-shape of a Citrate repo in seconds, and never lets them act on stale or contradicted information.

## What it is

`citrate-memories` is a content-addressed knowledge DAG with two planes:
**Derived** (deterministic, rebuildable ingestion of git/docs/chain state) and
**Asserted** (signed, append-only, non-deterministic LLM/human claims). It is
served three ways: a **local MCP daemon** (`mem-mcp`, over a Unix socket) that any
Claude Code / MCP client connects to, an **HTTP gateway** (`mem-gateway`,
OIDC-gated, org-scoped) for remote/web access, and the **Memrizz** webapp (a 2.5D
memory constellation with grounded RAG). Storage is per-tenant crypto-shredded at
rest; the gateway can anchor audit checkpoints to chain **40204**.

See the concept docs at https://docs.citrate.ai/memories. The gateway/webapp
authenticate against
[citrate-identity](https://github.com/CitrateNetwork/citrate-identity).

## Prerequisites

```bash
# Rust (stable) + a C toolchain for RocksDB.
rustup show
#   Debian/Ubuntu:
sudo apt-get install -y build-essential clang libclang-dev pkg-config
#   macOS: clang ships with Xcode command-line tools
# Webapp (Memrizz, optional): Node 20+ and pnpm/npm.
```

- OS: Linux or macOS.
- The `mem-mcp` daemon and RocksDB-backed builds require the `rocksdb` cargo
  feature (a native RocksDB link — hence clang/libclang).

## Build from source

```bash
git clone https://github.com/CitrateNetwork/citrate-memories.git
cd citrate-memories
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# The MCP daemon binary (RocksDB singleton — the rocksdb feature is REQUIRED):
cargo build -p mem-mcp --bin mem-mcp --release --features rocksdb
# The HTTP gateway:
cargo build -p mem-gateway --release
```

Expected artifacts: `target/release/mem-mcp` and `target/release/mem-gateway`.
First build pulls RocksDB and can take several minutes. Note: without
`--features rocksdb`, cargo reports "no bin target named `mem-mcp`" — the feature
is mandatory for the daemon.

## Run locally

**MCP daemon (the primary local surface).** One process owns the RocksDB lock and
serves any number of concurrent MCP sessions over a Unix socket
(`mem-mcp <store-path> <sock-path>`):

```bash
mkdir -p ./mem-data
./target/release/mem-mcp ./mem-data/store ./mem-data/mem.sock
```

There is no TCP port — clients connect through the Unix socket. Verify it's up:

```bash
test -S ./mem-data/mem.sock && echo "mem-mcp socket is live"
# Point an MCP client (e.g. Claude Code .mcp.json) at ./mem-data/mem.sock, or use
# the provided stdio shim:
./scripts/mcp-stdio.sh      # bridges stdio ↔ the daemon socket
```

**HTTP gateway (for the webapp / remote access).** Defaults to
`127.0.0.1:8799`:

```bash
./target/release/mem-gateway --org citrate-federation --store ./mem-data/store --bind 127.0.0.1:8799
```

Verify it's up (liveness is unauthenticated; org routes are fail-closed):

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8799/api/health          # → 200
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8799/api/orgs/citrate-federation/layout   # → 401 (fail-closed, correct)
```

**Webapp (Memrizz, optional):**

```bash
cd webapp
cp .env.example .env.local
pnpm install
pnpm dev                    # next dev on :3000
```

## Connect it locally  ← the differentiator

1. **Run the MCP daemon** (above) and point your MCP client's `.mcp.json` at the
   socket — this is the zero-dependency local path; no chain node or identity
   authority is needed for local MCP reads/writes.
2. **Run the gateway** on `127.0.0.1:8799` when you want the webapp or HTTP access.
   Its upstreams:
   - **Identity (OIDC).** Run
     [citrate-identity](https://github.com/CitrateNetwork/citrate-identity) on
     `:3000`; the gateway verifies OIDC JWTs against its JWKS. Org routes are
     fail-closed (401 without a valid token).
   - **Chain 40204 (optional).** External-root audit checkpoints bind to the live
     40204 head via read-only JSON-RPC (`MEM_CHAIN_RPC_URL`); omit to skip
     anchoring.
3. **Wire the webapp to the gateway + identity.** In `webapp/.env.local`:
   ```bash
   MEM_GATEWAY_ORIGIN=http://127.0.0.1:8799      # must match the gateway --bind
   NEXT_PUBLIC_AUTH_MODE=oidc                     # (use mock to skip the authority in dev)
   NEXT_PUBLIC_OIDC_ISSUER=http://localhost:3000
   NEXT_PUBLIC_OIDC_CLIENT_ID=memrizz
   OIDC_ISSUER=http://localhost:3000
   OIDC_JWKS_URL=http://localhost:3000/jwks
   OIDC_AUDIENCE=memrizz
   ```
4. **End-to-end check:** `GET http://127.0.0.1:8799/api/health` returns 200 and an
   org route returns 401 without a token; with a token minted by the local
   authority, `/api/orgs/citrate-federation/recall` returns grounded results.

For the full chain → identity → apps bring-up see https://docs.citrate.ai/local-stack.

## Configuration

MCP daemon:

| Var / arg | Default | Purpose |
|-----------|---------|---------|
| `mem-mcp <store-path> <sock-path>` | — | Positional: RocksDB store path + Unix socket path. |
| `CITRATE_MEM_STORE_KEY` | unset → ephemeral identity | Per-user wrapping key (hex) seeding the daemon's ed25519 identity; passed in env, never argv. |
| `MEM_CHECKPOINT_INTERVAL_SECS` / `MEM_CHECKPOINT_KEEP` | `1800` / `3` | Rolling RocksDB recovery checkpoints. |

Gateway:

| Var / arg | Default | Purpose |
|-----------|---------|---------|
| `--org` / `MEM_DEFAULT_ORG` | (built-in default) | Default org id. |
| `--store` / `MEM_DEFAULT_STORE` | (built-in default) | RocksDB store path. |
| `--bind` | `127.0.0.1:8799` | HTTP listen address. |
| `OIDC_ISSUER` / `OIDC_JWKS_URL` / `OIDC_AUDIENCE` | — | OIDC verification against the identity authority. |
| `MEM_INGEST_REPOS` / `MEM_INGEST_GIT_BASE` | unset | Repos the ingest worker mirrors into the Derived plane. |

## Links

- Docs: https://docs.citrate.ai/memories
- Depends on: [citrate-identity](https://github.com/CitrateNetwork/citrate-identity) (gateway/webapp login) · [citrate-chain](https://github.com/CitrateNetwork/citrate-chain) (optional audit anchoring)
- Consumed by: MCP clients (Claude Code and other agents), the Memrizz webapp, and citrate-core
- Contributing (DCO): CONTRIBUTING.md · Security: SECURITY.md · License: LICENSE

## License

Source-available (BUSL-1.1) — free for personal/non-commercial; commercial = membership.
