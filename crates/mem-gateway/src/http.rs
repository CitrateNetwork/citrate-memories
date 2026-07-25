//! The axum HTTP surface. Every Org route is OIDC-gated and fails closed; the
//! webapp BFF reaches this server-to-server (no CORS — never browser→gateway).
//!
//! Routes:
//!   GET  /api/health                     — liveness, unauthenticated
//!   GET  /api/orgs/:org/layout           — constellation scene (PCA-2D)
//!   GET  /api/orgs/:org/recall?repo&budget
//!   GET  /api/orgs/:org/search?repo&q&budget
//!   GET  /api/orgs/:org/neighbors?repo&id&budget
//!   GET  /api/orgs/:org/verify?repo&id   — is this memory current / superseded / contradicted?
//!   GET  /api/orgs/:org/review?repo&budget — advisory / non-Active items needing review
//!   POST /api/orgs/:org/assert           — write an Asserted node (gated, audited)
//!   GET  /ops                            — operational status (Org-wide read)
//!   POST /mcp/u/:sub                      — BYOM MCP-over-HTTP (HS256 connect token)
//!
//! Handlers do their RocksDB work synchronously and contain no `.await`, so the
//! per-request `Recall`/`MemoryMcpServer` borrows of the store never cross a
//! suspension point (keeps the handler futures `Send`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::collections::VecDeque;
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use serde_json::{json, Value};

use mem_assert::Asserter;
use mem_authz::{AuditChain, MemoryEvent, Op};
use mem_core::{MemoryNode, NodeKind};
use mem_index::Embedder;
use mem_mcp::MemoryMcpServer;
use mem_query::{NeighborItem, Recall, RecallItem, RecallResult, TenantIndexCache};
use mem_store::MemoryDagStore;

use crate::auth::{mint_grant, repo_resource, verify_connect_token, OidcVerifier};
use crate::control::{Control, OrgStatus};
use crate::now_ms;
use crate::scene::{self, NodeInput, Scene};

/// In-process grants are short-lived; the principal is re-verified every request.
const GRANT_TTL_MS: u64 = 3_600_000;
/// Cap on nodes pulled into the layout scene when there's no `federation` meta tenant.
const LAYOUT_CAP: usize = 1500;

/// Shared, cheaply-cloneable server state.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<MemoryDagStore<MemoryNode>>,
    pub embedder: Option<Arc<dyn Embedder>>,
    pub index_cache: Arc<TenantIndexCache>,
    pub write_gate: Arc<Mutex<()>>,
    pub audit: Arc<Mutex<AuditChain>>,
    /// Gateway key: signs minted grants and is the write asserter (stable seed).
    pub signing_key: SigningKey,
    pub control: Arc<RwLock<Control>>,
    pub org_id: Arc<String>,
    pub store_path: Arc<String>,
    /// Label placed in minted grants' `issuer` field (informational).
    pub issuer: Arc<String>,
    pub oidc: Option<Arc<OidcVerifier>>,
    pub connect_secret: Option<Arc<String>>,
    pub allow_dev_auth: bool,
    pub layout_cache: Arc<Mutex<Option<Scene>>>,
    /// MEM-S7 WP-7.2: verified, in-scope push events awaiting incremental ingest.
    /// The receiver only enqueues; the single-writer worker (WP-7.3) drains it.
    pub ingest_queue: Arc<Mutex<VecDeque<crate::webhook::PushEvent>>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/connector.py", get(connector_script))
        .route("/api/orgs/:org/layout", get(layout))
        .route("/api/orgs/:org/recall", get(recall))
        .route("/api/orgs/:org/search", get(search))
        .route("/api/orgs/:org/neighbors", get(neighbors))
        .route("/api/orgs/:org/verify", get(verify))
        .route("/api/orgs/:org/review", get(review))
        .route("/api/orgs/:org/assert", post(assert))
        .route("/ops", get(ops))
        .route("/mcp/u/:sub", post(byom))
        .route("/webhook/github", post(github_webhook))
        .with_state(state)
}

/// Bind and serve until the process is killed.
pub async fn run(state: AppState, bind: &str) -> std::io::Result<()> {
    // MEM-S7 WP-7.3: drain webhook-enqueued ingest jobs in the background.
    crate::ingest_worker::spawn(state.clone());
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("mem-gateway listening on http://{bind}");
    axum::serve(listener, app).await
}

// --------------------------------------------------------------------------
// Error type
// --------------------------------------------------------------------------

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiError {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

fn unauthorized(m: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, m)
}
fn forbidden(m: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, m)
}
fn not_found(m: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, m)
}
fn bad(m: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, m)
}
fn ise<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("internal: {e}"))
}

// --------------------------------------------------------------------------
// Lock helpers (recover from poisoning instead of panicking — Rule 8)
// --------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
fn rlock<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|p| p.into_inner())
}

// --------------------------------------------------------------------------
// Auth + authorization
// --------------------------------------------------------------------------

fn bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .map(|s| s.trim().to_string())
}

/// Establish the principal `sub`. OIDC takes precedence and fails closed; the
/// `x-dev-sub` backdoor is only honored when OIDC is off and dev-auth is on.
fn authenticate(app: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    if let Some(oidc) = &app.oidc {
        let token = bearer(headers).ok_or_else(|| unauthorized("missing bearer token"))?;
        return oidc
            .verify(&token)
            .map_err(|_| unauthorized("token rejected"));
    }
    if app.allow_dev_auth {
        return headers
            .get("x-dev-sub")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| unauthorized("missing x-dev-sub (dev-auth)"));
    }
    Err(unauthorized("authentication not configured"))
}

fn audit_event(app: &AppState, ev: MemoryEvent, actor: &str, resource: &str, detail: &str) {
    let mut chain = lock(&app.audit);
    if let Err(e) = chain.append(ev, actor, resource, detail, now_ms()) {
        tracing::error!("audit append failed for {actor} on {resource}: {e}");
    }
}

/// Authenticate, then authorize one operation against the principal's Org
/// membership, minting an in-process [`mem_authz::CapabilityGrant`] and running
/// it through `grant.check()` — the same primitive the MCP surface uses. Audits
/// the allow/deny decision. Returns the principal `sub` on success.
fn gate(
    app: &AppState,
    headers: &HeaderMap,
    org: &str,
    resource: &str,
    op: Op,
    detail: &str,
) -> Result<String, ApiError> {
    // Single-Org runner: this gateway serves exactly one store.
    if org != app.org_id.as_str() {
        return Err(not_found("unknown org on this gateway"));
    }
    let sub = authenticate(app, headers)?;

    let membership = {
        let control = rlock(&app.control);
        match control.org(org) {
            Some(o) if o.status == OrgStatus::Active => {}
            Some(_) => return Err(forbidden("org suspended")),
            None => return Err(not_found("unknown org")),
        }
        match control.membership(&sub, org) {
            Some(m) => m.clone(),
            None => {
                drop(control);
                audit_event(app, MemoryEvent::Denied, &sub, resource, "no membership");
                return Err(forbidden("no membership in org"));
            }
        }
    };

    let now = now_ms();
    let grant = mint_grant(&app.signing_key, &app.issuer, &membership, now, GRANT_TTL_MS);
    match grant.check(resource, op, now) {
        Ok(()) => {
            let ev = match op {
                Op::Read => MemoryEvent::Read,
                Op::Write => MemoryEvent::Write,
            };
            audit_event(app, ev, &sub, resource, detail);
            Ok(sub)
        }
        Err(e) => {
            audit_event(app, MemoryEvent::Denied, &sub, resource, &e.to_string());
            Err(forbidden("not authorized for resource"))
        }
    }
}

// --------------------------------------------------------------------------
// Read-path helpers
// --------------------------------------------------------------------------

fn recaller(app: &AppState) -> Recall<'_> {
    let recall = match &app.embedder {
        Some(e) => Recall::with_embedder(&app.store, Box::new(e.clone())),
        None => Recall::new(&app.store),
    };
    recall.with_index_cache(app.index_cache.clone())
}

fn qparam(q: &HashMap<String, String>, key: &str) -> Option<String> {
    q.get(key).map(|s| s.to_string()).filter(|s| !s.is_empty())
}

fn budget(q: &HashMap<String, String>, default: usize) -> usize {
    q.get("budget")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0 && *n <= 500)
        .unwrap_or(default)
}

fn item_json(it: &RecallItem) -> Value {
    json!({
        "id": it.id.to_hex(),
        "kind": it.kind.discriminant(),
        "repo": it.repo,
        "title": it.title,
        "valid_from": it.valid_from,
        "plane": format!("{:?}", it.plane),
        "trust_tier": format!("{:?}", it.trust_tier),
        "status": format!("{:?}", it.status),
        "score": it.score,
        // ADR-09 B.4: null for canonical (merged) items; the branch name for
        // work-in-progress surfaced via include_in_flight.
        "in_flight_branch": it.in_flight_branch,
    })
}

fn result_json(r: &RecallResult) -> Value {
    json!({
        "repo": r.repo,
        "watermark": r.watermark.as_ref().map(|w| format!("{w:?}")),
        "total_in_tenant": r.total_in_tenant,
        "count": r.items.len(),
        "items": r.items.iter().map(item_json).collect::<Vec<_>>(),
    })
}

fn neighbor_json(n: &NeighborItem) -> Value {
    json!({
        "edge_kind": format!("{:?}", n.edge_kind),
        "direction": format!("{:?}", n.direction),
        "quarantined": n.quarantined,
        "node": n.node.as_ref().map(item_json),
    })
}

fn parse_kind(s: &str) -> NodeKind {
    match s {
        "finding" => NodeKind::Finding,
        "doc" => NodeKind::Doc,
        "blocker" => NodeKind::Blocker,
        "techdebt" | "tech_debt" => NodeKind::TechDebt,
        "workpackage" | "work_package" => NodeKind::WorkPackage,
        "adr" => NodeKind::Adr,
        "handoff" => NodeKind::Handoff,
        "benchmark" => NodeKind::Benchmark,
        "agentaction" | "agent_action" => NodeKind::AgentAction,
        _ => NodeKind::Rationale,
    }
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "mem-gateway" }))
}

/// Serve the BYOM stdio<->HTTP connector shim so a teammate can install it with a
/// single `curl` (see `docs/TEAM_INSTALL_GUIDE.md`, Option B). Embedded at compile
/// time so it can never drift from the shipped `scripts/mcp-connector.py`.
///
/// Public + unauthenticated by design: the script holds NO secrets (the connect
/// token is supplied by the user at runtime via env), and it must be fetchable
/// before the user has configured anything.
async fn connector_script() -> Response {
    const SCRIPT: &str = include_str!("../../../scripts/mcp-connector.py");
    (
        [
            (header::CONTENT_TYPE, "text/x-python; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"mcp-connector.py\"",
            ),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        SCRIPT,
    )
        .into_response()
}

/// MEM-S7 WP-7.2 — GitHub push webhook. Authenticity FIRST (HMAC over the raw
/// body, fail-closed when `MEM_INGEST_WEBHOOK_SECRET` is unset), then org
/// allowlist + parse, then enqueue an incremental-ingest job. The single-writer
/// worker (WP-7.3) drains the queue; this handler never writes the store.
async fn github_webhook(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let secret = std::env::var("MEM_INGEST_WEBHOOK_SECRET").unwrap_or_default();
    if secret.is_empty() {
        // Fail closed: an unconfigured secret must never accept an unauthenticated body.
        return ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ingest webhook disabled (MEM_INGEST_WEBHOOK_SECRET unset)",
        )
        .into_response();
    }
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if !crate::webhook::verify_signature(secret.as_bytes(), sig, &body) {
        return unauthorized("invalid or missing webhook signature").into_response();
    }
    // GitHub's connectivity check.
    if headers.get("x-github-event").and_then(|v| v.to_str().ok()) == Some("ping") {
        return (StatusCode::OK, Json(json!({ "pong": true }))).into_response();
    }
    match crate::webhook::parse_push_event(&body) {
        Some(ev) => {
            let depth = {
                let mut q = lock(&app.ingest_queue);
                q.push_back(ev.clone());
                q.len()
            };
            (
                StatusCode::ACCEPTED,
                Json(json!({ "queued": ev.repo, "ref": ev.git_ref, "queue_depth": depth })),
            )
                .into_response()
        }
        None => bad("not an in-scope CitrateNetwork push event").into_response(),
    }
}

async fn layout(
    State(app): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    gate(&app, &headers, &org, "*", Op::Read, "layout")?;
    let scene = build_layout(&app, &org)?;
    Ok(Json(serde_json::to_value(scene).map_err(ise)?))
}

async fn recall(
    State(app): State<AppState>,
    Path(org): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let repo = qparam(&q, "repo").ok_or_else(|| bad("repo query param required"))?;
    gate(&app, &headers, &org, &repo_resource(&repo), Op::Read, "recall")?;
    let r = recaller(&app)
        .with_in_flight(in_flight_param(&q))
        .storyline(&repo, budget(&q, 15))
        .map_err(ise)?;
    Ok(Json(result_json(&r)))
}

/// ADR-09 B.4: opt in to the in-flight branch layer via `?include_in_flight=true`.
fn in_flight_param(q: &HashMap<String, String>) -> bool {
    q.get("include_in_flight").map(|v| v == "true" || v == "1").unwrap_or(false)
}

async fn search(
    State(app): State<AppState>,
    Path(org): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let repo = qparam(&q, "repo").ok_or_else(|| bad("repo query param required"))?;
    let query = qparam(&q, "q").ok_or_else(|| bad("q query param required"))?;
    gate(&app, &headers, &org, &repo_resource(&repo), Op::Read, "search")?;
    let r = recaller(&app)
        .with_in_flight(in_flight_param(&q))
        .search(&repo, &query, budget(&q, 10))
        .map_err(ise)?;
    Ok(Json(result_json(&r)))
}

async fn neighbors(
    State(app): State<AppState>,
    Path(org): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let repo = qparam(&q, "repo").ok_or_else(|| bad("repo query param required"))?;
    let id_prefix = qparam(&q, "id").ok_or_else(|| bad("id query param required"))?;
    gate(&app, &headers, &org, &repo_resource(&repo), Op::Read, "neighbors")?;
    let rc = recaller(&app);
    let id = rc
        .resolve_prefix(&id_prefix)
        .map_err(ise)?
        .ok_or_else(|| not_found("node id prefix did not resolve"))?;
    let ns = rc.neighbors(&id, budget(&q, 20)).map_err(ise)?;
    Ok(Json(json!({
        "id": id.to_hex(),
        "count": ns.len(),
        "neighbors": ns.iter().map(neighbor_json).collect::<Vec<_>>(),
    })))
}

async fn verify(
    State(app): State<AppState>,
    Path(org): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let repo = qparam(&q, "repo").ok_or_else(|| bad("repo query param required"))?;
    let id_prefix = qparam(&q, "id").ok_or_else(|| bad("id query param required"))?;
    gate(&app, &headers, &org, &repo_resource(&repo), Op::Read, "verify")?;
    let rc = recaller(&app);
    let id = rc
        .resolve_prefix(&id_prefix)
        .map_err(ise)?
        .ok_or_else(|| not_found("node id prefix did not resolve"))?;
    let node = app
        .store
        .get_node(&id)
        .map_err(ise)?
        .ok_or_else(|| not_found("node not found"))?;
    let ns = rc.neighbors(&id, 64).map_err(ise)?;
    let superseded = matches!(node.status, mem_core::Status::Superseded)
        || ns.iter().any(|n| {
            matches!(n.edge_kind, mem_core::EdgeKind::Supersedes)
                && matches!(n.direction, mem_query::Direction::In)
                && !n.quarantined
        });
    let contradicted = ns.iter().any(|n| {
        matches!(
            n.edge_kind,
            mem_core::EdgeKind::Contradicts | mem_core::EdgeKind::Refutes
        ) && !n.quarantined
    });
    let verdict = if superseded {
        "superseded"
    } else if contradicted {
        "contradicted"
    } else {
        "current"
    };
    let item = RecallItem {
        id,
        kind: node.kind.clone(),
        repo: node.repo.clone(),
        title: String::from_utf8_lossy(&node.content).to_string(),
        valid_from: node.valid_from,
        plane: node.plane,
        trust_tier: node.trust_tier,
        source: node.source_ref.clone(),
        status: node.status,
        score: None,
        in_flight_branch: None,
    };
    Ok(Json(json!({
        "id": item.id.to_hex(),
        "verdict": verdict,
        "superseded": superseded,
        "contradicted": contradicted,
        "node": item_json(&item),
        "neighbors": ns.iter().map(neighbor_json).collect::<Vec<_>>(),
    })))
}

async fn review(
    State(app): State<AppState>,
    Path(org): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let repo = qparam(&q, "repo").ok_or_else(|| bad("repo query param required"))?;
    gate(&app, &headers, &org, &repo_resource(&repo), Op::Read, "review")?;
    let want = budget(&q, 25);
    // Pull a wider window, then keep the items a human should review: advisory
    // (inferred) trust, or anything no longer Active.
    let r = recaller(&app)
        .storyline(&repo, want.saturating_mul(4).min(500))
        .map_err(ise)?;
    let items: Vec<Value> = r
        .items
        .iter()
        .filter(|it| {
            matches!(it.trust_tier, mem_core::TrustTier::InferredAdvisory)
                || !matches!(it.status, mem_core::Status::Active)
        })
        .take(want)
        .map(item_json)
        .collect();
    Ok(Json(json!({
        "repo": r.repo,
        "total_in_tenant": r.total_in_tenant,
        "count": items.len(),
        "items": items,
    })))
}

#[derive(Deserialize)]
struct AssertBody {
    repo: String,
    content: String,
    #[serde(default)]
    kind: Option<String>,
    /// Real-world date this became true (ms since epoch). Defaults to now.
    /// Recorded separately from the write time, which stays in `observed_at`.
    #[serde(default)]
    valid_from: Option<u64>,
}

async fn assert(
    State(app): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AssertBody>,
) -> Result<Json<Value>, ApiError> {
    if body.repo.is_empty() {
        return Err(bad("repo required"));
    }
    if body.content.is_empty() {
        return Err(bad("content required"));
    }
    let sub = gate(
        &app,
        &headers,
        &org,
        &repo_resource(&body.repo),
        Op::Write,
        "assert",
    )?;
    let kind = parse_kind(body.kind.as_deref().unwrap_or("rationale"));
    // FWA-C10-04: sign under a per-principal sub-identity so `blame()` names the
    // authenticated actor, not the shared gateway key.
    let asserter = Asserter::for_principal(&app.signing_key, &sub);
    // Embed at write time in the store's own space, or the node is invisible to
    // every semantic query: `Recall::search` indexes only nodes whose `embedding`
    // is `Some`, and the model guard skips vectors from a mismatched space.
    let (embedding, embed_warning) = match &app.embedder {
        Some(e) => match e.embed(&body.content) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e.to_string())),
        },
        None => (None, Some("gateway has no embedder configured".to_string())),
    };
    let now = now_ms();
    let id = {
        let _gate = lock(&app.write_gate); // serialize writes (single-writer store)
        let node = asserter.assert_node_with(
            &body.repo,
            kind,
            &body.content,
            body.valid_from.unwrap_or(now),
            now,
            embedding,
        );
        app.store.put_node(&node).map_err(ise)?
    };
    // The graph changed; drop the cached constellation so it rebuilds on demand.
    *lock(&app.layout_cache) = None;
    // Never let a failed embed be silent: the write is durable either way, but an
    // unembedded node will not answer memory.search until it is backfilled.
    Ok(Json(json!({
        "id": id.to_hex(),
        "repo": body.repo,
        "author": asserter.pubkey_hex(),
        "embedded": embed_warning.is_none(),
        "warning": embed_warning,
    })))
}

async fn ops(State(app): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let org = app.org_id.as_str().to_string();
    gate(&app, &headers, &org, "*", Op::Read, "ops")?;
    let mut obj = serde_json::Map::new();
    obj.insert("org".into(), json!(org));
    obj.insert("store_path".into(), json!(app.store_path.as_str()));
    obj.insert(
        "encrypted_at_rest".into(),
        json!(app.store.is_encrypted_at_rest()),
    );
    obj.insert("node_count".into(), json!(app.store.node_count().map_err(ise)?));
    obj.insert("edge_count".into(), json!(app.store.edge_count().map_err(ise)?));
    obj.insert("audit_records".into(), json!(lock(&app.audit).len()));
    obj.insert(
        "embedder".into(),
        json!(app.embedder.as_ref().map(|e| e.model_id().to_string())),
    );
    obj.insert("oidc".into(), json!(app.oidc.is_some()));
    obj.insert("dev_auth".into(), json!(app.allow_dev_auth));
    #[cfg(feature = "chain")]
    obj.insert(
        "chain".into(),
        json!({ "enabled": true, "note": "on-chain anchor lookup not wired in this build" }),
    );
    Ok(Json(Value::Object(obj)))
}

/// BYOM (bring-your-own-model) MCP-over-HTTP: a connect-token-authenticated
/// client speaks JSON-RPC to the same `MemoryMcpServer` the stdio/daemon
/// transports use.
async fn byom(
    State(app): State<AppState>,
    Path(sub): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Result<Response, ApiError> {
    let secret = app
        .connect_secret
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_IMPLEMENTED, "BYOM connect not configured"))?;
    let token = bearer(&headers).ok_or_else(|| unauthorized("missing connect token"))?;
    let token_sub =
        verify_connect_token(secret, &token).map_err(|_| unauthorized("connect token rejected"))?;
    if token_sub != sub {
        return Err(forbidden("connect token sub mismatch"));
    }

    let membership = {
        let control = rlock(&app.control);
        match control.membership(&sub, app.org_id.as_str()) {
            Some(m) => m.clone(),
            None => return Err(forbidden("no membership in org")),
        }
    };
    let now = now_ms();
    let grant = mint_grant(&app.signing_key, &app.issuer, &membership, now, GRANT_TTL_MS);
    // FWA-C10-04: per-principal authorship (see /assert). `sub` is the
    // connect-token-verified principal.
    let asserter = Asserter::for_principal(&app.signing_key, &sub);
    let mut server = MemoryMcpServer::new_with_asserter(&app.store, grant, asserter)
        .with_write_gate(app.write_gate.clone())
        .with_index_cache(app.index_cache.clone())
        .with_audit_chain(app.audit.clone());
    if let Some(e) = &app.embedder {
        server = server.with_query_embedder(e.clone());
    }

    let mut out = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_line(line) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&resp);
        }
    }
    Ok(([(header::CONTENT_TYPE, "application/json")], out).into_response())
}

// --------------------------------------------------------------------------
// Layout scene builder
// --------------------------------------------------------------------------

fn build_layout(app: &AppState, org: &str) -> Result<Scene, ApiError> {
    if let Some(scene) = lock(&app.layout_cache).as_ref() {
        return Ok(scene.clone());
    }
    let all = app.store.all_nodes().map_err(ise)?;
    // Prefer the reserved `federation` meta tenant (one Tenant node per repo +
    // DependsOn edges = the org overview). Fall back to a capped sample so the
    // scene is never empty on a freshly-backfilled store without a meta graph.
    let fed: Vec<&MemoryNode> = all.iter().filter(|n| n.repo == "federation").collect();
    let chosen: Vec<&MemoryNode> = if fed.len() >= 2 {
        fed
    } else {
        all.iter().take(LAYOUT_CAP).collect()
    };

    let id_of = |n: &MemoryNode| n.compute_id().to_hex();
    let id_set: HashSet<String> = chosen.iter().map(|n| id_of(n)).collect();

    let inputs: Vec<NodeInput> = chosen
        .iter()
        .map(|n| NodeInput {
            id: id_of(n),
            title: String::from_utf8_lossy(&n.content).to_string(),
            kind: n.kind.discriminant(),
            repo: n.repo.clone(),
            // lowercase to match the renderer's "derived"/"asserted" comparison.
            plane: format!("{:?}", n.plane).to_lowercase(),
            // trust-tier id, verbatim enum name (the renderer keys lanes on it).
            trust: format!("{:?}", n.trust_tier),
            status: format!("{:?}", n.status),
            embedding: n.embedding.as_ref().map(|v| v.data.clone()),
        })
        .collect();

    let mut edges: Vec<(String, String, String, bool)> = Vec::new();
    for n in &chosen {
        let from = id_of(n);
        for e in app.store.out_edges(&n.compute_id()).map_err(ise)? {
            let to = e.to.to_hex();
            if id_set.contains(&to) {
                edges.push((from.clone(), to, format!("{:?}", e.kind), e.quarantined));
            }
        }
    }

    let scene = scene::build_scene(org, inputs, edges);
    *lock(&app.layout_cache) = Some(scene.clone());
    Ok(scene)
}
