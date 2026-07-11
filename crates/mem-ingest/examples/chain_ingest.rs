//! Ingest chain-state (E-4 WP-2) into an existing store: network params + the
//! deployed-contract catalog from a pinned `40204.json`-shaped file, all in
//! the reserved `chain-state` tenant.
//!
//!   cargo run --release -p mem-ingest --example chain_ingest --features rocksdb[,transformer] -- \
//!       ./data/federation.bge.enc.memdag ../citrate-chain/contracts/addresses/40204.json
//!
//! Deterministic: the same catalog produces byte-identical node ids on every
//! machine, so re-running is a pure no-op (the report proves it — counts do
//! not grow). The query embedder is auto-detected from the store (bge needs
//! the `transformer` feature) so chain nodes land in the same vector space as
//! everything else. Stop the MCP daemon first — it holds the DB lock.
//!
//! The event walk (WP-3) is library-only for now: see
//! `mem_ingest::chain::ingest_chain_events` and the `chain` feature for the
//! live `RpcBlockSource`. Daemon mode is WP-4.

use mem_core::MemoryNode;
use mem_ingest::chain::ingest_chain_catalog;
use mem_index::{Embedder, HashingEmbedder};
use mem_store::MemoryDagStore;

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The embedding model the store's existing nodes use (first embedded node wins).
fn store_model(store: &MemoryDagStore<MemoryNode>) -> Option<String> {
    store.all_nodes().ok()?.into_iter().find_map(|n| n.embedding.map(|v| v.model))
}

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.memdag".into());
    let catalog = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "../citrate-chain/contracts/addresses/40204.json".into());

    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("chain_ingest: cannot open {db} (daemon still holding the lock?): {e}");
            std::process::exit(1);
        }
    };

    let embedder: Box<dyn Embedder> = match store_model(&store).as_deref() {
        Some("bge-base-en-v1.5") => {
            #[cfg(feature = "transformer")]
            {
                eprintln!("chain_ingest: store is bge-embedded, loading transformer…");
                match mem_index::TransformerEmbedder::bge_base() {
                    Ok(e) => Box::new(e),
                    Err(e) => {
                        eprintln!("chain_ingest: failed to load bge: {e}");
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(feature = "transformer"))]
            {
                eprintln!("chain_ingest: store is bge-embedded; rebuild with --features rocksdb,transformer");
                std::process::exit(2);
            }
        }
        _ => Box::new(HashingEmbedder::new(mem_ingest::EMBED_DIM)),
    };

    match ingest_chain_catalog(std::path::Path::new(&catalog), &store, embedder.as_ref(), now_ms())
    {
        Ok(r) => {
            println!(
                "chain-state ingest ok: chainId {} — {} contracts, {} redeploys superseded ({} rejected); watermark head {}",
                r.chain_id,
                r.contracts,
                r.superseded,
                r.supersessions_rejected,
                r.watermark.head.as_deref().unwrap_or("-"),
            );
        }
        Err(e) => {
            eprintln!("chain_ingest: {e}");
            std::process::exit(1);
        }
    }
}
