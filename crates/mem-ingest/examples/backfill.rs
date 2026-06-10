//! Full-federation backfill (WP-1.5): ingest every git repo under a workspace
//! into one persistent RocksDB-backed DAG. Each repo is a tenant (carried on every
//! node's `repo` field); per-tenant store isolation is a later refinement.
//!
//! Usage:
//!   cargo run -p mem-ingest --example backfill --features rocksdb -- [WORKSPACE] [DB_PATH] [EMBEDDER]
//!   (defaults: WORKSPACE=.. , DB_PATH=./data/federation.memdag , EMBEDDER=hashing)
//!
//! EMBEDDER is `hashing` (default, dependency-free) or `bge` (the bge-base-en-v1.5
//! transformer, requires `--features rocksdb,transformer`). Use a DISTINCT DB path
//! per embedder — vectors from different models must not share a store (the index's
//! model-version guard would reject cross-space queries). One model is loaded once
//! and shared across all tenants.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mem_core::{MemoryNode, NodeKind};
use mem_ingest::Ingestor;
use mem_store::MemoryDagStore;

fn is_git_repo(p: &Path) -> bool {
    p.join(".git").exists()
}

/// Build the per-tenant ingestor for the chosen embedder. With `bge`, every tenant
/// shares one `Arc`-wrapped transformer model (loaded once by the caller).
fn make_ingestor(name: String, embedder: &Embedder) -> Ingestor {
    match embedder {
        Embedder::Hashing => Ingestor::new(name),
        #[cfg(feature = "transformer")]
        Embedder::Bge(model) => Ingestor::with_embedder(name, Box::new(model.clone())),
    }
}

/// The selected embedder, holding the shared model for `bge`.
enum Embedder {
    Hashing,
    #[cfg(feature = "transformer")]
    Bge(std::sync::Arc<mem_index::TransformerEmbedder>),
}

fn select_embedder(which: &str) -> Embedder {
    match which {
        "hashing" => Embedder::Hashing,
        "bge" => {
            #[cfg(feature = "transformer")]
            {
                eprintln!("loading bge-base-en-v1.5 (first run downloads ~440MB)…");
                let model = mem_index::TransformerEmbedder::bge_base().expect("load bge-base");
                Embedder::Bge(std::sync::Arc::new(model))
            }
            #[cfg(not(feature = "transformer"))]
            {
                eprintln!("embedder 'bge' requires --features transformer; rebuild with \
                    `--features rocksdb,transformer`");
                std::process::exit(2);
            }
        }
        other => {
            eprintln!("unknown embedder '{other}' (hashing|bge)");
            std::process::exit(2);
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let workspace = args.next().unwrap_or_else(|| "..".to_string());
    let db_path = args.next().unwrap_or_else(|| "./data/federation.memdag".to_string());
    let which = args.next().unwrap_or_else(|| "hashing".to_string());

    if let Some(parent) = Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Discover repos: depth-1 directories that are git working trees.
    let mut repos: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&workspace).expect("read workspace dir") {
        let p = entry.expect("dir entry").path();
        if p.is_dir() && is_git_repo(&p) {
            repos.push(p);
        }
    }
    repos.sort();

    let embedder = select_embedder(&which);
    // Auto mode: a crypto-shredded DB (keyring present) keeps getting sealed
    // writes; a plaintext DB stays plaintext. See `reencrypt` for migrating.
    let store = MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db_path).expect("open rocksdb store");
    if store.is_encrypted_at_rest() {
        println!("(store is encrypted at rest — new nodes will be sealed)");
    }
    println!("backfilling {} repos -> {} (embedder: {which})\n", repos.len(), db_path);

    let started = Instant::now();
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let (mut total_commits, mut total_docs) = (0usize, 0usize);

    for repo in &repos {
        let name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ingestor = make_ingestor(name.clone(), &embedder);
        match ingestor.ingest(repo, &store) {
            Ok(r) => {
                println!("  {name:<34} {:>6} commits  {:>4} docs", r.commits, r.docs);
                total_commits += r.commits;
                total_docs += r.docs;
                ok += 1;
            }
            Err(e) => {
                println!("  {name:<34} FAILED: {e}");
                failed.push(name);
            }
        }
    }

    let nodes = store.node_count().unwrap_or(0);
    let edges = store.edge_count().unwrap_or(0);
    println!("\n{ok} repos ok, {} failed, in {:.1}s", failed.len(), started.elapsed().as_secs_f64());
    if !failed.is_empty() {
        println!("failed: {}", failed.join(", "));
    }
    println!("federation graph: {nodes} nodes, {edges} edges ({total_commits} commits + {total_docs} docs ingested)");

    // Kind histogram across the whole federation.
    let all = store.all_nodes().unwrap_or_default();
    let mut commits = 0;
    let mut docs = 0;
    let mut refs = 0;
    for n in &all {
        match n.kind {
            NodeKind::Commit => commits += 1,
            _ if n.embedding.is_some() => docs += 1,
            _ => refs += 1,
        }
    }
    println!("  commit nodes: {commits} · doc nodes: {docs} · reference nodes: {refs}");
}
