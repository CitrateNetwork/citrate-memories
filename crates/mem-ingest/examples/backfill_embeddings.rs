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
//! **Not every unembedded node is a bug.** Marker kinds (`Branch`) are left
//! unembedded on purpose: their content is a mechanical `<name>@<tip>` string, so
//! a vector over it is noise. This tool honours `mem_ingest::is_marker_kind` and
//! reports what it skipped, because a backfill that cannot tell "a bug dropped
//! this vector" from "this was deliberate" silently overrides a design decision
//! everywhere it runs. `--strip-markers` reverses an earlier run that did.
//!
//! Usage:
//!   cargo run -p mem-ingest --example backfill_embeddings --release \
//!     --features rocksdb,transformer -- <DB_PATH> [--apply] [--strip-markers]
//!
//! Dry-run by default: it reports what it would do and writes nothing. Pass
//! `--apply` to commit. **Stop the gateway first** (`sudo systemctl stop
//! mem-gateway`) or the RocksDB lock is held and the store will not open.

use std::collections::BTreeMap;

use mem_core::MemoryNode;
use mem_index::{Embedder, HashingEmbedder};
use mem_ingest::{is_marker_kind, EMBED_DIM};
use mem_store::MemoryDagStore;

/// `{kind: count}`, so the operator sees WHAT is about to change, not just how
/// much. A backfill is a bulk rewrite; "429 nodes" is not reviewable, "429
/// commits" or "429 branch markers" is.
fn by_kind(nodes: &[MemoryNode]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for n in nodes {
        *m.entry(n.kind.discriminant().to_string()).or_default() += 1;
    }
    m
}

/// Rewrite one node in place. Returns false if the id moved, which would mean
/// minting a duplicate rather than repairing the original.
fn put_in_place(store: &MemoryDagStore<MemoryNode>, node: &MemoryNode, before: mem_core::ContentHash) -> bool {
    assert_eq!(
        before,
        node.compute_id(),
        "node id changed during repair; embedding must not be identity-bearing. Aborting before write."
    );
    match store.put_node(node) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  SKIP {} ({}): write failed: {e}", &before.to_hex()[..12], node.repo);
            false
        }
    }
}

/// The embedding model this store's existing nodes were built with (first
/// embedded node wins). `None` means nothing in the store is embedded yet.
fn store_model(store: &MemoryDagStore<MemoryNode>) -> Option<String> {
    store.all_nodes().ok()?.into_iter().find_map(|n| n.embedding.map(|v| v.model))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let db = args.next().unwrap_or_else(|| {
        eprintln!("usage: backfill_embeddings <DB_PATH> [--apply] [--strip-markers]");
        std::process::exit(2);
    });
    let flags: Vec<String> = args.collect();
    let apply = flags.iter().any(|a| a == "--apply");
    let strip_markers = flags.iter().any(|a| a == "--strip-markers");

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

    // Repair mode: undo an earlier run that embedded marker nodes before this
    // tool knew the difference. Restores them to their intended unembedded state.
    if strip_markers {
        let stale: Vec<MemoryNode> =
            all.into_iter().filter(|n| is_marker_kind(&n.kind) && n.embedding.is_some()).collect();
        eprintln!("backfill_embeddings: {} marker node(s) carry an embedding they should not", stale.len());
        for (kind, count) in by_kind(&stale) {
            eprintln!("  {kind:<28} {count}");
        }
        if stale.is_empty() {
            eprintln!("backfill_embeddings: nothing to strip");
            return;
        }
        if !apply {
            eprintln!("backfill_embeddings: DRY RUN, nothing written. Add --apply to commit.");
            return;
        }
        let mut stripped = 0usize;
        for mut node in stale {
            let before = node.compute_id();
            node.embedding = None;
            if put_in_place(&store, &node, before) {
                stripped += 1;
            }
        }
        eprintln!("backfill_embeddings: stripped {stripped} marker embedding(s)");
        return;
    }

    let unembedded: Vec<MemoryNode> = all.into_iter().filter(|n| n.embedding.is_none()).collect();
    let (skipped, targets): (Vec<MemoryNode>, Vec<MemoryNode>) =
        unembedded.into_iter().partition(|n| is_marker_kind(&n.kind));

    let mut by_tenant: BTreeMap<String, usize> = BTreeMap::new();
    for n in &targets {
        *by_tenant.entry(n.repo.clone()).or_default() += 1;
    }
    eprintln!(
        "backfill_embeddings: {} of {total} nodes to embed, across {} tenant(s)",
        targets.len(),
        by_tenant.len()
    );
    for (repo, count) in &by_tenant {
        eprintln!("  {repo:<28} {count}");
    }
    for (kind, count) in by_kind(&targets) {
        eprintln!("  [kind] {kind:<21} {count}");
    }
    // Say what was left alone and why. A silent skip reads as "nothing there".
    if !skipped.is_empty() {
        eprintln!(
            "backfill_embeddings: leaving {} marker node(s) unembedded on purpose ({}); \
             use --strip-markers to undo an earlier run that embedded them",
            skipped.len(),
            by_kind(&skipped)
                .into_iter()
                .map(|(k, c)| format!("{k}: {c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
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
        if put_in_place(&store, &node, before) {
            done += 1;
        } else {
            failed += 1;
        }
    }
    eprintln!("backfill_embeddings: embedded {done} node(s), {failed} skipped");
    if failed > 0 {
        std::process::exit(1);
    }
}
