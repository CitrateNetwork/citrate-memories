//! Per-tenant freshness report (MEM-S7 ops): print every tenant's recorded
//! `Watermark` (ingested HEAD, commit count, ingested-at) as TSV, read-only.
//!
//! The live store is single-writer (held by `mem-gateway`), so point this at a
//! COPY — e.g. a backup taken while the gateway was briefly stopped, or a
//! `cp -r` snapshot with `LOCK` removed (as `scripts/mcp-stdio.sh` does). Never
//! point it at the live store dir while the gateway runs: a second opener fails
//! on the LOCK (by design).
//!
//! Usage:
//!   cargo run -p mem-ingest --example freshness --features rocksdb -- <DB_PATH> [REPO...]
//!
//! With no REPO arguments, the tenant set is discovered from the store's nodes
//! (every distinct `repo` field). Output: `repo\thead\thead_count\tingested_at_ms`
//! (`-` for a tenant with no watermark). Compare `head` against
//! `git ls-remote <url> HEAD` to classify a tenant fresh/stale; the gateway's
//! WP-7.4 reconciler performs exactly that compare on its own sweep.

use std::collections::BTreeSet;

use mem_core::MemoryNode;
use mem_ingest::Ingestor;
use mem_store::MemoryDagStore;

fn main() {
    let mut args = std::env::args().skip(1);
    let db_path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: freshness <DB_PATH> [REPO...]");
            std::process::exit(2);
        }
    };
    let explicit: Vec<String> = args.collect();

    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("freshness: open {db_path}: {e}");
            std::process::exit(1);
        }
    };

    let repos: BTreeSet<String> = if explicit.is_empty() {
        match store.all_nodes() {
            Ok(nodes) => nodes.into_iter().map(|n| n.repo).collect(),
            Err(e) => {
                eprintln!("freshness: scan nodes: {e}");
                std::process::exit(1);
            }
        }
    } else {
        explicit.into_iter().collect()
    };

    for repo in repos {
        match Ingestor::new(repo.clone()).read_watermark(&store) {
            Ok(Some(w)) => println!(
                "{}\t{}\t{}\t{}",
                repo,
                w.head.as_deref().unwrap_or("-"),
                w.head_count,
                w.ingested_at_ms
            ),
            Ok(None) => println!("{repo}\t-\t0\t0"),
            Err(e) => {
                eprintln!("freshness: watermark {repo}: {e}");
                println!("{repo}\t?\t0\t0");
            }
        }
    }
}
