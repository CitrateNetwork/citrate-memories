//! The multi-session MCP daemon: ONE process owns the RocksDB lock (and the
//! transformer model, loaded once) and serves any number of concurrent MCP
//! sessions over a Unix socket. Each Claude Code instance connects through the
//! thin `mcp_connect` shim, so a whole team shares one graph.
//!
//!   cargo run -p mem-mcp --example mcp_serve --features rocksdb,transformer -- \
//!       ./data/federation.bge.memdag ./data/memdag.sock
//!
//! Singleton by construction: the RocksDB LOCK is taken *before* the socket is
//! touched, so a second daemon racing for the same DB exits at open and never
//! disturbs a live socket. A stale socket file (crashed daemon) is safe to
//! remove precisely because we already hold the DB lock no live daemon could
//! have released.
//!
//! Each connection gets its own `MemoryMcpServer` — its own grant — over the
//! shared store; writes are serialized through one write gate. All sessions
//! share ONE persistent audit chain (`<db>.audit.jsonl`, verified on load). The
//! grant here is the same demo wildcard as `mcp_stdio`; per-user signed grants
//! (citrate-identity SIWE) are the v2 integration (F-5).

use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;

use mem_assert::Asserter;
use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::MemoryNode;
use mem_index::Embedder;
use mem_mcp::{serve_connection, MemoryMcpServer};
use mem_store::MemoryDagStore;

fn demo_grant() -> CapabilityGrant {
    let mut grant = CapabilityGrant {
        id: "daemon-demo".into(),
        issuer: "did:saul".into(),
        recipient: "agent:mcp-client".into(),
        allowed_resources: vec![ResourceScope { resource_id: "*".into(), can_read: true, can_write: true }],
        policy: PolicyProfile::Maintainer,
        expires_at_ms: u64::MAX,
        revoked: false,
        delegation_chain: vec![],
        issuer_pubkey: vec![],
        signature: vec![],
    };
    grant.sign_with(&SigningKey::from_bytes(&[1u8; 32]));
    grant
}

/// Detect the store's embedding model and load the matching query embedder once
/// for every session (else the index's model guard would reject every search).
/// The hashing baseline needs no setup, so a hashing store returns `None`.
#[cfg(feature = "transformer")]
fn load_query_embedder(store: &MemoryDagStore<MemoryNode>) -> Option<Arc<dyn Embedder>> {
    let model = mem_query::detect_store_embedding_model(store).ok().flatten()?;
    if model != mem_index::transformer::DEFAULT_MODEL_ID {
        return None;
    }
    eprintln!("mcp_serve: store embedded with '{model}', loading transformer embedder…");
    match mem_index::TransformerEmbedder::bge_base() {
        Ok(e) => Some(Arc::new(e)),
        Err(e) => {
            eprintln!("mcp_serve: failed to load embedder: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "transformer"))]
fn load_query_embedder(_store: &MemoryDagStore<MemoryNode>) -> Option<Arc<dyn Embedder>> {
    None
}

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.memdag".to_string());
    let sock = std::env::args().nth(2).unwrap_or_else(|| "./data/memdag.sock".to_string());

    // DB first: this is the singleton lock. If another daemon is live, we exit
    // here and never touch its socket.
    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mcp_serve: cannot open {db} (another daemon live?): {e}");
            std::process::exit(1);
        }
    };

    // Holding the DB lock proves any existing socket file is stale.
    if std::path::Path::new(&sock).exists() {
        if let Err(e) = std::fs::remove_file(&sock) {
            eprintln!("mcp_serve: cannot remove stale socket {sock}: {e}");
            std::process::exit(1);
        }
    }

    // Load the query embedder ONCE for all sessions, matching the store's model.
    let query_embedder = load_query_embedder(&store);

    // ONE persistent audit chain for the whole daemon (SECREM-02 7.5): every
    // session appends to it, it is verified on load, and it survives restart.
    let audit_log = format!("{db}.audit.jsonl");
    let audit = match mem_authz::AuditChain::open(&audit_log) {
        Ok(c) => {
            eprintln!("mcp_serve: audit chain {audit_log} verified ({} records)", c.len());
            Arc::new(Mutex::new(c))
        }
        Err(e) => {
            // Fail closed: a chain that cannot be trusted must not be extended.
            eprintln!("mcp_serve: audit chain {audit_log} REJECTED: {e}");
            std::process::exit(1);
        }
    };

    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("mcp_serve: cannot bind {sock}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("mcp_serve: serving {db} on {sock}");

    let write_gate = Arc::new(Mutex::new(()));
    let index_cache = Arc::new(mem_query::TenantIndexCache::new());
    let store = &store;
    std::thread::scope(|scope| {
        for conn in listener.incoming() {
            let stream: UnixStream = match conn {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("mcp_serve: accept failed: {e}");
                    continue;
                }
            };
            let embedder = query_embedder.clone();
            let gate = Arc::clone(&write_gate);
            let cache = Arc::clone(&index_cache);
            let audit = Arc::clone(&audit);
            scope.spawn(move || {
                // Fresh session: own grant + signing identity; the persistent
                // audit chain is SHARED so every session extends one log.
                let mut server =
                    MemoryMcpServer::new_with_asserter(store, demo_grant(), Asserter::new(SigningKey::from_bytes(&[2u8; 32])))
                        .with_write_gate(gate)
                        .with_index_cache(cache)
                        .with_audit_chain(audit);
                if let Some(e) = embedder {
                    server = server.with_query_embedder(e);
                }
                let reader = match stream.try_clone() {
                    Ok(r) => BufReader::new(r),
                    Err(e) => {
                        eprintln!("mcp_serve: cannot clone stream: {e}");
                        return;
                    }
                };
                if let Err(e) = serve_connection(reader, stream, &mut server) {
                    eprintln!("mcp_serve: session ended with error: {e}");
                }
            });
        }
    });
}
