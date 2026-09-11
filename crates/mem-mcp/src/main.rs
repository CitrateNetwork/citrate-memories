//! `mem-mcp` — the multi-session memory daemon.
//!
//! ONE process owns the RocksDB lock (and the transformer model, loaded once)
//! and serves any number of concurrent MCP sessions over a Unix socket. Each
//! Claude Code instance connects through the thin `mcp_connect` shim, so a whole
//! team shares one graph.
//!
//! ```text
//! cargo build -p mem-mcp --bin mem-mcp --release --features rocksdb
//! mem-mcp <store-path> <sock-path>
//! ```
//!
//! **`--features rocksdb` is required** — the daemon is a RocksDB singleton, so
//! the bin carries `required-features = ["rocksdb"]`. Without it cargo reports
//! "no bin target named `mem-mcp`", which is exactly how the missing binary
//! first surfaced when packaging citrate-core.
//!
//! # Promoted from an example (2026-07-25)
//!
//! This was `examples/mcp_serve.rs`. `citrate-core` declares `binaries/mem-mcp`
//! as a Tauri `externalBin` and `tauri-build` validates every such path at build
//! time, so the desktop app could not be packaged at all while the daemon existed
//! only as an example. Promoting it (rather than writing a second daemon) keeps
//! the shipped path and the tested path the same code.
//!
//! # The contract with citrate-core (do not change unilaterally)
//!
//! ```text
//! mem-mcp <store-path> <sock-path>
//! env: CITRATE_MEM_STORE_KEY = <hex>   (optional; see "Identity" below)
//! ```
//!
//! Positional args only: `MemoryManager::build_spec` passes no lock/socket flags
//! because the daemon owns both its singleton lock and its stale-socket cleanup.
//! The key travels in the environment, never argv, so it cannot leak via `ps`.
//!
//! # Identity — the shipped binary must not carry a hardcoded key
//!
//! As an example this signed its grant with `SigningKey::from_bytes(&[1u8; 32])`
//! and authored with `[2u8; 32]`. That is fine for a demo and wrong for a
//! product: every install would share one identity, so `blame()` could not tell
//! two users apart and any copy of the binary could mint that issuer's grants.
//!
//! `citrate-core` mints a per-user store wrapping key in the OS keyring and
//! passes it as `CITRATE_MEM_STORE_KEY`, documenting it as a "forward-compatible
//! seam — honoured IFF/when the daemon grows a key intake". This is that intake.
//! The key is NOT used for encryption (mem-store seals each tenant with its own
//! key inside the `KEYS` CF; `open_rocksdb_auto` takes only a path) — it seeds a
//! **domain-separated** ed25519 identity via the same blake3 idiom
//! `Asserter::for_principal` already uses. Domain separation is what keeps this
//! from being key reuse: the derived key cannot be inverted to the wrapping key.
//!
//! Deriving from the keyring key also keeps the identity stable across restarts
//! without writing a secret to disk in the clear. With no key in the environment
//! the daemon falls back to an ephemeral per-process identity: reads are
//! unaffected, authored writes get a per-process author.
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
//! session grant is wildcard over this user's tenants and is bounded by the
//! socket's filesystem permissions (see `session_grant`); per-user signed grants
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
use std::sync::{Arc, Mutex};

// Cross-platform local IPC: `ListenerOptions`/`incoming` (via `prelude`) replace
// the old `std::os::unix::net::UnixListener`. On unix the bound endpoint is the
// same filesystem socket path as before (see `mem_mcp::endpoint_name`).
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::ListenerOptions;

use ed25519_dalek::SigningKey;

use mem_assert::Asserter;
use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::MemoryNode;
use mem_index::Embedder;
use mem_mcp::{endpoint_name, serve_connection, MemoryMcpServer};
use mem_store::MemoryDagStore;

/// Env var carrying the per-user store wrapping key (hex), set by citrate-core.
const STORE_KEY_ENV: &str = "CITRATE_MEM_STORE_KEY";

/// Domain separator for the daemon's ed25519 identity. Changing it rotates every
/// local daemon identity, so treat it as a constant.
const IDENTITY_LABEL: &[u8] = b"mem-mcp:daemon-identity:v1";

/// The daemon's signing identity, derived from the keyring-held wrapping key, or
/// ephemeral when the seam is unset. See "Identity" in the module docs.
fn daemon_identity() -> SigningKey {
    match std::env::var(STORE_KEY_ENV).ok().and_then(|k| {
        let raw = hex::decode(k.trim()).ok()?;
        (!raw.is_empty()).then_some(raw)
    }) {
        Some(raw) => {
            let mut h = blake3::Hasher::new();
            h.update(IDENTITY_LABEL);
            h.update(&(raw.len() as u64).to_le_bytes());
            h.update(&raw);
            SigningKey::from_bytes(h.finalize().as_bytes())
        }
        None => {
            eprintln!(
                "mem-mcp: {STORE_KEY_ENV} unset — using an ephemeral daemon identity \
                 (reads unaffected; authored writes get a per-process author)"
            );
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).expect("OS randomness");
            SigningKey::from_bytes(&seed)
        }
    }
}

/// The locally-minted session grant: wildcard read+write over this user's own
/// tenants.
///
/// That is deliberate and is not a hole here, because the real trust boundary is
/// the **filesystem permissions on the Unix socket** — it lives in the per-user
/// app-data dir, one graph per user, never shared, so anyone who can `connect()`
/// already holds the user's privileges. Callers needing finer authorization use
/// the 2026-07-28 stateless profile and present a signed per-request grant in
/// `_meta["ai.citrate/grant"]`, which overrides this for that call.
fn session_grant(sk: &SigningKey) -> CapabilityGrant {
    let mut grant = CapabilityGrant {
        id: "mem-mcp:local-session".into(),
        issuer: "local:citrate-core".into(),
        recipient: "local:mem-mcp".into(),
        allowed_resources: vec![ResourceScope { resource_id: "*".into(), can_read: true, can_write: true }],
        policy: PolicyProfile::Maintainer,
        expires_at_ms: u64::MAX,
        revoked: false,
        delegation_chain: vec![],
        issuer_pubkey: vec![],
        signature: vec![],
    };
    grant.sign_with(sk);
    grant
}

/// Detect the store's embedding model and load the matching query embedder once
/// for every session (else the index's model guard would reject every search).
/// The hashing baseline needs no setup, so a hashing store returns `None`.
#[cfg(feature = "transformer")]
fn load_query_embedder(store: &MemoryDagStore<MemoryNode>) -> Option<Arc<dyn Embedder>> {
    let detected = mem_query::detect_store_embedding_model(store).ok().flatten();
    // `CITRATE_MEM_EMBED=bge` FORCES the BGE embedder even on a FRESH store (none
    // detected yet). Without it, a brand-new store's first write falls back to the
    // HashingEmbedder (embed_for_write) and locks the store to lexical vectors
    // forever — so a client that wants semantic recall (citrate-core) sets this to
    // bootstrap the store to BGE for BOTH writes and queries. A store ALREADY
    // embedded with a different (non-BGE) model is never overridden.
    let force_bge = std::env::var("CITRATE_MEM_EMBED")
        .map(|v| v.eq_ignore_ascii_case("bge"))
        .unwrap_or(false);
    match detected.as_deref() {
        Some(m) if m == mem_index::transformer::DEFAULT_MODEL_ID => {
            eprintln!("mem-mcp: store embedded with '{m}', loading transformer embedder…");
        }
        Some(other) => {
            if force_bge {
                eprintln!("mem-mcp: store already embedded with '{other}', not overriding to BGE");
            }
            return None;
        }
        None if force_bge => {
            eprintln!("mem-mcp: fresh store, CITRATE_MEM_EMBED=bge → bootstrapping to BGE");
        }
        None => return None,
    }
    match mem_index::TransformerEmbedder::bge_base() {
        Ok(e) => Some(Arc::new(e)),
        Err(e) => {
            eprintln!("mem-mcp: failed to load embedder: {e}");
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

    // One identity for this daemon process (see "Identity" in the module docs).
    let identity = daemon_identity();

    // DB first: this is the singleton lock. If another daemon is live, we exit
    // here and never touch its socket.
    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mem-mcp: cannot open {db} (another daemon live?): {e}");
            std::process::exit(1);
        }
    };

    // Holding the DB lock proves any existing socket file is stale. This is a
    // Unix-only concern: the socket is a filesystem object there, and bind fails
    // on a leftover path. On Windows the endpoint is a namespaced pipe with no
    // stale filesystem object to remove (the OS drops the name with the owner).
    #[cfg(unix)]
    if std::path::Path::new(&sock).exists() {
        if let Err(e) = std::fs::remove_file(&sock) {
            eprintln!("mem-mcp: cannot remove stale socket {sock}: {e}");
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
            Ok(name) => eprintln!("mem-mcp: startup checkpoint {} (keep {ckpt_keep})", name),
            Err(e) => eprintln!("mem-mcp: WARN startup checkpoint failed (continuing): {e}"),
        }
    }

    // Load the query embedder ONCE for all sessions, matching the store's model.
    let query_embedder = load_query_embedder(&store);

    // ONE persistent audit chain for the whole daemon (SECREM-02 7.5): every
    // session appends to it, it is verified on load, and it survives restart.
    let audit_log = format!("{db}.audit.jsonl");
    let audit = match mem_authz::AuditChain::open(&audit_log) {
        Ok(c) => {
            eprintln!("mem-mcp: audit chain {audit_log} verified ({} records)", c.len());
            Arc::new(Mutex::new(c))
        }
        Err(e) => {
            // Fail closed: a chain that cannot be trusted must not be extended.
            eprintln!("mem-mcp: audit chain {audit_log} REJECTED: {e}");
            std::process::exit(1);
        }
    };

    // Same positional `<sock-path>` arg; `endpoint_name` maps it to the
    // platform endpoint (unix: the same filesystem socket path as before).
    let name = match endpoint_name(&sock) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("mem-mcp: invalid socket name {sock}: {e}");
            std::process::exit(1);
        }
    };
    let listener = match ListenerOptions::new().name(name).create_sync() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("mem-mcp: cannot bind {sock}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("mem-mcp: serving {db} on {sock}");

    let write_gate = Arc::new(Mutex::new(()));
    let index_cache = Arc::new(mem_query::TenantIndexCache::new());
    let grant = session_grant(&identity);
    let identity = &identity;
    let grant = &grant;
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
                    Ok(name) => eprintln!("mem-mcp: checkpoint {name}"),
                    Err(e) => eprintln!("mem-mcp: WARN checkpoint failed (continuing): {e}"),
                }
            });
        }
        for conn in listener.incoming() {
            let stream = match conn {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("mem-mcp: accept failed: {e}");
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
                    MemoryMcpServer::new_with_asserter(store, grant.clone(), Asserter::new(identity.clone()))
                        .with_write_gate(gate)
                        .with_index_cache(cache)
                        .with_audit_chain(audit);
                if let Some(e) = embedder {
                    server = server.with_query_embedder(e);
                }
                // `serve_connection` runs synchronously on this one thread
                // (read a line, write its response, repeat), so a single
                // connection borrowed twice suffices — no fd clone needed. The
                // interprocess `Stream` implements `Read`+`Write` on `&Stream`.
                let reader = BufReader::new(&stream);
                if let Err(e) = serve_connection(reader, &stream, &mut server) {
                    eprintln!("mem-mcp: session ended with error: {e}");
                }
            });
        }
    });
}
