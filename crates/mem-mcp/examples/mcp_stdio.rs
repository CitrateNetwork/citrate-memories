//! The real MCP stdio server: an MCP client (e.g. Claude Code) speaks JSON-RPC
//! over stdin/stdout. One JSON-RPC message per line.
//!
//! Usage (in an MCP client config): run this binary with the DB path as arg.
//!   cargo run -p mem-mcp --example mcp_stdio --features rocksdb -- ./data/federation.memdag
//!
//! The grant here is a demo wildcard read grant. In production the server loads a
//! real signed CapabilityGrant (issued via citrate-identity SIWE) per session.

use ed25519_dalek::SigningKey;

use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::MemoryNode;
use mem_mcp::{serve_stdio, MemoryMcpServer};
use mem_store::MemoryDagStore;

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.memdag".to_string());
    let store = MemoryDagStore::<MemoryNode>::open_rocksdb(&db).expect("open rocksdb store");

    let mut grant = CapabilityGrant {
        id: "stdio-demo".into(),
        issuer: "did:saul".into(),
        recipient: "agent:mcp-client".into(),
        allowed_resources: vec![ResourceScope { resource_id: "*".into(), can_read: true, can_write: false }],
        policy: PolicyProfile::ReadOnly,
        expires_at_ms: u64::MAX,
        revoked: false,
        delegation_chain: vec![],
        issuer_pubkey: vec![],
        signature: vec![],
    };
    grant.sign_with(&SigningKey::from_bytes(&[1u8; 32]));

    let mut server = MemoryMcpServer::new(&store, grant);
    if let Err(e) = serve_stdio(&mut server) {
        eprintln!("mcp_stdio: {e}");
        std::process::exit(1);
    }
}
