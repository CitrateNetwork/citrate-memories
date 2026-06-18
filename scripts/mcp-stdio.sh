#!/usr/bin/env bash
# Launch the citrate-memories MCP stdio server for an MCP client (e.g. Claude Code).
#
# The live federation RocksDB store is held open (single-writer) by the running
# `mem-gateway` service that fronts mem-gateway.citrate.ai. RocksDB does not allow
# a second primary to open the same dir, so this launcher reads from a SNAPSHOT
# copy instead — the gateway keeps serving, untouched.
#
# The snapshot is refreshed on each launch (the graph is read-mostly; a few-second-
# stale read is fine, and the tools carry freshness/provenance anyway). To force a
# refresh between launches, just delete $SNAP.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIVE="$REPO/data/federation.bge.memdag"
SNAP="${MEM_MCP_SNAPSHOT:-/tmp/mem-snapshot.memdag}"
BIN="$REPO/target/release/examples/mcp_stdio"

[ -x "$BIN" ] || { echo "mcp-stdio: missing $BIN — build with: cargo build -p mem-mcp --example mcp_stdio --features rocksdb,transformer --release" >&2; exit 1; }

# (Re)take the snapshot from the live store, then drop the copied LOCK so we can open it.
rm -rf "$SNAP"
cp -r "$LIVE" "$SNAP"
rm -f "$SNAP/LOCK"

exec "$BIN" "$SNAP"
