//! `mem-mcp` — an MCP server exposing the memory read path (WP-2.1/2.4).
//!
//! Speaks JSON-RPC 2.0 (newline-delimited, the MCP stdio framing): `initialize`,
//! `tools/list`, `tools/call`. Tools (`memory.recall`, `memory.search`,
//! `memory.neighbors`) run [`mem_query::Recall`] over the DAG. Every call is gated
//! by a [`CapabilityGrant`] (resource-scoped read check) and recorded in an
//! [`AuditChain`] — including denials. Write tools (`assert`/`diff`) are a later WP.

use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use mem_authz::{AuditChain, CapabilityGrant, MemoryEvent, Op};
use mem_core::MemoryNode;
use mem_query::{Direction, Recall, RecallResult};
use mem_store::MemoryDagStore;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const PROTOCOL_VERSION: &str = "2024-11-05";

/// An MCP server bound to one DAG store and one capability grant (the session's
/// authorization). One grant per session models "this agent may touch these
/// resources".
pub struct MemoryMcpServer<'a> {
    recall: Recall<'a>,
    grant: CapabilityGrant,
    audit: AuditChain,
}

impl<'a> MemoryMcpServer<'a> {
    pub fn new(store: &'a MemoryDagStore<MemoryNode>, grant: CapabilityGrant) -> Self {
        Self {
            recall: Recall::new(store),
            grant,
            audit: AuditChain::new(),
        }
    }

    pub fn audit(&self) -> &AuditChain {
        &self.audit
    }

    /// Handle one JSON-RPC line. Returns `Some(response)` for requests and `None`
    /// for notifications (messages without an `id`).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(err_response(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let outcome = self.dispatch(method, params);
        // Notifications (no id) get no response.
        id.map(|id| match outcome {
            Ok(result) => ok_response(id, result),
            Err((code, msg)) => err_response(id, code, &msg),
        })
    }

    fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "citrate-memories", "version": env!("CARGO_PKG_VERSION") }
            })),
            "tools/list" => Ok(tools_list()),
            "tools/call" => self.tools_call(params),
            "ping" => Ok(json!({})),
            other => Err((-32601, format!("method not found: {other}"))),
        }
    }

    fn tools_call(&mut self, params: Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or((-32602, "missing tool name".to_string()))?
            .to_string();
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        match name.as_str() {
            "memory.recall" => self.call_recall(&args),
            "memory.search" => self.call_search(&args),
            "memory.neighbors" => self.call_neighbors(&args),
            other => Err((-32602, format!("unknown tool: {other}"))),
        }
    }

    /// Resource-scoped read check + audit. Returns `Ok(())` if allowed, or a
    /// ready-to-return tool error if denied (and records the denial).
    fn authorize_read(&mut self, repo: &str, detail: &str) -> Result<(), Value> {
        let resource = format!("repo:{repo}/memory");
        let now = now_ms();
        match self.grant.check(&resource, Op::Read, now) {
            Ok(()) => {
                self.audit.append(MemoryEvent::Read, self.grant.recipient.clone(), resource, detail.to_string(), now);
                Ok(())
            }
            Err(e) => {
                self.audit.append(
                    MemoryEvent::Denied,
                    self.grant.recipient.clone(),
                    resource,
                    format!("{detail}: {e}"),
                    now,
                );
                Err(tool_error(format!("authorization denied: {e}")))
            }
        }
    }

    fn call_recall(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let budget = arg_usize(args, "budget", 15);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.recall budget={budget}")) {
            return Ok(deny);
        }
        let result = self.recall.storyline(&repo, budget).map_err(store_err)?;
        Ok(tool_text(render_result(&result)))
    }

    fn call_search(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let query = arg_str(args, "query")?;
        let budget = arg_usize(args, "budget", 10);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.search {query:?}")) {
            return Ok(deny);
        }
        let result = self.recall.search(&repo, &query, budget).map_err(store_err)?;
        Ok(tool_text(render_result(&result)))
    }

    fn call_neighbors(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let prefix = arg_str(args, "id_prefix")?;
        let budget = arg_usize(args, "budget", 20);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.neighbors {prefix}")) {
            return Ok(deny);
        }
        let id = match self.recall.resolve_prefix(&prefix).map_err(store_err)? {
            Some(id) => id,
            None => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        };
        let neighbors = self.recall.neighbors(&id, budget).map_err(store_err)?;
        let mut text = format!("neighbors of {}:\n", &id.to_hex()[..12]);
        for nb in &neighbors {
            let arrow = match nb.direction {
                Direction::Out => "->",
                Direction::In => "<-",
            };
            let title = nb.node.as_ref().map(|n| n.title.replace('\n', " ")).unwrap_or_else(|| "(dangling)".into());
            text.push_str(&format!("  {arrow} [{:?}] {}\n", nb.edge_kind, title));
        }
        Ok(tool_text(text))
    }
}

/// Drive an [`MemoryMcpServer`] over stdio (the MCP stdio transport): one JSON-RPC
/// message per line in, one per line out.
pub fn serve_stdio(server: &mut MemoryMcpServer) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_line(&line) {
            writeln!(out, "{resp}")?;
            out.flush()?;
        }
    }
    Ok(())
}

// ---- helpers ----

fn arg_str(args: &Value, key: &str) -> Result<String, (i64, String)> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or((-32602, format!("missing string argument '{key}'")))
}

fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key).and_then(|v| v.as_u64()).map(|n| n as usize).unwrap_or(default)
}

fn store_err(e: mem_store::StoreError) -> (i64, String) {
    (-32000, e.to_string())
}

fn tools_list() -> Value {
    json!({ "tools": [
        {
            "name": "memory.recall",
            "description": "Budget-shaped storyline (most recent N memory nodes) for a repo tenant. Carries provenance + freshness.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "tenant repo, e.g. citrate-chain" },
                    "budget": { "type": "integer", "description": "max items", "default": 15 }
                },
                "required": ["repo"]
            }
        },
        {
            "name": "memory.search",
            "description": "Semantic search (cosine over node embeddings) within a repo tenant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "query": { "type": "string" },
                    "budget": { "type": "integer", "default": 10 }
                },
                "required": ["repo", "query"]
            }
        },
        {
            "name": "memory.neighbors",
            "description": "Blast-radius: nodes connected by one edge to a node (by id prefix) in a repo tenant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "id_prefix": { "type": "string" },
                    "budget": { "type": "integer", "default": 20 }
                },
                "required": ["repo", "id_prefix"]
            }
        }
    ]})
}

fn render_result(r: &RecallResult) -> String {
    let mut s = String::new();
    match &r.watermark {
        Some(w) => {
            let head = w.head.as_deref().unwrap_or("?");
            s.push_str(&format!(
                "freshness: HEAD {} ({} commits) ingested @ {}ms\n",
                &head[..head.len().min(12)],
                w.head_count,
                w.ingested_at_ms
            ));
        }
        None => s.push_str("freshness: (no watermark)\n"),
    }
    s.push_str(&format!("tenant '{}' — {} nodes, showing {}:\n", r.repo, r.total_in_tenant, r.items.len()));
    for i in &r.items {
        let score = i.score.map(|x| format!("{x:.3} ")).unwrap_or_default();
        let title = i.title.replace('\n', " ");
        let title = if title.chars().count() > 72 {
            format!("{}…", title.chars().take(71).collect::<String>())
        } else {
            title
        };
        s.push_str(&format!("  {} {}[{}] {}\n", &i.id.to_hex()[..10], score, i.kind.discriminant(), title));
    }
    s
}

fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": false })
}

fn tool_error(message: String) -> Value {
    json!({ "content": [ { "type": "text", "text": message } ], "isError": true })
}

fn ok_response(id: Value, result: Value) -> String {
    serde_json::to_string(&json!({ "jsonrpc": "2.0", "id": id, "result": result })).unwrap_or_default()
}

fn err_response(id: Value, code: i64, message: &str) -> String {
    serde_json::to_string(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
    use mem_core::{MemoryNode, NodeKind, Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION};
    use mem_store::kv::InMemoryKv;

    use ed25519_dalek::SigningKey;

    fn node(repo: &str, subject: &str) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Commit,
            repo: repo.into(),
            author: "t".into(),
            source_ref: SourceRef::GitCommit { repo: repo.into(), sha: subject.into() },
            content: subject.as_bytes().to_vec(),
            valid_from: 1,
            valid_to: None,
            observed_at: 1,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: None,
            confidence: vec![],
            anchors: vec![],
            status: Status::Active,
        }
    }

    fn store() -> MemoryDagStore<MemoryNode> {
        let s = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        s.commit(&[node("citrate-chain", "ghostdag tip selection"), node("citrate-identity", "siwe login")], &[])
            .unwrap();
        s
    }

    /// Grant: read on citrate-chain only, far-future expiry, signed.
    fn grant() -> CapabilityGrant {
        let mut g = CapabilityGrant {
            id: "g1".into(),
            issuer: "did:human".into(),
            recipient: "agent:test".into(),
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
        g.sign_with(&SigningKey::from_bytes(&[3u8; 32]));
        g
    }

    #[test]
    fn initialize_and_tools_list() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());

        let init = srv.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).unwrap();
        let v: Value = serde_json::from_str(&init).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "citrate-memories");

        let list = srv.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let v: Value = serde_json::from_str(&list).unwrap();
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn authorized_recall_succeeds_and_is_audited() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-chain","budget":5}}}"#;
        let resp = srv.handle_line(call).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("ghostdag"), "recall should surface the chain node");
        assert_eq!(srv.audit().records()[0].event, MemoryEvent::Read);
    }

    #[test]
    fn unauthorized_repo_is_denied_and_audited() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        let call = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-identity"}}}"#;
        let resp = srv.handle_line(call).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], true, "recall on ungranted repo must be denied");
        assert_eq!(srv.audit().records()[0].event, MemoryEvent::Denied);
        assert_eq!(srv.audit().verify_integrity(), Ok(1));
    }

    #[test]
    fn notifications_get_no_response() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        // No "id" → notification.
        assert!(srv.handle_line(r#"{"jsonrpc":"2.0","method":"initialize"}"#).is_none());
    }

    #[test]
    fn unknown_method_is_jsonrpc_error() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        let resp = srv.handle_line(r#"{"jsonrpc":"2.0","id":9,"method":"bogus"}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }
}
