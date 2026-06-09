//! Ingest a git repo's history into an in-memory DAG and print its storyline.
//!
//! Usage: `cargo run -p mem-ingest --example ingest_repo -- [PATH]`
//! (PATH defaults to the current directory.)

use std::collections::BTreeMap;
use std::path::Path;

use mem_core::{MemoryNode, NodeKind};
use mem_ingest::Ingestor;
use mem_store::kv::InMemoryKv;
use mem_store::MemoryDagStore;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
    let ingestor = Ingestor::new("citrate-memories");

    let report = match ingestor.ingest(Path::new(&path), &store) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ingest failed: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "ingested {} commits + {} docs -> {} nodes, {} edges",
        report.commits, report.docs, report.nodes_in_store, report.edges_in_store
    );
    if let Some(head) = &report.watermark.head {
        let short = &head[..head.len().min(12)];
        println!(
            "watermark: HEAD {short} ({} commits) @ {}ms",
            report.watermark.head_count, report.watermark.ingested_at_ms
        );
    }

    let all = store.all_nodes().unwrap_or_default();

    // Kind histogram.
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for n in &all {
        *by_kind.entry(n.kind.discriminant()).or_default() += 1;
    }
    println!("\n-- nodes by kind --");
    for (kind, count) in &by_kind {
        println!("  {count:>3}  {kind}");
    }

    // Commit storyline.
    let mut commits: Vec<&MemoryNode> = all.iter().filter(|n| matches!(n.kind, NodeKind::Commit)).collect();
    commits.sort_by_key(|n| n.valid_from);
    println!("\n-- storyline (oldest first) --");
    for n in &commits {
        let subject = String::from_utf8_lossy(&n.content);
        println!("  {}  {}", &n.compute_id().to_hex()[..10], subject);
    }

    // Docs ingested: doc nodes carry an embedding and aren't commits; minted
    // reference nodes have no embedding, so they're excluded.
    let docs: Vec<&MemoryNode> = all
        .iter()
        .filter(|n| n.embedding.is_some() && !matches!(n.kind, NodeKind::Commit))
        .collect();
    println!("\n-- docs --");
    for n in &docs {
        let title = String::from_utf8_lossy(&n.content);
        println!("  [{}] {}", n.kind.discriminant(), title);
    }
}
