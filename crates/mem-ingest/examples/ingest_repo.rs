//! Ingest a git repo's history into an in-memory DAG and print its storyline.
//!
//! Usage: `cargo run -p mem-ingest --example ingest_repo -- [PATH]`
//! (PATH defaults to the current directory.)

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
        "ingested {} commits -> {} nodes, {} edges",
        report.commits, report.nodes_in_store, report.edges_in_store
    );
    if let Some(head) = &report.watermark.head {
        let short = &head[..head.len().min(12)];
        println!(
            "watermark: HEAD {short} ({} commits) @ {}ms",
            report.watermark.head_count, report.watermark.ingested_at_ms
        );
    }

    let mut commits: Vec<MemoryNode> = store
        .all_nodes()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Commit))
        .collect();
    commits.sort_by_key(|n| n.valid_from);

    println!("\n-- storyline (oldest first) --");
    for n in &commits {
        let subject = String::from_utf8_lossy(&n.content);
        let id = n.compute_id().to_hex();
        println!("  {}  {}", &id[..10], subject);
    }
}
