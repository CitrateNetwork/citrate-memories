//! `mem-mcp` — an MCP server exposing the memory read path (WP-2.1/2.4).
//!
//! Speaks JSON-RPC 2.0 (newline-delimited, the MCP stdio framing): `initialize`,
//! `tools/list`, `tools/call`. Tools (`memory.recall`, `memory.search`,
//! `memory.neighbors`) run [`mem_query::Recall`] over the DAG. Every call is gated
//! by a [`CapabilityGrant`] (resource-scoped read check) and recorded in an
//! [`AuditChain`] — including denials. Write tools (`assert`/`diff`) are a later WP.

use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use std::collections::BTreeSet;

use serde_json::{json, Value};

use std::sync::{Arc, Mutex};

use mem_assert::{apply_diff, Asserter, MemoryDiff};
use mem_authz::{AuditChain, CapabilityGrant, MemoryEvent, Op};
use mem_core::{ClaimStatus, MemoryNode, NodeKind};
use mem_index::Embedder;
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
    store: &'a MemoryDagStore<MemoryNode>,
    grant: CapabilityGrant,
    /// The session agent's signing identity. `None` → read-only (assert tools error).
    asserter: Option<Asserter>,
    /// Query embedder for `memory.search`, loaded once and shared. `None` → the
    /// hashing baseline; set to the model the store was embedded with (via
    /// [`with_query_embedder`](MemoryMcpServer::with_query_embedder)) so the index's
    /// model-version guard lines up instead of rejecting every search.
    query_embedder: Option<Arc<dyn Embedder>>,
    /// Serializes store mutations across concurrent sessions. The store's reads
    /// are snapshot-consistent and every write lands as one atomic batch, but
    /// `merge_diff` does check-then-write (cycle guard), so two sessions writing
    /// at once must take turns. `None` → single-session (stdio), no gate needed.
    write_gate: Option<Arc<Mutex<()>>>,
    audit: AuditChain,
}

impl<'a> MemoryMcpServer<'a> {
    /// Read-only server (no signing identity; write tools are unavailable).
    pub fn new(store: &'a MemoryDagStore<MemoryNode>, grant: CapabilityGrant) -> Self {
        Self {
            store,
            grant,
            asserter: None,
            query_embedder: None,
            write_gate: None,
            audit: AuditChain::new(),
        }
    }

    /// Read+write server: the `asserter` signs every assertion this session makes.
    pub fn new_with_asserter(
        store: &'a MemoryDagStore<MemoryNode>,
        grant: CapabilityGrant,
        asserter: Asserter,
    ) -> Self {
        Self {
            store,
            grant,
            asserter: Some(asserter),
            query_embedder: None,
            write_gate: None,
            audit: AuditChain::new(),
        }
    }

    /// Use `embedder` for `memory.search` (must match the model the store was
    /// embedded with). Load it once and share it for the whole session.
    pub fn with_query_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.query_embedder = Some(embedder);
        self
    }

    /// Share `gate` with every server bound to the same store: each write tool
    /// holds it for the whole mutation, so concurrent sessions cannot interleave
    /// check-then-write sequences. Reads run lock-free.
    pub fn with_write_gate(mut self, gate: Arc<Mutex<()>>) -> Self {
        self.write_gate = Some(gate);
        self
    }

    /// Acquire the cross-session write gate (no-op when ungated). A poisoned gate
    /// means another session panicked mid-write; refuse to write past it.
    fn lock_writes(&self) -> Result<Option<std::sync::MutexGuard<'_, ()>>, Value> {
        match &self.write_gate {
            None => Ok(None),
            Some(g) => match g.lock() {
                Ok(guard) => Ok(Some(guard)),
                Err(_) => Err(tool_error("write gate poisoned; daemon needs a restart".to_string())),
            },
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
            "memory.assert" => self.call_assert(&args),
            "memory.merge_diff" => self.call_merge_diff(&args),
            other => Err((-32602, format!("unknown tool: {other}"))),
        }
    }

    /// Resource-scoped op check + audit. Returns `Ok(())` if allowed, or a
    /// ready-to-return tool error if denied (and records the denial).
    fn authorize(&mut self, op: Op, repo: &str, detail: &str) -> Result<(), Value> {
        let resource = format!("repo:{repo}/memory");
        let now = now_ms();
        match self.grant.check(&resource, op, now) {
            Ok(()) => {
                let event = match op {
                    Op::Read => MemoryEvent::Read,
                    Op::Write => MemoryEvent::Write,
                };
                self.audit.append(event, self.grant.recipient.clone(), resource, detail.to_string(), now);
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

    fn authorize_read(&mut self, repo: &str, detail: &str) -> Result<(), Value> {
        self.authorize(Op::Read, repo, detail)
    }

    fn call_recall(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let budget = arg_usize(args, "budget", 15);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.recall budget={budget}")) {
            return Ok(deny);
        }
        let result = Recall::new(self.store).storyline(&repo, budget).map_err(store_err)?;
        Ok(tool_text(render_result(&result)))
    }

    fn call_search(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let query = arg_str(args, "query")?;
        let budget = arg_usize(args, "budget", 10);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.search {query:?}")) {
            return Ok(deny);
        }
        let recall = match &self.query_embedder {
            Some(e) => Recall::with_embedder(self.store, Box::new(Arc::clone(e))),
            None => Recall::new(self.store),
        };
        let result = recall.search(&repo, &query, budget).map_err(store_err)?;
        Ok(tool_text(render_result(&result)))
    }

    fn call_neighbors(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let prefix = arg_str(args, "id_prefix")?;
        let budget = arg_usize(args, "budget", 20);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.neighbors {prefix}")) {
            return Ok(deny);
        }
        let recall = Recall::new(self.store);
        let id = match recall.resolve_prefix(&prefix).map_err(store_err)? {
            Some(id) => id,
            None => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        };
        let neighbors = recall.neighbors(&id, budget).map_err(store_err)?;
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

    /// Append a signed assertion to the Asserted plane (write-gated).
    fn call_assert(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let content = arg_str(args, "content")?;
        let kind_str = arg_str(args, "kind").unwrap_or_else(|_| "rationale".to_string());

        if self.asserter.is_none() {
            return Ok(tool_error("server has no signing identity; assert unavailable".to_string()));
        }
        if let Err(deny) = self.authorize(Op::Write, &repo, &format!("memory.assert {kind_str}")) {
            return Ok(deny);
        }
        let now = now_ms();
        let node = match self.asserter.as_ref() {
            Some(a) => a.assert_node(&repo, node_kind_from_str(&kind_str), &content, now),
            None => return Ok(tool_error("server has no signing identity".to_string())),
        };
        let id = node.compute_id();
        let _writes = match self.lock_writes() {
            Ok(guard) => guard,
            Err(deny) => return Ok(deny),
        };
        self.store.put_node(&node).map_err(store_err)?;
        Ok(tool_text(format!(
            "asserted {} [{}] in {repo} by {}",
            &id.to_hex()[..12],
            node.kind.discriminant(),
            self.grant.recipient
        )))
    }

    /// Merge a signed memory-diff (session subgraph). Write-gated on every repo the
    /// diff touches; rejected wholesale if any signature fails.
    fn call_merge_diff(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let diff_json = arg_str(args, "diff")?;
        let diff = match MemoryDiff::from_json(&diff_json) {
            Ok(d) => d,
            Err(e) => return Ok(tool_error(format!("malformed diff: {e}"))),
        };
        let repos: BTreeSet<String> = diff.nodes.iter().map(|n| n.repo.clone()).collect();
        for repo in &repos {
            if let Err(deny) = self.authorize(Op::Write, repo, &format!("memory.merge_diff ({} nodes)", diff.nodes.len())) {
                return Ok(deny);
            }
        }
        let _writes = match self.lock_writes() {
            Ok(guard) => guard,
            Err(deny) => return Ok(deny),
        };
        match apply_diff(self.store, &diff) {
            Ok(report) => Ok(tool_text(format!("merged {} nodes, {} edges", report.nodes, report.edges))),
            Err(e) => Ok(tool_error(format!("merge rejected: {e}"))),
        }
    }
}

fn node_kind_from_str(s: &str) -> NodeKind {
    match s.to_ascii_lowercase().as_str() {
        "claim" => NodeKind::Claim(ClaimStatus::Confirmed),
        "analogy" | "analogy_hypothesis" => NodeKind::AnalogyHypothesis,
        "doc" | "note" => NodeKind::Doc,
        _ => NodeKind::Rationale,
    }
}

/// Drive an [`MemoryMcpServer`] over any line-oriented transport: one JSON-RPC
/// message per line in, one per line out. Returns when the reader reaches EOF
/// (the client closed its end). This is the unit a multi-session daemon runs
/// once per accepted connection — each connection gets its own server (its own
/// grant + audit chain) over the shared store.
pub fn serve_connection<R: BufRead, W: Write>(
    reader: R,
    mut writer: W,
    server: &mut MemoryMcpServer,
) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_line(&line) {
            writeln!(writer, "{resp}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Drive an [`MemoryMcpServer`] over stdio (the MCP stdio transport): one JSON-RPC
/// message per line in, one per line out.
pub fn serve_stdio(server: &mut MemoryMcpServer) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_connection(stdin.lock(), stdout.lock(), server)
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
        },
        {
            "name": "memory.assert",
            "description": "Append a signed assertion (Asserted plane) — a rationale/claim/note that lives nowhere else. Requires write scope.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "content": { "type": "string", "description": "the assertion text" },
                    "kind": { "type": "string", "enum": ["rationale", "claim", "analogy", "note"], "default": "rationale" }
                },
                "required": ["repo", "content"]
            }
        },
        {
            "name": "memory.merge_diff",
            "description": "Merge a signed memory-diff (a session subgraph) into the graph — the 'git for agents' handoff. Requires write scope on the diff's repos; rejected wholesale if any signature fails.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "diff": { "type": "string", "description": "JSON-encoded MemoryDiff" }
                },
                "required": ["diff"]
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
    use mem_assert::Asserter;

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
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 5);
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

    /// Grant with write on citrate-chain.
    fn write_grant() -> CapabilityGrant {
        let mut g = grant();
        g.allowed_resources[0].can_write = true;
        g.sign_with(&SigningKey::from_bytes(&[3u8; 32]));
        g
    }

    fn asserter() -> Asserter {
        Asserter::new(SigningKey::from_bytes(&[8u8; 32]))
    }

    const ASSERT_CALL: &str = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"memory.assert","arguments":{"repo":"citrate-chain","content":"we chose rocksdb for durable atomic batches","kind":"rationale"}}}"#;

    #[test]
    fn assert_with_write_grant_appends_and_audits() {
        let s = store();
        let before = s.node_count().unwrap();
        let mut srv = MemoryMcpServer::new_with_asserter(&s, write_grant(), asserter());
        let v: Value = serde_json::from_str(&srv.handle_line(ASSERT_CALL).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], false);
        assert_eq!(s.node_count().unwrap(), before + 1, "assertion appended to the store");
        assert_eq!(srv.audit().records()[0].event, MemoryEvent::Write);
    }

    #[test]
    fn assert_denied_without_write_scope() {
        let s = store();
        // read-only grant + asserter present
        let mut srv = MemoryMcpServer::new_with_asserter(&s, grant(), asserter());
        let v: Value = serde_json::from_str(&srv.handle_line(ASSERT_CALL).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], true, "assert without write scope must be denied");
        assert_eq!(srv.audit().records()[0].event, MemoryEvent::Denied);
        assert_eq!(s.node_count().unwrap(), 2, "nothing written");
    }

    #[test]
    fn assert_without_signing_identity_errors() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, write_grant()); // no asserter
        let v: Value = serde_json::from_str(&srv.handle_line(ASSERT_CALL).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("signing identity"));
    }

    /// Drive one full client session over a real socket: write requests on one end
    /// of a `UnixStream` pair, run `serve_connection` on the other. Returns the
    /// parsed responses in order.
    #[cfg(unix)]
    fn run_session(srv: &mut MemoryMcpServer, requests: &[&str]) -> Vec<Value> {
        use std::io::BufReader;
        use std::os::unix::net::UnixStream;
        let (client, server_end) = UnixStream::pair().expect("socketpair");
        std::thread::scope(|scope| {
            let handle = scope.spawn(move || {
                let reader = BufReader::new(server_end.try_clone().expect("clone stream"));
                serve_connection(reader, server_end, srv).expect("serve_connection");
            });
            let mut w = client.try_clone().expect("clone client");
            for req in requests {
                use std::io::Write as _;
                writeln!(w, "{req}").expect("write request");
            }
            client.shutdown(std::net::Shutdown::Write).expect("shutdown write");
            use std::io::BufRead as _;
            let responses: Vec<Value> = BufReader::new(client)
                .lines()
                .map(|l| serde_json::from_str(&l.expect("read response")).expect("parse response"))
                .collect();
            handle.join().expect("server thread");
            responses
        })
    }

    #[cfg(unix)]
    #[test]
    fn full_session_over_unix_socket() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        let responses = run_session(
            &mut srv,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-chain","budget":5}}}"#,
            ],
        );
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "citrate-memories");
        assert_eq!(responses[1]["result"]["isError"], false);
        let text = responses[1]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("ghostdag"));
    }

    /// Two sessions on the same store at the same time — the multi-session daemon
    /// shape. Both write through one shared gate; both writes must land, and each
    /// session keeps its own audit chain.
    #[cfg(unix)]
    #[test]
    fn two_concurrent_sessions_share_one_store() {
        let s = store();
        let before = s.node_count().unwrap();
        let gate = Arc::new(Mutex::new(()));

        let mut srv_a = MemoryMcpServer::new_with_asserter(&s, write_grant(), asserter())
            .with_write_gate(Arc::clone(&gate));
        let mut srv_b = MemoryMcpServer::new_with_asserter(
            &s,
            write_grant(),
            Asserter::new(SigningKey::from_bytes(&[9u8; 32])),
        )
        .with_write_gate(Arc::clone(&gate));

        let call_b = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"memory.assert","arguments":{"repo":"citrate-chain","content":"session B: the daemon serializes writes through one gate","kind":"note"}}}"#;
        std::thread::scope(|scope| {
            let a = scope.spawn(move || run_session(&mut srv_a, &[ASSERT_CALL]));
            let b = scope.spawn(move || run_session(&mut srv_b, &[call_b]));
            for handle in [a, b] {
                let responses = handle.join().expect("session thread");
                assert_eq!(responses[0]["result"]["isError"], false, "both sessions' writes must succeed");
            }
        });
        assert_eq!(s.node_count().unwrap(), before + 2, "both sessions' assertions landed");
    }
}
