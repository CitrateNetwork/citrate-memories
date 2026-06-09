//! Asserted-plane write path over MCP, end to end (in-memory, no DB needed):
//! assert → recall → emit a memory-diff → another agent merges it → blame.
//!
//! Usage: cargo run -p mem-mcp --example assert_demo

use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use mem_assert::{Asserter, MemoryDiff};
use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::{MemoryNode, NodeKind};
use mem_mcp::MemoryMcpServer;
use mem_store::kv::InMemoryKv;
use mem_store::MemoryDagStore;

fn write_grant(repo: &str, seed: u8) -> CapabilityGrant {
    let mut g = CapabilityGrant {
        id: "demo".into(),
        issuer: "did:saul".into(),
        recipient: format!("agent:{seed}"),
        allowed_resources: vec![ResourceScope {
            resource_id: format!("repo:{repo}/memory"),
            can_read: true,
            can_write: true,
        }],
        policy: PolicyProfile::Maintainer,
        expires_at_ms: u64::MAX,
        revoked: false,
        delegation_chain: vec![],
        issuer_pubkey: vec![],
        signature: vec![],
    };
    g.sign_with(&SigningKey::from_bytes(&[seed; 32]));
    g
}

fn call(name: &str, args: Value) -> String {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string()
}

fn show(label: &str, resp: &str) {
    let v: Value = serde_json::from_str(resp).unwrap_or(Value::Null);
    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str()).unwrap_or("(no text)");
    let denied = v.pointer("/result/isError").and_then(|b| b.as_bool()).unwrap_or(false);
    println!("\n### {label}\n{}{}", if denied { "[DENIED] " } else { "" }, text);
}

fn main() {
    let repo = "citrate-memories";

    // --- Agent A: a working memory, write grant, signing identity ---
    let store_a = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
    let mut srv_a = MemoryMcpServer::new_with_asserter(&store_a, write_grant(repo, 9), Asserter::new(SigningKey::from_bytes(&[9u8; 32])));

    println!("== Agent A asserts what it learned ==");
    for content in [
        "We chose RocksDB because WriteBatch gives atomic + sync durability (audit REM-2).",
        "Node identity excludes embeddings, so re-embedding never breaks edges.",
    ] {
        show("assert", &srv_a.handle_line(&call("memory.assert", json!({"repo": repo, "content": content, "kind": "rationale"}))).unwrap());
    }
    show("recall (Agent A sees its assertions)", &srv_a.handle_line(&call("memory.recall", json!({"repo": repo, "budget": 5}))).unwrap());

    // --- Agent A emits a memory-diff (the handoff artifact) ---
    let asserter = Asserter::new(SigningKey::from_bytes(&[9u8; 32]));
    let mut diff = MemoryDiff::new(asserter.pubkey_hex(), 1000);
    diff.add_node(asserter.assert_node(
        repo,
        NodeKind::Rationale,
        "Handoff: Belnap 'Both' means contradiction — agents must stop and resolve, never auto-pick.",
        1000,
    ));
    let diff_json = diff.to_json().expect("serialize diff");
    println!("\n== memory-diff handoff artifact ({} bytes, signed, verifiable) ==", diff_json.len());

    // --- Agent B: fresh memory, receives and merges the handoff ---
    let store_b = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
    let mut srv_b = MemoryMcpServer::new_with_asserter(&store_b, write_grant(repo, 5), Asserter::new(SigningKey::from_bytes(&[5u8; 32])));
    show("Agent B merges Agent A's diff", &srv_b.handle_line(&call("memory.merge_diff", json!({"diff": diff_json}))).unwrap());
    show("recall (Agent B now has the handed-off knowledge)", &srv_b.handle_line(&call("memory.recall", json!({"repo": repo, "budget": 5}))).unwrap());

    // --- blame: who asserted the merged node ---
    let merged = store_b.all_nodes().unwrap_or_default();
    if let Some(n) = merged.iter().find(|n| String::from_utf8_lossy(&n.content).contains("Belnap")) {
        let b = mem_assert::blame(n);
        println!("\n== blame ==\n  node by author {}… ({:?}, {:?})", &b.author[..16], b.plane, b.trust_tier);
    }

    println!("\n== Agent A audit log ==");
    for r in srv_a.audit().records() {
        println!("  #{} {:?}  {}  ({})", r.sequence, r.event, r.resource_id, r.detail);
    }
}
