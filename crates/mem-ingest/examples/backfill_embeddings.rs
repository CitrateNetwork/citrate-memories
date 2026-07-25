//! Backfill embeddings onto nodes that were written without one.
//!
//! `Asserter::assert_node` used to hardcode `embedding: None`, so every node ever
//! written through `memory.assert` or `POST /assert` is invisible to
//! `Recall::search` (which indexes only nodes whose `embedding` is `Some`). The
//! write path is fixed; this repairs the nodes already in a store.
//!
//! **Why this is safe.** `MemoryNode::compute_id` covers
//! `{schema_version, plane, kind, repo, author, source_ref, content}` only.
//! `embedding` is not identity-bearing, so adding one rewrites the node **at the
//! same key** and cannot invalidate its signature. This tool re-checks that
//! invariant per node and aborts rather than risk writing a duplicate.
//!
//! The embedder is chosen from the store's OWN vectors, never a flag: mixing
//! models is worse than no vector, because the index's model guard silently skips
//! mismatched spaces and the node stays just as unfindable.
//!
//! Usage:
//!   cargo run -p mem-ingest --example backfill_embeddings --release \
//!     --features rocksdb,transformer -- <DB_PATH> [--apply]
//!
//! Dry-run by default: it reports what it would do and writes nothing. Pass
//! `--apply` to commit. **Stop the gateway first** (`sudo systemctl stop
//! mem-gateway`) or the RocksDB lock is held and the store will not open.

use std::collections::BTreeMap;

use mem_core::MemoryNode;
use mem_index::{Embedder, HashingEmbedder};
use mem_ingest::EMBED_DIM;
use mem_store::MemoryDagStore;

/// The embedding model this store's existing nodes were built with (first
/// embedded node wins). `None` means nothing in the store is embedded yet.
fn store_model(store: &MemoryDagStore<MemoryNode>) -> Option<String> {
    store.all_nodes().ok()?.into_iter().find_map(|n| n.embedding.map(|v| v.model))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let db = args.next().unwrap_or_else(|| {
        eprintln!("usage: backfill_embeddings <DB_PATH> [--apply]");
        std::process::exit(2);
    });
    let apply = args.any(|a| a == "--apply");

    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("backfill_embeddings: cannot open {db} (gateway still holding the lock?): {e}");
            std::process::exit(1);
        }
    };

    // Match the store's space, or do nothing. A wrong model is not a partial win.
    let embedder: Box<dyn Embedder> = match store_model(&store).as_deref() {
        Some("bge-base-en-v1.5") => {
            #[cfg(feature = "transformer")]
            {
                eprintln!("backfill_embeddings: store is bge-embedded, loading transformer…");
                match mem_index::TransformerEmbedder::bge_base() {
                    Ok(e) => Box::new(e),
                    Err(e) => {
                        eprintln!("backfill_embeddings: failed to load bge: {e}");
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(feature = "transformer"))]
            {
                eprintln!("backfill_embeddings: store is bge-embedded; rebuild with --features rocksdb,transformer");
                std::process::exit(2);
            }
        }
        Some(other) => {
            eprintln!("backfill_embeddings: unrecognised store model {other:?}; refusing to guess");
            std::process::exit(2);
        }
        None => {
            eprintln!("backfill_embeddings: no embedded node found; assuming the hashing default (d={EMBED_DIM})");
            Box::new(HashingEmbedder::new(EMBED_DIM))
        }
    };

    let all = match store.all_nodes() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("backfill_embeddings: scan failed: {e}");
            std::process::exit(1);
        }
    };
    let total = all.len();
    let targets: Vec<MemoryNode> = all.into_iter().filter(|n| n.embedding.is_none()).collect();

    let mut by_tenant: BTreeMap<String, usize> = BTreeMap::new();
    for n in &targets {
        *by_tenant.entry(n.repo.clone()).or_default() += 1;
    }
    eprintln!(
        "backfill_embeddings: {} of {total} nodes unembedded, across {} tenant(s)",
        targets.len(),
        by_tenant.len()
    );
    for (repo, count) in &by_tenant {
        eprintln!("  {repo:<28} {count}");
    }
    if targets.is_empty() {
        eprintln!("backfill_embeddings: nothing to do");
        return;
    }
    if !apply {
        eprintln!("backfill_embeddings: DRY RUN, nothing written. Re-run with --apply to commit.");
        return;
    }

    let (mut done, mut failed) = (0usize, 0usize);
    for mut node in targets {
        let before = node.compute_id();
        let text = String::from_utf8_lossy(&node.content).to_string();
        let vector = match embedder.embed(&text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  SKIP {} ({}): embed failed: {e}", &before.to_hex()[..12], node.repo);
                failed += 1;
                continue;
            }
        };
        node.embedding = Some(vector);
        // The invariant this whole tool rests on. If it ever fails, we would be
        // minting a second node rather than repairing the first, so stop dead
        // instead of writing.
        let after = node.compute_id();
        assert_eq!(
            before, after,
            "node id changed while adding an embedding; embedding must not be identity-bearing. Aborting before write."
        );
        match store.put_node(&node) {
            Ok(_) => done += 1,
            Err(e) => {
                eprintln!("  SKIP {} ({}): write failed: {e}", &before.to_hex()[..12], node.repo);
                failed += 1;
            }
        }
    }
    eprintln!("backfill_embeddings: embedded {done} node(s), {failed} skipped");
    if failed > 0 {
        std::process::exit(1);
    }
}
