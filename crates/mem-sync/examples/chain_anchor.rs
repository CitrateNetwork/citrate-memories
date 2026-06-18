//! WP-5.3 operator tool: bind a tenant's merkle root to the live citrate-chain
//! head, or read back the latest binding.
//!
//! ```bash
//! # anchor citrate-chain's root to the live chain (default RPC + chainId 40204)
//! cargo run -q -p mem-sync --example chain_anchor --features rocksdb,chain -- \
//!     anchor ./data/federation.bge.enc.memdag citrate-chain
//!
//! # read the latest chain-anchor binding
//! cargo run -q -p mem-sync --example chain_anchor --features rocksdb,chain -- \
//!     show ./data/federation.bge.enc.memdag citrate-chain
//! ```
//!
//! Override the endpoint with `CITRATE_RPC_URL` / `CITRATE_CHAIN_ID`.

use mem_store::MemoryDagStore;
use mem_sync::chain::{anchor_tenant_to_chain, latest_chain_anchor};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rpc_url() -> String {
    std::env::var("CITRATE_RPC_URL").unwrap_or_else(|_| "https://rpc.citrate.ai".to_string())
}

fn chain_id() -> u64 {
    std::env::var("CITRATE_CHAIN_ID").ok().and_then(|s| s.parse().ok()).unwrap_or(40204)
}

fn open(db: &str) -> MemoryDagStore<mem_core::MemoryNode> {
    MemoryDagStore::open_rocksdb_auto(db).expect("open store")
}

fn usage() -> ! {
    eprintln!("usage: chain_anchor <anchor|show> <db> <tenant>");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parts: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match parts.as_slice() {
        ["anchor", db, tenant] => {
            let store = open(db);
            match anchor_tenant_to_chain(&store, tenant, &rpc_url(), chain_id(), now_ms()) {
                Ok(rec) => eprintln!(
                    "chain-anchored '{tenant}': root {}… pinned to {} block #{} ({}…)",
                    &rec.anchor.root[..12],
                    rec.checkpoint.chain_id,
                    rec.checkpoint.block_number,
                    &rec.checkpoint.block_hash[..rec.checkpoint.block_hash.len().min(12)],
                ),
                Err(e) => {
                    eprintln!("chain anchor FAILED (fail-closed, nothing written): {e}");
                    std::process::exit(1);
                }
            }
        }
        ["show", db, tenant] => {
            let store = open(db);
            match latest_chain_anchor(&store, tenant).expect("read chain anchor") {
                None => {
                    eprintln!("'{tenant}' has no chain anchor yet");
                    std::process::exit(3);
                }
                Some(rec) => eprintln!(
                    "'{tenant}': root {}… @ chain {} block #{} ({}…), anchored {}ms",
                    &rec.anchor.root[..12],
                    rec.checkpoint.chain_id,
                    rec.checkpoint.block_number,
                    &rec.checkpoint.block_hash[..rec.checkpoint.block_hash.len().min(12)],
                    rec.checkpoint.fetched_at_ms,
                ),
            }
        }
        _ => usage(),
    }
}
