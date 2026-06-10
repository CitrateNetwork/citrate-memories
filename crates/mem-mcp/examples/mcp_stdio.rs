//! The real MCP stdio server: an MCP client (e.g. Claude Code) speaks JSON-RPC
//! over stdin/stdout. One JSON-RPC message per line.
//!
//! Usage (in an MCP client config): run this binary with the DB path as arg.
//!   cargo run -p mem-mcp --example mcp_stdio --features rocksdb -- ./data/federation.memdag
//!
//! For a bge-embedded graph, build with `--features rocksdb,transformer`: the
//! server detects the store's embedding model at startup and loads the matching
//! transformer embedder so `memory.search` ranks in the right vector space.
//!
//! The grant here is a demo wildcard read grant. In production the server loads a
//! real signed CapabilityGrant (issued via citrate-identity SIWE) per session.

use ed25519_dalek::SigningKey;

use mem_assert::Asserter;
use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::MemoryNode;
use mem_mcp::{serve_stdio, MemoryMcpServer};
use mem_store::MemoryDagStore;

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.memdag".to_string());
    let store = MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&db).expect("open rocksdb store");

    let mut grant = CapabilityGrant {
        id: "stdio-demo".into(),
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

    // Session signing identity for assertions.
    let asserter = Asserter::new(SigningKey::from_bytes(&[2u8; 32]));
    let mut server = MemoryMcpServer::new_with_asserter(&store, grant, asserter);

    // If the store was embedded with a transformer model, load the matching query
    // embedder so memory.search ranks in the same vector space (else the index
    // guard would reject every query). The hashing baseline needs no setup.
    #[cfg(feature = "transformer")]
    if let Ok(Some(model)) = mem_query::detect_store_embedding_model(&store) {
        if model == mem_index::transformer::DEFAULT_MODEL_ID {
            eprintln!("mcp_stdio: store embedded with '{model}', loading transformer embedder…");
            match mem_index::TransformerEmbedder::bge_base() {
                Ok(e) => server = server.with_query_embedder(std::sync::Arc::new(e)),
                Err(e) => {
                    eprintln!("mcp_stdio: failed to load embedder: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    if let Err(e) = serve_stdio(&mut server) {
        eprintln!("mcp_stdio: {e}");
        std::process::exit(1);
    }
}
