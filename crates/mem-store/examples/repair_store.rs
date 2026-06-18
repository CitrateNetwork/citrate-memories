//! Repair a damaged RocksDB memory store whose MANIFEST went stale (e.g. a
//! daemon killed mid-compaction left it referencing a since-deleted SST).
//!
//! Rebuilds the catalog from the SST files present, then re-opens (in auto mode)
//! to confirm the store is usable and reports node/edge counts. BACK UP the
//! directory first — this rewrites the MANIFEST in place.
//!
//! ```bash
//! cargo run -q -p mem-store --example repair_store --features rocksdb -- \
//!     ./data/federation.bge.enc.memdag
//! ```

use mem_store::rocks::RocksKv;
use mem_store::MemoryDagStore;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: repair_store <db-path>");
            std::process::exit(2);
        }
    };

    eprintln!("repair: rebuilding MANIFEST/catalog for {path} …");
    if let Err(e) = RocksKv::repair(&path) {
        eprintln!("repair FAILED: {e}");
        std::process::exit(1);
    }
    eprintln!("repair: catalog rebuilt. Re-opening to verify …");

    match MemoryDagStore::<mem_core::MemoryNode>::open_rocksdb_auto(&path) {
        Ok(store) => {
            let nodes = store.node_count().map(|n| n.to_string()).unwrap_or_else(|e| format!("?({e})"));
            let edges = store.edge_count().map(|n| n.to_string()).unwrap_or_else(|e| format!("?({e})"));
            let encrypted = store.is_encrypted_at_rest();
            eprintln!("repair: OK — store opens. nodes={nodes} edges={edges} encrypted_at_rest={encrypted}");
        }
        Err(e) => {
            eprintln!("repair: catalog rebuilt but the store still won't open: {e}");
            eprintln!("        → the data is not recoverable by repair; rebuild from the repos with `backfill`.");
            std::process::exit(1);
        }
    }
}
