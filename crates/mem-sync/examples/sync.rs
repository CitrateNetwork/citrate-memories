//! Federation sync + anchoring operator CLI (MEM-S5 WP-5.1/5.2).
//!
//!   sync export <db> <tenant> <bundle.json>   write a tenant's CRDT bundle
//!   sync import <db> <bundle.json>            merge a bundle into a store
//!   sync anchor <db> <tenant>                 record a hash-chained merkle anchor
//!   sync verify <db> <tenant>                 check current state vs latest anchor
//!
//! Bundles are plain JSON — move them over any transport (file, scp, HTTP).
//! Stop the MCP daemon first when targeting its DB — it holds the lock.

use mem_core::MemoryNode;
use mem_store::MemoryDagStore;
use mem_sync::{anchor_tenant, export_tenant, merge_bundle, verify_anchor, SyncBundle};

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn open(db: &str) -> MemoryDagStore<MemoryNode> {
    match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sync: cannot open {db} (daemon still holding the lock?): {e}");
            std::process::exit(1);
        }
    }
}

fn usage() -> ! {
    eprintln!("usage: sync export <db> <tenant> <bundle.json>\n       sync import <db> <bundle.json>\n       sync anchor <db> <tenant>\n       sync verify <db> <tenant>");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, db, tenant, path] if cmd == "export" => {
            let store = open(db);
            let bundle = export_tenant(&store, tenant, now_ms()).expect("export tenant");
            let json = bundle.to_json().expect("serialize bundle");
            std::fs::write(path, &json).expect("write bundle file");
            eprintln!(
                "sync: exported tenant '{tenant}' — {} nodes, {} edges → {path} ({} bytes)",
                bundle.nodes.len(),
                bundle.edges.len(),
                json.len()
            );
        }
        [cmd, db, path] if cmd == "import" => {
            let store = open(db);
            let json = std::fs::read_to_string(path).expect("read bundle file");
            let bundle = SyncBundle::from_json(&json).expect("parse bundle");
            let out = merge_bundle(&store, &bundle).expect("merge bundle");
            eprintln!(
                "sync: merged tenant '{}' — +{} nodes ({} merged), +{} edges ({} merged), {} superseded; rejected: {} supersessions, {} signatures; {} contradictions surfaced",
                bundle.repo,
                out.nodes_added,
                out.nodes_merged,
                out.edges_added,
                out.edges_merged,
                out.superseded,
                out.rejected_supersessions,
                out.rejected_signatures,
                out.contradictions
            );
        }
        [cmd, db, tenant] if cmd == "anchor" => {
            let store = open(db);
            let rec = anchor_tenant(&store, tenant, now_ms()).expect("anchor tenant");
            eprintln!(
                "sync: anchored tenant '{tenant}' — root {}… over {} nodes / {} edges (prev: {})",
                &rec.root[..12],
                rec.node_count,
                rec.edge_count,
                rec.prev.as_deref().map(|p| &p[..12]).unwrap_or("none")
            );
        }
        [cmd, db, tenant] if cmd == "verify" => {
            let store = open(db);
            match verify_anchor(&store, tenant).expect("verify anchor") {
                None => {
                    eprintln!("sync: tenant '{tenant}' has never been anchored");
                    std::process::exit(3);
                }
                Some(true) => eprintln!("sync: tenant '{tenant}' matches its latest anchor ✓"),
                Some(false) => {
                    eprintln!("sync: tenant '{tenant}' has DRIFTED from its latest anchor (changed or tampered since)");
                    std::process::exit(4);
                }
            }
        }
        _ => usage(),
    }
}
