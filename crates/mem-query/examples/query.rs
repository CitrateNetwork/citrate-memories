//! Ask the federation memory graph questions.
//!
//! Usage:
//!   cargo run -p mem-query --example query --features rocksdb -- <DB> <REPO> storyline [N]
//!   cargo run -p mem-query --example query --features rocksdb -- <DB> <REPO> search "<QUERY>" [N]
//!   cargo run -p mem-query --example query --features rocksdb -- <DB> <REPO> neighbors <ID_PREFIX> [N]

use mem_core::MemoryNode;
use mem_query::{Recall, RecallItem, RecallResult};
use mem_store::MemoryDagStore;

fn truncate(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= n {
        one_line
    } else {
        let kept: String = one_line.chars().take(n.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

fn print_item(i: &RecallItem) {
    let score = i.score.map(|s| format!("{s:.3}  ")).unwrap_or_default();
    println!(
        "  {}  {}{:<16} {}",
        &i.id.to_hex()[..10],
        score,
        format!("[{}]", i.kind.discriminant()),
        truncate(&i.title, 70)
    );
}

fn print_result(r: &RecallResult) {
    match &r.watermark {
        Some(w) => {
            let head = w.head.as_deref().unwrap_or("?");
            println!(
                "freshness: HEAD {} ({} commits) ingested @ {}ms",
                &head[..head.len().min(12)],
                w.head_count,
                w.ingested_at_ms
            );
        }
        None => println!("freshness: (no watermark — repo not ingested?)"),
    }
    println!("tenant '{}' — {} nodes, showing {}:", r.repo, r.total_in_tenant, r.items.len());
    for i in &r.items {
        print_item(i);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: query <DB> <REPO> <storyline|search|neighbors> [args]");
        std::process::exit(2);
    }
    let (db, repo, cmd) = (&args[0], &args[1], args[2].as_str());

    let store = MemoryDagStore::<MemoryNode>::open_rocksdb(db).expect("open rocksdb store");
    let recall = Recall::new(&store);

    match cmd {
        "storyline" => {
            let budget = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(15);
            let r = recall.storyline(repo, budget).expect("storyline");
            print_result(&r);
        }
        "search" => {
            let query = args.get(3).cloned().unwrap_or_default();
            let budget = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(10);
            println!("query: {query:?}");
            let r = recall.search(repo, &query, budget).expect("search");
            print_result(&r);
        }
        "neighbors" => {
            let prefix = args.get(3).cloned().unwrap_or_default();
            let budget = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
            match recall.resolve_prefix(&prefix).expect("resolve") {
                None => eprintln!("no unique node for prefix '{prefix}'"),
                Some(id) => {
                    println!("neighbors of {}:", &id.to_hex()[..12]);
                    for nb in recall.neighbors(&id, budget).expect("neighbors") {
                        let dir = match nb.direction {
                            mem_query::Direction::Out => "->",
                            mem_query::Direction::In => "<-",
                        };
                        let title = nb
                            .node
                            .as_ref()
                            .map(|n| truncate(&n.title, 60))
                            .unwrap_or_else(|| "(dangling)".to_string());
                        let kind = nb
                            .node
                            .as_ref()
                            .map(|n| n.kind.discriminant())
                            .unwrap_or_else(|| "?".to_string());
                        println!("  {dir} [{:?}] [{kind}] {title}", nb.edge_kind);
                    }
                }
            }
        }
        other => {
            eprintln!("unknown command '{other}' (storyline|search|neighbors)");
            std::process::exit(2);
        }
    }
}
