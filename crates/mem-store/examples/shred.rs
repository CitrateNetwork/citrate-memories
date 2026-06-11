//! Crypto-shred a tenant (WP-1.6 operator tool): destroy its key material so
//! every sealed payload of that tenant reads as forgotten. The ciphertext rows
//! stay (grow-only store); a later re-ingest of the same tenant mints a fresh
//! key generation.
//!
//!   cargo run --release -p mem-store --example shred --features rocksdb -- \
//!       ./data/federation.bge.enc.memdag <tenant>
//!
//! Stop the MCP daemon first — it holds the DB lock.

use mem_core::MemoryNode;
use mem_store::MemoryDagStore;

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.enc.memdag".into());
    let Some(tenant) = std::env::args().nth(2) else {
        eprintln!("usage: shred <db-path> <tenant>");
        std::process::exit(2);
    };
    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("shred: cannot open {db} (daemon still holding the lock?): {e}");
            std::process::exit(1);
        }
    };
    match store.shred_tenant(&tenant) {
        Ok(true) => eprintln!("shred: destroyed live key material for tenant '{tenant}' — its sealed nodes are now forgotten"),
        Ok(false) => eprintln!("shred: tenant '{tenant}' had no live key (never written, or already shredded)"),
        Err(e) => {
            eprintln!("shred: {e}");
            std::process::exit(1);
        }
    }
}
