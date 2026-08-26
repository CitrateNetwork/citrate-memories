# citrate-memories

> A federated, MCP-accessible **knowledge DAG** that gives every agent (and human)
> the storyline and code-shape of any repository in seconds — without reading the
> code — and never lets them act on stale, contradicted, or unverified information.
> **"Git for agents."**

Apache-2.0. A memory server for AI agents: content-addressed, bitemporal, and
provenance-carrying, served over the Model Context Protocol.

## The core invariant

> **Derived = deterministic & rebuildable. Asserted = nondeterministic but signed
> & append-only. Any LLM/heuristic output is an _assertion_, never a _derivation_.**

Every answer carries how fresh it is and whether it has been superseded or
contradicted — so an agent can trust what it reads.

## What's in the box

```
crates/
├─ mem-core     # ontology, content-only identity, bitemporal nodes, Belnap confidence
├─ mem-store    # content-addressed DAG over a KvStore (RocksDB) with per-tenant
│               #   XChaCha20-Poly1305 crypto-shredding at rest
├─ mem-ingest   # git/docs/frontmatter ingestion into the Derived plane
├─ mem-index    # HNSW vector index + local embedding model
├─ mem-query    # budgeted recall, semantic search, blast-radius over the DAG
├─ mem-assert   # signed, append-only Asserted-plane records
├─ mem-authz    # capability grants + hash-chained audit log
├─ mem-sync     # federation merge (Belnap-CRDT) + tamper-evident anchoring
├─ mem-mcp      # MCP-accessible surface (the 11 memory.* tools)
└─ mem-gateway  # HTTP control-plane: OIDC/JWKS auth, signed writes, org/registry
```

Default builds are pure-Rust and network-free — native/heavy features are opt-in
via Cargo features: `rocksdb`, `transformer` (embeddings), `server` (HTTP gateway),
`chain`/`http` (optional anchoring). The core runs standalone: **no blockchain and
no identity provider required.**

## Quickstart

```bash
# Build the MCP daemon (RocksDB-backed store + local embeddings)
cargo build -p mem-mcp --bin mem-mcp --release --features rocksdb,transformer

# Ingest a repo into a fresh store
cargo run -p mem-ingest --example backfill --release --features rocksdb,transformer -- \
  /path/to/your/repos ./data/memory.memdag bge

# Serve it over MCP (Unix-socket daemon; positional <store> <socket>)
./target/release/mem-mcp ./data/memory.memdag ./data/memdag.sock
```

Point any MCP client (e.g. Claude Code) at the daemon; a stdio launcher for
single-user setups lives in `scripts/mcp-stdio.sh`.

## The MCP tool surface

Eleven tools, read and write:

| Tool | What it does |
|---|---|
| `memory.recall` | the storyline of a repo (budgeted) |
| `memory.search` | semantic search within a repo |
| `memory.neighbors` | graph neighbors of a node |
| `memory.as_of` | time-travel: the graph as of a point in time |
| `memory.verify` | is a node current, superseded, or contradicted? |
| `memory.critique` | self-critique over a claim |
| `memory.analogy` | analogous nodes across repos |
| `memory.assert` | write a signed, append-only assertion |
| `memory.merge_diff` | apply a memory-diff |
| `memory.propose_edge` / `memory.confirm_edge` | propose/confirm a relation |

The HTTP gateway exposes the same surface as REST (`/api/orgs/:org/{recall,search,
neighbors,verify,review,assert,layout}`) plus a BYOM MCP-over-HTTP endpoint.

## Build & test

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Clients

Typed clients for this gateway ship in the Citrate SDKs (JavaScript and Python):
a `memory` module over the REST surface plus a BYOM MCP passthrough.

---
© 2026 Citrate Inc. Licensed under Apache-2.0. See `LICENSE` and `NOTICE`.
