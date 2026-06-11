//! Ingest the federation meta-graph (MEM-S4 WP-4.4) into an existing store:
//! one Tenant node per `[repos.*]` manifest entry + DependsOn edges from the
//! `[[drift]]` map, all in the reserved `federation` tenant.
//!
//!   cargo run --release -p mem-ingest --example meta_ingest --features rocksdb[,transformer] -- \
//!       ./data/federation.bge.enc.memdag ../citrate-federation/manifest.toml
//!
//! The query embedder is auto-detected from the store (bge needs the
//! `transformer` feature) so the meta nodes land in the same vector space as
//! everything else. Stop the MCP daemon first — it holds the DB lock.

use mem_core::MemoryNode;
use mem_ingest::federation::ingest_federation_meta;
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
    let manifest = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "../citrate-federation/manifest.toml".into());

    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("meta_ingest: cannot open {db} (daemon still holding the lock?): {e}");
            std::process::exit(1);
        }
    };

    let embedder: Box<dyn Embedder> = match store_model(&store).as_deref() {
        Some("bge-base-en-v1.5") => {
            #[cfg(feature = "transformer")]
            {
                eprintln!("meta_ingest: store is bge-embedded, loading transformer…");
                match mem_index::TransformerEmbedder::bge_base() {
                    Ok(e) => Box::new(e),
                    Err(e) => {
                        eprintln!("meta_ingest: failed to load bge: {e}");
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(feature = "transformer"))]
            {
                eprintln!("meta_ingest: store is bge-embedded; rebuild with --features rocksdb,transformer");
                std::process::exit(2);
            }
        }
        _ => Box::new(HashingEmbedder::new(mem_ingest::EMBED_DIM)),
    };

    match ingest_federation_meta(std::path::Path::new(&manifest), &store, embedder.as_ref(), now_ms()) {
        Ok(r) => eprintln!(
            "meta_ingest: {} tenants, {} DependsOn edges ({} skipped — undeclared endpoint), head {}…",
            r.tenants,
            r.depends_on,
            r.skipped_deps,
            &r.watermark.head.unwrap_or_default()[..12]
        ),
        Err(e) => {
            eprintln!("meta_ingest: {e}");
            std::process::exit(1);
        }
    }
}
