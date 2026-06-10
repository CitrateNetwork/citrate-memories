//! Re-encrypt a plaintext federation store into a crypto-shredded one (WP-1.6
//! operator step): same graph, same embeddings, same watermarks — node payloads
//! sealed per tenant on the way in. Minutes instead of the ~hour a fresh bge
//! re-backfill would cost, because nothing is re-embedded.
//!
//!   cargo run --release -p mem-store --example reencrypt --features rocksdb -- \
//!       ./data/federation.bge.memdag ./data/federation.bge.enc.memdag
//!
//! Nodes are decoded through the source store (so a partially-encrypted source
//! also works) and re-put through an encrypted destination store, which mints
//! each tenant's key on first write. Edges and operational meta (watermarks)
//! are structural plaintext by design and copy byte-for-byte. The destination
//! must not already exist.

use mem_core::MemoryNode;
use mem_store::kv::KvStore;
use mem_store::rocks::RocksKv;
use mem_store::{cf, MemoryDagStore, ALL_CFS};

fn main() {
    let src_path = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.memdag".into());
    let dst_path = std::env::args().nth(2).unwrap_or_else(|| "./data/federation.enc.memdag".into());
    if std::path::Path::new(&dst_path).exists() {
        eprintln!("reencrypt: destination {dst_path} already exists — refusing to mix into it");
        std::process::exit(2);
    }

    let src_kv = match RocksKv::open(&src_path, ALL_CFS) {
        Ok(kv) => kv,
        Err(e) => {
            eprintln!("reencrypt: cannot open source {src_path}: {e}\n(is the mcp daemon/server still holding its lock?)");
            std::process::exit(1);
        }
    };
    let dst_kv = RocksKv::open(&dst_path, ALL_CFS).expect("create destination db");

    // Edges + meta: structural plaintext, byte-for-byte (copied on the raw kv
    // handle before it becomes the encrypted store).
    let mut copied = 0usize;
    for cf_name in [cf::EDGES_OUT, cf::EDGES_IN, cf::META] {
        for (k, v) in src_kv.kv_iter_cf(cf_name).expect("iterate source cf") {
            dst_kv.kv_put(cf_name, &k, &v).expect("copy row");
            copied += 1;
        }
    }
    eprintln!("reencrypt: copied {copied} edge/meta rows");
    let dst = MemoryDagStore::<MemoryNode>::new_encrypted(Box::new(dst_kv));

    // Nodes: decode through the source store, seal through the destination.
    let src = MemoryDagStore::<MemoryNode>::new_auto(Box::new(src_kv)).expect("open source store");
    let nodes = src.all_nodes().expect("read source nodes");
    let total = nodes.len();
    let mut tenants = std::collections::BTreeSet::new();
    for (i, n) in nodes.iter().enumerate() {
        tenants.insert(n.repo.clone());
        dst.put_node(n).expect("seal node into destination");
        if (i + 1) % 1000 == 0 {
            eprintln!("reencrypt: {}/{total} nodes sealed", i + 1);
        }
    }

    // Verify counts line up and the destination really is sealed.
    assert_eq!(dst.node_count().expect("dst node count"), total, "node count mismatch");
    assert_eq!(
        dst.edge_count().expect("dst edge count"),
        src.edge_count().expect("src edge count"),
        "edge count mismatch"
    );
    assert!(dst.is_encrypted_at_rest());
    eprintln!(
        "reencrypt: done — {total} nodes sealed across {} tenants, {} edges, keys minted per tenant",
        tenants.len(),
        dst.edge_count().expect("edge count"),
    );
}
