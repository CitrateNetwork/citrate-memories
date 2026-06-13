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
//!
//! ## Durability / recovery (MEM-S6 WP-6.5)
//!
//! The encrypted store is the only on-disk copy, so the daemon keeps rolling
//! RocksDB checkpoints in `<db>.checkpoints/ckpt-<millis>`: one at startup and
//! one every `MEM_CHECKPOINT_INTERVAL_SECS` (default 1800; set 0 to disable),
//! retaining `MEM_CHECKPOINT_KEEP` (default 3). Each is a complete,
//! independently-openable store that survives deletion of the live store, so an
//! unclean death (even `kill -9` mid-compaction) can lose at most one interval.
//!
//! **To recover** if the live store won't open: stop the daemon, then
//! `mv <db> <db>.broken && cp -R <db>.checkpoints/ckpt-<latest> <db>` and
//! restart. (Or point `.mcp.json` at the checkpoint dir directly.) If even the
//! checkpoint is partial, `mem-store`'s `repair_store` example rebuilds the
//! catalog, and a `backfill` re-ingest restores the deterministic Derived plane.

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

/// MEM-S6 WP-6.5 — daemon durability. The encrypted store is the only on-disk
/// copy (the plaintext predecessors were deleted in MEM-S4), so an unclean death
/// of a daemon mid-compaction once left it referencing a since-deleted SST and
/// bricked the whole graph. The defence is a rolling **recovery point**: a
/// RocksDB checkpoint (cheap, hard-linked, consistent, and proven to survive
/// deletion of the source) taken at startup and on an interval, with the last K
/// retained. A `kill -9` can't defeat this — there is always a checkpoint at
/// most one interval old that opens on its own.
fn checkpoint_dir(db: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{db}.checkpoints"))
}

/// Take one rolling checkpoint, then prune to the newest `keep`. Returns the new
/// checkpoint's name. Best-effort: errors are logged by the caller, never fatal
/// (a failed checkpoint must not take down a serving daemon).
fn rolling_checkpoint(store: &MemoryDagStore<MemoryNode>, db: &str, keep: usize) -> Result<String, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let base = checkpoint_dir(db);
    std::fs::create_dir_all(&base).map_err(|e| format!("mkdir {}: {e}", base.display()))?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    // Zero-padded so lexical sort == chronological sort for the prune step.
    let name = format!("ckpt-{stamp:020}");
    let dest = base.join(&name);
    // create_checkpoint requires the dest not exist; the timestamp makes it unique.
    store.checkpoint(&dest).map_err(|e| e.to_string())?;

    // Prune: keep only the newest `keep` ckpt-* dirs.
    let mut ckpts: Vec<_> = std::fs::read_dir(&base)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("ckpt-"))
        .collect();
    ckpts.sort();
    if ckpts.len() > keep {
        for old in &ckpts[..ckpts.len() - keep] {
            let _ = std::fs::remove_dir_all(base.join(old));
        }
    }
    Ok(name)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
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

    // MEM-S6 WP-6.5: take a recovery checkpoint at startup, BEFORE binding the
    // socket — the store was just verified openable, so this is a known-good
    // point even if the daemon dies seconds later. Disable with
    // MEM_CHECKPOINT_INTERVAL_SECS=0.
    let ckpt_interval_secs = env_usize("MEM_CHECKPOINT_INTERVAL_SECS", 1800); // 30 min
    let ckpt_keep = env_usize("MEM_CHECKPOINT_KEEP", 3).max(1);
    if ckpt_interval_secs > 0 {
        match rolling_checkpoint(&store, &db, ckpt_keep) {
            Ok(name) => eprintln!("mcp_serve: startup checkpoint {} (keep {ckpt_keep})", name),
            Err(e) => eprintln!("mcp_serve: WARN startup checkpoint failed (continuing): {e}"),
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
    let db_for_ckpt = db.clone();
    std::thread::scope(|scope| {
        // MEM-S6 WP-6.5: rolling checkpoint thread. Runs alongside the accept
        // loop for the daemon's whole life, so the recovery point is never more
        // than one interval stale regardless of how the daemon dies.
        if ckpt_interval_secs > 0 {
            let store_ck = store;
            let db_ck = db_for_ckpt;
            scope.spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(ckpt_interval_secs as u64));
                match rolling_checkpoint(store_ck, &db_ck, ckpt_keep) {
                    Ok(name) => eprintln!("mcp_serve: checkpoint {name}"),
                    Err(e) => eprintln!("mcp_serve: WARN checkpoint failed (continuing): {e}"),
                }
            });
        }
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
