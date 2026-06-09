//! Scripted MCP session against the persistent federation graph, showing
//! capability-grant authorization (allow + deny) and the audit log.
//!
//! Usage: cargo run -p mem-mcp --example mcp_demo --features rocksdb -- [DB]

use ed25519_dalek::SigningKey;
use serde_json::Value;

use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::MemoryNode;
use mem_mcp::MemoryMcpServer;
use mem_store::MemoryDagStore;

fn show(label: &str, resp: &str) {
    println!("\n### {label}");
    let v: Value = match serde_json::from_str(resp) {
        Ok(v) => v,
        Err(_) => {
            println!("{resp}");
            return;
        }
    };
    if let Some(err) = v.get("error") {
        println!("JSON-RPC error: {err}");
    } else if let Some(result) = v.get("result") {
        if let Some(text) = result.pointer("/content/0/text").and_then(|t| t.as_str()) {
            let denied = result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
            println!("{}{}", if denied { "[DENIED] " } else { "" }, text);
        } else if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
            for t in tools {
                println!("  tool: {}", t.get("name").and_then(|n| n.as_str()).unwrap_or("?"));
            }
        } else {
            println!("{result}");
        }
    }
}

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| "./data/federation.memdag".to_string());
    let store = MemoryDagStore::<MemoryNode>::open_rocksdb(&db).expect("open rocksdb store");

    // Grant: read on citrate-chain only. Signed by a (demo) issuer key.
    let mut grant = CapabilityGrant {
        id: "demo-grant".into(),
        issuer: "did:saul".into(),
        recipient: "agent:demo".into(),
        allowed_resources: vec![ResourceScope {
            resource_id: "repo:citrate-chain/memory".into(),
            can_read: true,
            can_write: false,
        }],
        policy: PolicyProfile::ReadOnly,
        expires_at_ms: u64::MAX,
        revoked: false,
        delegation_chain: vec![],
        issuer_pubkey: vec![],
        signature: vec![],
    };
    grant.sign_with(&SigningKey::from_bytes(&[42u8; 32]));
    println!("grant: recipient={} reads={:?}", grant.recipient, grant.allowed_resources.iter().map(|r| &r.resource_id).collect::<Vec<_>>());

    let mut srv = MemoryMcpServer::new(&store, grant);

    let session = [
        ("initialize", r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
        ("tools/list", r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#),
        (
            "recall citrate-chain (ALLOWED by grant)",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-chain","budget":6}}}"#,
        ),
        (
            "recall citrate-identity (DENIED — outside grant scope)",
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-identity"}}}"#,
        ),
        (
            "search citrate-chain 'ghostdag blue score' (ALLOWED)",
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memory.search","arguments":{"repo":"citrate-chain","query":"ghostdag blue score consensus","budget":5}}}"#,
        ),
    ];

    for (label, req) in session {
        if let Some(resp) = srv.handle_line(req) {
            show(label, &resp);
        }
    }

    println!("\n=== audit log (tamper-evident hash-chain) ===");
    for r in srv.audit().records() {
        println!("  #{} {:?}  {}  ({})", r.sequence, r.event, r.resource_id, r.detail);
    }
    println!("integrity check: {:?}", srv.audit().verify_integrity());
}
