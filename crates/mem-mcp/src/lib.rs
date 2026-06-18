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
use mem_core::{ClaimStatus, EdgeKind, EdgeMethod, MemoryNode, NodeKind};
use mem_index::Embedder;
use mem_query::{Direction, Recall, RecallResult, TenantIndexCache};
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
    /// Shared per-tenant HNSW cache: `memory.search` reuses one index across
    /// queries (and, in the daemon, across sessions) instead of scanning +
    /// brute-forcing the tenant per call. `None` → exact per-query search.
    index_cache: Option<Arc<TenantIndexCache>>,
    /// Serializes store mutations across concurrent sessions. The store's reads
    /// are snapshot-consistent and every write lands as one atomic batch, but
    /// `merge_diff` does check-then-write (cycle guard), so two sessions writing
    /// at once must take turns. `None` → single-session (stdio), no gate needed.
    write_gate: Option<Arc<Mutex<()>>>,
    /// Hash-chained audit log. Shared (`Arc<Mutex<…>>`) so the daemon can bind
    /// every session to ONE persistent chain (`AuditChain::open`) that survives
    /// restart; defaults to a fresh in-memory chain per server.
    audit: Arc<Mutex<AuditChain>>,
}

impl<'a> MemoryMcpServer<'a> {
    /// Read-only server (no signing identity; write tools are unavailable).
    pub fn new(store: &'a MemoryDagStore<MemoryNode>, grant: CapabilityGrant) -> Self {
        Self {
            store,
            grant,
            asserter: None,
            query_embedder: None,
            index_cache: None,
            write_gate: None,
            audit: Arc::new(Mutex::new(AuditChain::new())),
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
            index_cache: None,
            write_gate: None,
            audit: Arc::new(Mutex::new(AuditChain::new())),
        }
    }

    /// Use `embedder` for `memory.search` (must match the model the store was
    /// embedded with). Load it once and share it for the whole session.
    pub fn with_query_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.query_embedder = Some(embedder);
        self
    }

    /// Share `cache` with every server bound to the same store: `memory.search`
    /// builds each tenant's HNSW index once and every session reuses it until
    /// the store changes.
    pub fn with_index_cache(mut self, cache: Arc<TenantIndexCache>) -> Self {
        self.index_cache = Some(cache);
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

    /// Bind this session to `audit` — share one persistent chain
    /// ([`AuditChain::open`]) across every session so the log survives restart.
    pub fn with_audit_chain(mut self, audit: Arc<Mutex<AuditChain>>) -> Self {
        self.audit = audit;
        self
    }

    /// The session's audit chain (locked). Panics only if another thread
    /// panicked while appending — at which point the log is suspect anyway.
    pub fn audit(&self) -> std::sync::MutexGuard<'_, AuditChain> {
        self.audit.lock().expect("audit chain lock poisoned")
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
            "memory.as_of" => self.call_as_of(&args),
            "memory.verify" => self.call_verify(&args),
            "memory.critique" => self.call_critique(&args),
            "memory.analogy" => self.call_analogy(&args),
            "memory.assert" => self.call_assert(&args),
            "memory.merge_diff" => self.call_merge_diff(&args),
            "memory.propose_edge" => self.call_propose_edge(&args),
            "memory.confirm_edge" => self.call_confirm_edge(&args),
            other => Err((-32602, format!("unknown tool: {other}"))),
        }
    }

    /// Resource-scoped op check + audit. Returns `Ok(())` if allowed, or a
    /// ready-to-return tool error if denied (and records the denial).
    fn authorize(&mut self, op: Op, repo: &str, detail: &str) -> Result<(), Value> {
        let resource = format!("repo:{repo}/memory");
        let now = now_ms();
        // Fail closed: an operation that cannot be audited does not run.
        let mut audit = match self.audit.lock() {
            Ok(g) => g,
            Err(_) => return Err(tool_error("audit chain lock poisoned; refusing to proceed".to_string())),
        };
        match self.grant.check(&resource, op, now) {
            Ok(()) => {
                let event = match op {
                    Op::Read => MemoryEvent::Read,
                    Op::Write => MemoryEvent::Write,
                };
                audit
                    .append(event, self.grant.recipient.clone(), resource, detail.to_string(), now)
                    .map_err(|e| tool_error(format!("audit append failed; refusing to proceed: {e}")))?;
                Ok(())
            }
            Err(e) => {
                if let Err(ae) = audit.append(
                    MemoryEvent::Denied,
                    self.grant.recipient.clone(),
                    resource,
                    format!("{detail}: {e}"),
                    now,
                ) {
                    return Err(tool_error(format!(
                        "authorization denied: {e} (and the denial could not be audited: {ae})"
                    )));
                }
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
        let mut recall = match &self.query_embedder {
            Some(e) => Recall::with_embedder(self.store, Box::new(Arc::clone(e))),
            None => Recall::new(self.store),
        };
        if let Some(cache) = &self.index_cache {
            recall = recall.with_index_cache(Arc::clone(cache));
        }
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
        // FUA-MEMORIES-01: `resolve_prefix` matches across ALL tenants, so a
        // session authorized on `repo` could pass a prefix of a node in another
        // tenant and read its connected content. Require the resolved node to
        // belong to the authorized repo (a cross-tenant hit reads as "not found"
        // so existence isn't leaked).
        match self.store.get_node(&id).map_err(store_err)? {
            Some(n) if n.repo == repo => {}
            _ => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        }
        let neighbors = recall.neighbors(&id, budget).map_err(store_err)?;
        let mut text = format!("neighbors of {}:\n", &id.to_hex()[..12]);
        for nb in &neighbors {
            // FUA-MEMORIES-01, refined to grant intersection (MEM-S4 WP-4.2, R3):
            // a neighbour in another tenant (e.g. via a cross-DAG AnalogousTo
            // edge) is shown iff this session's grant can READ that tenant.
            // Unauthorized tenants are silently dropped — neither content nor
            // existence leaks.
            if let Some(n) = nb.node.as_ref() {
                if n.repo != repo && !self.can_read(&n.repo) {
                    continue;
                }
            }
            let arrow = match nb.direction {
                Direction::Out => "->",
                Direction::In => "<-",
            };
            // A quarantined edge is a proposal — advisory until confirmed (WP-4.1).
            let proposed = if nb.quarantined { " (proposed)" } else { "" };
            let cross = nb
                .node
                .as_ref()
                .filter(|n| n.repo != repo)
                .map(|n| format!(" @{}", n.repo))
                .unwrap_or_default();
            let title = nb.node.as_ref().map(|n| n.title.replace('\n', " ")).unwrap_or_else(|| "(dangling)".into());
            text.push_str(&format!("  {arrow} [{:?}{proposed}]{cross} {}\n", nb.edge_kind, title));
        }
        Ok(tool_text(text))
    }

    /// `memory.as_of` (WP-3.4): decision-replay — the Derived-plane snapshot of a
    /// tenant as it stood at a point in time. Tenant-scoped + audited like recall.
    fn call_as_of(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let as_of_ms = args
            .get("as_of_ms")
            .and_then(|v| v.as_u64())
            .ok_or((-32602, "missing integer argument 'as_of_ms' (epoch ms)".to_string()))?;
        let budget = arg_usize(args, "budget", 15);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.as_of t={as_of_ms} budget={budget}")) {
            return Ok(deny);
        }
        let result = Recall::new(self.store).as_of(&repo, as_of_ms, budget).map_err(store_err)?;
        let mut text = format!("as-of {as_of_ms}ms — Derived-plane snapshot:\n");
        text.push_str(&render_result(&result));
        Ok(tool_text(text))
    }

    /// `memory.verify` (WP-2.4): provenance check for one node — signature posture,
    /// supersession/refutation, contradiction. Tenant-scoped: the resolved node
    /// must belong to the authorized repo (cross-tenant hit reads as "not found",
    /// matching `memory.neighbors`' FUA-MEMORIES-01 guard).
    fn call_verify(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let prefix = arg_str(args, "id_prefix")?;
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.verify {prefix}")) {
            return Ok(deny);
        }
        let recall = Recall::new(self.store);
        let id = match recall.resolve_prefix(&prefix).map_err(store_err)? {
            Some(id) => id,
            None => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        };
        // Don't leak existence of a node in another tenant.
        match self.store.get_node(&id).map_err(store_err)? {
            Some(n) if n.repo == repo => {}
            _ => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        }
        let v = match recall.verify(&id).map_err(store_err)? {
            Some(v) => v,
            None => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        };
        let sig = match &v.signature {
            mem_query::SignatureStatus::NotApplicableDerived => "derived (trusted by construction)".to_string(),
            mem_query::SignatureStatus::Valid => "signature VALID".to_string(),
            mem_query::SignatureStatus::Invalid(e) => format!("signature INVALID: {e}"),
            mem_query::SignatureStatus::Missing => "signature MISSING".to_string(),
        };
        let mut text = format!(
            "verify {}:\n  trustworthy: {}\n  {}\n  plane: {:?}  tier: {:?}  status: {:?}\n",
            &id.to_hex()[..12],
            v.is_trustworthy(),
            sig,
            v.item.plane,
            v.item.trust_tier,
            v.item.status,
        );
        if !v.superseded_by.is_empty() {
            text.push_str(&format!("  ⚠ superseded by {} node(s)\n", v.superseded_by.len()));
        }
        if !v.refuted_by.is_empty() {
            text.push_str(&format!("  ⚠ refuted/contradicted by {} node(s)\n", v.refuted_by.len()));
        }
        if v.contradicted {
            text.push_str("  ⚠ Belnap contradiction (Both) recorded\n");
        }
        Ok(tool_text(text))
    }

    /// `memory.critique` (WP-3.5): run the self-critic over a fresh storyline (or a
    /// search, when `query` is given) for a tenant and report completeness gaps —
    /// what an agent acting on the result might be missing.
    fn call_critique(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let budget = arg_usize(args, "budget", 15);
        let query = args.get("query").and_then(|v| v.as_str()).map(|s| s.to_string());
        let detail = match &query {
            Some(q) => format!("memory.critique search {q:?} budget={budget}"),
            None => format!("memory.critique storyline budget={budget}"),
        };
        if let Err(deny) = self.authorize_read(&repo, &detail) {
            return Ok(deny);
        }
        let result = match &query {
            Some(q) => {
                let mut recall = match &self.query_embedder {
                    Some(e) => Recall::with_embedder(self.store, Box::new(Arc::clone(e))),
                    None => Recall::new(self.store),
                };
                if let Some(cache) = &self.index_cache {
                    recall = recall.with_index_cache(Arc::clone(cache));
                }
                recall.search(&repo, q, budget).map_err(store_err)?
            }
            None => Recall::new(self.store).storyline(&repo, budget).map_err(store_err)?,
        };
        let critique = Recall::new(self.store).critique(&result, now_ms()).map_err(store_err)?;
        let mut text = format!("self-critic over '{repo}' (completeness {:.2}):\n", critique.completeness);
        if let Some(age) = critique.watermark_age_ms {
            text.push_str(&format!("  freshness: index is {}ms behind now\n", age));
        }
        if critique.gaps.is_empty() {
            text.push_str("  no completeness gaps found.\n");
        }
        for g in &critique.gaps {
            match g {
                mem_query::Gap::TruncatedCoverage { shown, total } => {
                    text.push_str(&format!("  ⚠ truncated: showing {shown} of {total} — raise budget or narrow the query\n"));
                }
                mem_query::Gap::SupersededInResult(ids) => {
                    text.push_str(&format!("  ⚠ {} superseded node(s) in the result — stale memory\n", ids.len()));
                }
                mem_query::Gap::Contradicted(id) => {
                    text.push_str(&format!("  ⚠ contradiction recorded on {}\n", &id.to_hex()[..12]));
                }
                mem_query::Gap::AdjacentProposal(id) => {
                    text.push_str(&format!("  ⚠ unconfirmed proposal adjacent to {} — check memory.neighbors\n", &id.to_hex()[..12]));
                }
            }
        }
        Ok(tool_text(text))
    }

    /// Grant-intersection read check (R3): no audit record, used for filtering
    /// individual cross-tenant items inside an already-audited call.
    fn can_read(&self, repo: &str) -> bool {
        self.grant.check(&format!("repo:{repo}/memory"), Op::Read, now_ms()).is_ok()
    }

    /// Resolve an id prefix to a node, requiring the session to be able to read
    /// the node's tenant. Unauthorized or missing both read as "not found"
    /// (FUA-MEMORIES-01: existence must not leak).
    fn resolve_readable(&self, prefix: &str) -> Result<Option<(mem_core::ContentHash, MemoryNode)>, (i64, String)> {
        let recall = Recall::new(self.store);
        let Some(id) = recall.resolve_prefix(prefix).map_err(store_err)? else {
            return Ok(None);
        };
        match self.store.get_node(&id).map_err(store_err)? {
            Some(n) if self.can_read(&n.repo) => Ok(Some((id, n))),
            _ => Ok(None),
        }
    }

    /// Cross-tenant analogy (MEM-S4 WP-4.3): coarse embedding shortlist +
    /// structural verify, over exactly the tenants this grant can read (R3).
    fn call_analogy(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let repo = arg_str(args, "repo")?;
        let prefix = arg_str(args, "id_prefix")?;
        let budget = arg_usize(args, "budget", 5).min(50);
        if let Err(deny) = self.authorize_read(&repo, &format!("memory.analogy {prefix}")) {
            return Ok(deny);
        }
        let recall = Recall::new(self.store);
        let id = match recall.resolve_prefix(&prefix).map_err(store_err)? {
            Some(id) => id,
            None => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        };
        // Anchor must live in the authorized repo (FUA-MEMORIES-01).
        match self.store.get_node(&id).map_err(store_err)? {
            Some(n) if n.repo == repo => {}
            _ => return Ok(tool_error(format!("no unique node for prefix '{prefix}'"))),
        }
        // R3 grant intersection: candidates come only from readable tenants.
        let mut candidate_repos: BTreeSet<String> = BTreeSet::new();
        for n in self.store.all_nodes().map_err(store_err)? {
            if n.repo != repo && !candidate_repos.contains(&n.repo) && self.can_read(&n.repo) {
                candidate_repos.insert(n.repo);
            }
        }
        let candidate_repos: Vec<String> = candidate_repos.into_iter().collect();
        if candidate_repos.is_empty() {
            return Ok(tool_text("no other readable tenants to search for analogues".into()));
        }
        let hits = recall.analogies(&id, &candidate_repos, budget).map_err(store_err)?;
        let mut text = format!(
            "analogues of {} across {} readable tenant(s):\n",
            &id.to_hex()[..12],
            candidate_repos.len()
        );
        if hits.is_empty() {
            text.push_str("  (none — anchor may lack an embedding, or no candidates share its vector space)\n");
        }
        for h in &hits {
            let title = h.item.title.replace('\n', " ");
            let title = if title.chars().count() > 60 {
                format!("{}…", title.chars().take(59).collect::<String>())
            } else {
                title
            };
            text.push_str(&format!(
                "  {} {:.3} (cos {:.3}, struct {:.2}) [{}] @{} {}\n",
                &h.item.id.to_hex()[..10],
                h.score,
                h.cosine,
                h.structural,
                h.item.kind.discriminant(),
                h.item.repo,
                title
            ));
        }
        text.push_str("(use memory.propose_edge kind=analogous_to to record one as a quarantined proposal)\n");
        Ok(tool_text(text))
    }

    /// Record a signed, QUARANTINED edge proposal (MEM-S4 WP-4.1). Write-gated
    /// on BOTH endpoint tenants (the edge lands in both adjacency views). The
    /// proposal is advisory until `memory.confirm_edge` promotes it.
    fn call_propose_edge(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let from_prefix = arg_str(args, "from_prefix")?;
        let to_prefix = arg_str(args, "to_prefix")?;
        let kind_str = arg_str(args, "kind").unwrap_or_else(|_| "analogous_to".to_string());
        let evidence = arg_str(args, "evidence").ok();
        let Some(kind) = edge_kind_from_str(&kind_str) else {
            return Ok(tool_error(format!("unknown edge kind '{kind_str}'")));
        };
        if self.asserter.is_none() {
            return Ok(tool_error("server has no signing identity; propose unavailable".to_string()));
        }
        let Some((from_id, from_node)) = self.resolve_readable(&from_prefix)? else {
            return Ok(tool_error(format!("no unique node for prefix '{from_prefix}'")));
        };
        let Some((to_id, to_node)) = self.resolve_readable(&to_prefix)? else {
            return Ok(tool_error(format!("no unique node for prefix '{to_prefix}'")));
        };
        let repos: BTreeSet<String> = [from_node.repo.clone(), to_node.repo.clone()].into();
        for r in &repos {
            if let Err(deny) = self.authorize(Op::Write, r, &format!("memory.propose_edge {kind_str}")) {
                return Ok(deny);
            }
        }
        let edge = match self.asserter.as_ref() {
            Some(a) => a.propose_edge(from_id, to_id, kind, EdgeMethod::Nlp, evidence, now_ms()),
            None => return Ok(tool_error("server has no signing identity".to_string())),
        };
        let _writes = match self.lock_writes() {
            Ok(guard) => guard,
            Err(deny) => return Ok(deny),
        };
        self.store.add_edge(&edge).map_err(store_err)?;
        Ok(tool_text(format!(
            "proposed (quarantined) {} -{kind_str}-> {} — promote with memory.confirm_edge",
            &from_id.to_hex()[..10],
            &to_id.to_hex()[..10]
        )))
    }

    /// Promote a quarantined proposal to load-bearing (MEM-S4 WP-4.1).
    /// Write-gated on both endpoint tenants; a confirmed Supersedes applies
    /// the status transition (cycle-guarded) atomically.
    fn call_confirm_edge(&mut self, args: &Value) -> Result<Value, (i64, String)> {
        let from_prefix = arg_str(args, "from_prefix")?;
        let to_prefix = arg_str(args, "to_prefix")?;
        let kind_str = arg_str(args, "kind").unwrap_or_else(|_| "analogous_to".to_string());
        let Some(kind) = edge_kind_from_str(&kind_str) else {
            return Ok(tool_error(format!("unknown edge kind '{kind_str}'")));
        };
        let Some((from_id, from_node)) = self.resolve_readable(&from_prefix)? else {
            return Ok(tool_error(format!("no unique node for prefix '{from_prefix}'")));
        };
        let Some((to_id, to_node)) = self.resolve_readable(&to_prefix)? else {
            return Ok(tool_error(format!("no unique node for prefix '{to_prefix}'")));
        };
        let repos: BTreeSet<String> = [from_node.repo.clone(), to_node.repo.clone()].into();
        for r in &repos {
            if let Err(deny) = self.authorize(Op::Write, r, &format!("memory.confirm_edge {kind_str}")) {
                return Ok(deny);
            }
        }
        let _writes = match self.lock_writes() {
            Ok(guard) => guard,
            Err(deny) => return Ok(deny),
        };
        match self.store.confirm_edge(&from_id, &to_id, kind) {
            Ok(mem_store::ConfirmOutcome::Confirmed) => Ok(tool_text(format!(
                "confirmed {} -{kind_str}-> {} (now load-bearing)",
                &from_id.to_hex()[..10],
                &to_id.to_hex()[..10]
            ))),
            Ok(mem_store::ConfirmOutcome::AlreadyConfirmed) => {
                Ok(tool_text("edge is already load-bearing".to_string()))
            }
            Ok(mem_store::ConfirmOutcome::NotFound) => Ok(tool_error("no such edge".to_string())),
            Err(e) => Ok(tool_error(format!("confirm rejected: {e}"))),
        }
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
        // FUA-MEMORIES-05: bound the work an unauthenticated-shaped payload can
        // force — cap the raw bytes before parsing and the node/edge counts after.
        const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
        const MAX_DIFF_NODES: usize = 10_000;
        const MAX_DIFF_EDGES: usize = 50_000;
        if diff_json.len() > MAX_DIFF_BYTES {
            return Ok(tool_error(format!(
                "diff too large ({} bytes > {MAX_DIFF_BYTES} cap)",
                diff_json.len()
            )));
        }
        let diff = match MemoryDiff::from_json(&diff_json) {
            Ok(d) => d,
            Err(e) => return Ok(tool_error(format!("malformed diff: {e}"))),
        };
        if diff.nodes.len() > MAX_DIFF_NODES || diff.edges.len() > MAX_DIFF_EDGES {
            return Ok(tool_error(format!(
                "diff exceeds size cap ({} nodes, {} edges)",
                diff.nodes.len(),
                diff.edges.len()
            )));
        }
        // FUA-MEMORIES-03: a zero-node diff carrying only edges previously ran
        // the authz loop zero times and committed every edge unchecked. Reject a
        // no-op diff, and authorize the repo of EVERY edge endpoint (resolved
        // from the diff's own nodes, else from the store) — not just node repos.
        if diff.nodes.is_empty() && diff.edges.is_empty() {
            return Ok(tool_error("empty diff (no nodes, no edges)".to_string()));
        }
        let in_diff: std::collections::BTreeMap<_, String> = diff
            .nodes
            .iter()
            .map(|n| (n.compute_id(), n.repo.clone()))
            .collect();
        let mut repos: BTreeSet<String> = diff.nodes.iter().map(|n| n.repo.clone()).collect();
        for e in &diff.edges {
            for endpoint in [&e.from, &e.to] {
                let repo = match in_diff.get(endpoint) {
                    Some(r) => r.clone(),
                    None => match self.store.get_node(endpoint).map_err(store_err)? {
                        Some(n) => n.repo.clone(),
                        None => {
                            return Ok(tool_error(format!(
                                "edge references unknown node {}; cannot authorize",
                                &endpoint.to_hex()[..12]
                            )))
                        }
                    },
                };
                repos.insert(repo);
            }
        }
        for repo in &repos {
            if let Err(deny) = self.authorize(Op::Write, repo, &format!("memory.merge_diff ({} nodes, {} edges)", diff.nodes.len(), diff.edges.len())) {
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

/// Edge kinds an MCP client may propose/confirm. Structural ingest kinds
/// (TemporalNext/MergeParent) are deliberately absent — those only come from
/// deterministic ingestion.
fn edge_kind_from_str(s: &str) -> Option<EdgeKind> {
    match s.to_ascii_lowercase().as_str() {
        "analogous_to" | "analogousto" | "analogy" => Some(EdgeKind::AnalogousTo),
        "supersedes" => Some(EdgeKind::Supersedes),
        "references" => Some(EdgeKind::References),
        "depends_on" | "dependson" => Some(EdgeKind::DependsOn),
        "implements" => Some(EdgeKind::Implements),
        "decides" => Some(EdgeKind::Decides),
        "refutes" => Some(EdgeKind::Refutes),
        "contradicts" => Some(EdgeKind::Contradicts),
        "caused_by" | "causedby" => Some(EdgeKind::CausedBy),
        "motivates" => Some(EdgeKind::Motivates),
        "derived_from" | "derivedfrom" => Some(EdgeKind::DerivedFrom),
        "anchored_to" | "anchoredto" => Some(EdgeKind::AnchoredTo),
        _ => None,
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
            "name": "memory.as_of",
            "description": "Decision-replay: the Derived-plane snapshot of a repo tenant as it stood at a point in time (epoch ms). Shows only nodes that were already valid and not yet retired at that instant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "as_of_ms": { "type": "integer", "description": "epoch milliseconds — the replay instant" },
                    "budget": { "type": "integer", "default": 15 }
                },
                "required": ["repo", "as_of_ms"]
            }
        },
        {
            "name": "memory.verify",
            "description": "Provenance check for one node (by id prefix): signature posture, whether it has been superseded/refuted, and whether a contradiction is recorded — so a caller knows if it is safe to act on.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "id_prefix": { "type": "string" }
                },
                "required": ["repo", "id_prefix"]
            }
        },
        {
            "name": "memory.critique",
            "description": "Self-critic completeness pass: runs a storyline (or a search, if 'query' is given) and reports what an agent acting on the result might be missing — truncated coverage, stale/superseded items, contradictions, adjacent unconfirmed proposals, index staleness.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string" },
                    "query": { "type": "string", "description": "optional — critique a search instead of a storyline" },
                    "budget": { "type": "integer", "default": 15 }
                },
                "required": ["repo"]
            }
        },
        {
            "name": "memory.analogy",
            "description": "Cross-tenant analogues of a node (coarse embedding shortlist + structural edge-shape verify), searched only across tenants this session may read.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "the anchor node's tenant" },
                    "id_prefix": { "type": "string" },
                    "budget": { "type": "integer", "default": 5 }
                },
                "required": ["repo", "id_prefix"]
            }
        },
        {
            "name": "memory.propose_edge",
            "description": "Record a signed QUARANTINED edge proposal (advisory until confirmed; never load-bearing on arrival). Requires write scope on both endpoint tenants.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_prefix": { "type": "string" },
                    "to_prefix": { "type": "string" },
                    "kind": { "type": "string", "enum": ["analogous_to", "supersedes", "references", "depends_on", "implements", "decides", "refutes", "contradicts", "caused_by", "motivates", "derived_from", "anchored_to"], "default": "analogous_to" },
                    "evidence": { "type": "string", "description": "why — human-auditable rationale" }
                },
                "required": ["from_prefix", "to_prefix"]
            }
        },
        {
            "name": "memory.confirm_edge",
            "description": "Promote a quarantined edge proposal to load-bearing. A confirmed supersedes edge applies the status transition (cycle-guarded). Requires write scope on both endpoint tenants.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_prefix": { "type": "string" },
                    "to_prefix": { "type": "string" },
                    "kind": { "type": "string", "default": "analogous_to" }
                },
                "required": ["from_prefix", "to_prefix"]
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
        // Surface non-active lifecycle states (WP-1.4) so a caller never acts on
        // a stale memory without seeing it.
        let status = match i.status {
            mem_core::Status::Active => "",
            mem_core::Status::Superseded => " ⚠SUPERSEDED",
            mem_core::Status::Archived => " (archived)",
        };
        s.push_str(&format!("  {} {}[{}{}] {}\n", &i.id.to_hex()[..10], score, i.kind.discriminant(), status, title));
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
        // Embedded with Recall::new's default space (hashing, d=256) so search works.
        let embedding = Some(mem_index::HashingEmbedder::new(256).embed(subject).unwrap());
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
            embedding,
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
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 11);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for t in ["memory.as_of", "memory.verify", "memory.critique"] {
            assert!(names.contains(&t), "{t} must be advertised");
        }
    }

    #[test]
    fn as_of_is_tenant_scoped_and_audited() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        // Authorized tenant, replay instant after the node's valid_from=1.
        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory.as_of","arguments":{"repo":"citrate-chain","as_of_ms":100}}}"#;
        let v: Value = serde_json::from_str(&srv.handle_line(call).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], false);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("ghostdag"));
        assert_eq!(srv.audit().records()[0].event, MemoryEvent::Read);

        // Ungranted tenant is denied.
        let denied = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory.as_of","arguments":{"repo":"citrate-identity","as_of_ms":100}}}"#;
        let v: Value = serde_json::from_str(&srv.handle_line(denied).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn verify_reports_derived_node_trustworthy() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant());
        let id = node("citrate-chain", "ghostdag tip selection").compute_id();
        let prefix = &id.to_hex()[..10];
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"memory.verify","arguments":{{"repo":"citrate-chain","id_prefix":"{prefix}"}}}}}}"#
        );
        let v: Value = serde_json::from_str(&srv.handle_line(&call).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("trustworthy: true"));
        assert!(text.contains("derived"));
    }

    #[test]
    fn verify_will_not_leak_cross_tenant_node() {
        let s = store();
        let mut srv = MemoryMcpServer::new(&s, grant()); // read on citrate-chain only
        // Prefix of a node that lives in the ungranted citrate-identity tenant.
        let id = node("citrate-identity", "siwe login").compute_id();
        let prefix = &id.to_hex()[..10];
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"memory.verify","arguments":{{"repo":"citrate-chain","id_prefix":"{prefix}"}}}}}}"#
        );
        let v: Value = serde_json::from_str(&srv.handle_line(&call).unwrap()).unwrap();
        // Authorized on citrate-chain, but the resolved node is in another tenant →
        // reads as "no unique node" (existence not leaked), never the node's content.
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("no unique node"), "must not surface a cross-tenant node");
    }

    #[test]
    fn critique_flags_truncated_coverage_over_mcp() {
        let s = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        s.commit(
            &[
                node("citrate-chain", "n1"),
                node("citrate-chain", "n2"),
                node("citrate-chain", "n3"),
            ],
            &[],
        )
        .unwrap();
        let mut srv = MemoryMcpServer::new(&s, grant());
        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory.critique","arguments":{"repo":"citrate-chain","budget":2}}}"#;
        let v: Value = serde_json::from_str(&srv.handle_line(call).unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], false);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("truncated"));
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

    /// SECREM-02 7.5: the audit chain bound via `with_audit_chain` survives a
    /// server "restart" — reopened from disk, it verifies and continues.
    #[test]
    fn persistent_audit_chain_survives_server_restart() {
        let log = {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let mut p = std::env::temp_dir();
            p.push(format!("memmcp-audit-{}-{nanos}.jsonl", std::process::id()));
            p
        };
        let s = store();
        {
            let chain = AuditChain::open(&log).expect("open persistent chain");
            let mut srv = MemoryMcpServer::new(&s, grant())
                .with_audit_chain(Arc::new(Mutex::new(chain)));
            let ok = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-chain"}}}"#;
            let deny = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-identity"}}}"#;
            srv.handle_line(ok).unwrap();
            srv.handle_line(deny).unwrap();
            assert_eq!(srv.audit().len(), 2);
        } // daemon restart
        {
            let chain = AuditChain::open(&log).expect("reopen persistent chain");
            assert_eq!(chain.verify_integrity(), Ok(2), "pre-restart records survive and verify");
            assert_eq!(chain.records()[1].event, MemoryEvent::Denied);
            let mut srv = MemoryMcpServer::new(&s, grant())
                .with_audit_chain(Arc::new(Mutex::new(chain)));
            let again = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory.recall","arguments":{"repo":"citrate-chain"}}}"#;
            srv.handle_line(again).unwrap();
            // The chain CONTINUES from the pre-restart head, not from genesis.
            assert_eq!(srv.audit().records()[2].sequence, 2);
            assert_eq!(srv.audit().verify_integrity(), Ok(3));
        }
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(format!("{}.head", log.display()));
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

    /// Grant readable (+writable) on BOTH fixture tenants.
    fn grant_both(write: bool) -> CapabilityGrant {
        let mut g = grant();
        g.allowed_resources = vec![
            ResourceScope { resource_id: "repo:citrate-chain/memory".into(), can_read: true, can_write: write },
            ResourceScope { resource_id: "repo:citrate-identity/memory".into(), can_read: true, can_write: write },
        ];
        g.sign_with(&SigningKey::from_bytes(&[3u8; 32]));
        g
    }

    fn call_json(srv: &mut MemoryMcpServer, name: &str, args: Value) -> Value {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        let resp = srv.handle_line(&req.to_string()).unwrap();
        serde_json::from_str::<Value>(&resp).unwrap()["result"].clone()
    }

    fn text_of(result: &Value) -> String {
        result["content"][0]["text"].as_str().unwrap().to_string()
    }

    /// WP-4.2 (F-6/R3): a cross-tenant neighbour is shown iff the grant reads
    /// that tenant — and silently hidden otherwise.
    #[test]
    fn cross_tenant_neighbors_follow_grant_intersection() {
        let s = store();
        let nodes = s.all_nodes().unwrap();
        let chain = nodes.iter().find(|n| n.repo == "citrate-chain").unwrap().clone();
        let ident = nodes.iter().find(|n| n.repo == "citrate-identity").unwrap().clone();
        // A confirmed cross-DAG analogy edge chain -> identity.
        let edge = mem_core::Edge {
            from: chain.compute_id(),
            to: ident.compute_id(),
            kind: EdgeKind::AnalogousTo,
            plane: mem_core::Plane::Asserted,
            trust_tier: mem_core::TrustTier::InferredAdvisory,
            provenance: mem_core::EdgeProvenance {
                method: EdgeMethod::Analogy,
                asserter: "t".into(),
                at: 1,
                evidence: None,
            },
            confidence: vec![],
            quarantined: false,
            signature: None,
        };
        s.add_edge(&edge).unwrap();
        let prefix = &chain.compute_id().to_hex()[..12];
        let args = json!({"repo": "citrate-chain", "id_prefix": prefix});

        // Chain-only grant: the identity neighbour is silently hidden.
        let mut srv = MemoryMcpServer::new(&s, grant());
        let text = text_of(&call_json(&mut srv, "memory.neighbors", args.clone()));
        assert!(!text.contains("siwe"), "unreadable tenant's neighbour must be hidden");

        // Both-tenant grant: shown, labelled with its tenant.
        let mut srv = MemoryMcpServer::new(&s, grant_both(false));
        let text = text_of(&call_json(&mut srv, "memory.neighbors", args));
        assert!(text.contains("siwe login"), "readable cross-tenant neighbour shown");
        assert!(text.contains("@citrate-identity"), "cross-tenant neighbour labelled");
    }

    /// WP-4.1 over MCP: propose (quarantined, marked) → confirm (load-bearing).
    #[test]
    fn propose_then_confirm_edge_over_mcp() {
        let s = store();
        let nodes = s.all_nodes().unwrap();
        let chain = nodes.iter().find(|n| n.repo == "citrate-chain").unwrap().clone();
        let ident = nodes.iter().find(|n| n.repo == "citrate-identity").unwrap().clone();
        let from_prefix = chain.compute_id().to_hex()[..12].to_string();
        let to_prefix = ident.compute_id().to_hex()[..12].to_string();
        let args = json!({"from_prefix": from_prefix, "to_prefix": to_prefix, "kind": "analogous_to", "evidence": "same shape"});

        // Write on chain only: the cross-tenant proposal is denied.
        let mut g = grant_both(false);
        g.allowed_resources[0].can_write = true;
        g.sign_with(&SigningKey::from_bytes(&[3u8; 32]));
        let mut srv = MemoryMcpServer::new_with_asserter(&s, g, asserter());
        let r = call_json(&mut srv, "memory.propose_edge", args.clone());
        assert_eq!(r["isError"], true, "needs write on BOTH endpoint tenants");
        assert!(s.out_edges(&chain.compute_id()).unwrap().is_empty());

        // Write on both: proposal lands quarantined, then confirm promotes it.
        let mut srv = MemoryMcpServer::new_with_asserter(&s, grant_both(true), asserter());
        let r = call_json(&mut srv, "memory.propose_edge", args.clone());
        assert_eq!(r["isError"], false, "{r}");
        let stored = &s.out_edges(&chain.compute_id()).unwrap()[0];
        assert!(stored.quarantined, "proposal starts quarantined");

        let text = text_of(&call_json(
            &mut srv,
            "memory.neighbors",
            json!({"repo": "citrate-chain", "id_prefix": chain.compute_id().to_hex()[..12]}),
        ));
        assert!(text.contains("(proposed)"), "quarantined edge visibly marked: {text}");

        let r = call_json(&mut srv, "memory.confirm_edge", args);
        assert_eq!(r["isError"], false, "{r}");
        let stored = &s.out_edges(&chain.compute_id()).unwrap()[0];
        assert!(!stored.quarantined, "confirmed edge is load-bearing");
    }

    /// WP-4.3 over MCP: analogues come only from readable tenants.
    #[test]
    fn analogy_respects_grant_intersection() {
        let s = store();
        let nodes = s.all_nodes().unwrap();
        let chain = nodes.iter().find(|n| n.repo == "citrate-chain").unwrap().clone();
        let args = json!({"repo": "citrate-chain", "id_prefix": chain.compute_id().to_hex()[..12]});

        // Chain-only grant: identity is not a candidate tenant.
        let mut srv = MemoryMcpServer::new(&s, grant());
        let text = text_of(&call_json(&mut srv, "memory.analogy", args.clone()));
        assert!(text.contains("no other readable tenants"), "{text}");

        // Both readable: the identity node appears as a candidate.
        let mut srv = MemoryMcpServer::new(&s, grant_both(false));
        let text = text_of(&call_json(&mut srv, "memory.analogy", args));
        assert!(text.contains("@citrate-identity"), "readable tenant searched: {text}");
        assert!(text.contains("cos "), "scores rendered: {text}");
    }

    #[test]
    fn search_through_shared_index_cache() {
        let s = store();
        let cache = Arc::new(TenantIndexCache::new());
        // Two sessions sharing one cache, like daemon connections.
        let mut srv_a = MemoryMcpServer::new(&s, grant()).with_index_cache(Arc::clone(&cache));
        let mut srv_b = MemoryMcpServer::new(&s, grant()).with_index_cache(Arc::clone(&cache));
        let call = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memory.search","arguments":{"repo":"citrate-chain","query":"ghostdag tip","budget":2}}}"#;
        for srv in [&mut srv_a, &mut srv_b] {
            let v: Value = serde_json::from_str(&srv.handle_line(call).unwrap()).unwrap();
            assert_eq!(v["result"]["isError"], false);
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("ghostdag"), "search should surface the chain node");
        }
        assert_eq!(cache.builds(), 1, "one HNSW build serves both sessions");
        assert_eq!(cache.hits(), 1, "second session reused the index");
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
