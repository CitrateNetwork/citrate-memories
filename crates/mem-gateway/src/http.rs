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
use ed25519_dalek::{Signer, SigningKey};
use serde::Deserialize;
use serde_json::{json, Value};

use mem_assert::Asserter;
use mem_authz::{AuditChain, CapabilityGrant, MemoryEvent, Op, PolicyProfile, ResourceScope};
use mem_core::{MemoryNode, NodeKind};
use mem_index::Embedder;
use mem_mcp::MemoryMcpServer;
use mem_query::{NeighborItem, Recall, RecallItem, RecallResult, TenantIndexCache};
use mem_store::MemoryDagStore;

use crate::auth::{
    mint_connect_token, mint_grant, repo_resource, verify_connect_token, ConnectClaims,
    OidcVerifier,
};
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
    /// citrate-chain (40204) JSON-RPC URL for binding external audit checkpoints to
    /// the live chain head (WP-5.3, read-only). `None` disables chain binding — the
    /// checkpoint routes then return 503 rather than a chain-less checkpoint.
    pub chain_rpc: Option<Arc<String>>,
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
        .route("/connect/token", post(connect_token))
        .route("/api/orgs/:org/layout", get(layout))
        .route("/api/orgs/:org/recall", get(recall))
        .route("/api/orgs/:org/search", get(search))
        .route("/api/orgs/:org/neighbors", get(neighbors))
        .route("/api/orgs/:org/verify", get(verify))
        .route("/api/orgs/:org/review", get(review))
        .route("/api/orgs/:org/assert", post(assert))
        .route("/api/orgs/:org/health", get(org_health))
        .route(
            "/api/orgs/:org/checkpoint",
            post(submit_checkpoint).get(read_checkpoint),
        )
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

/// Append one audited decision, failing CLOSED: MEM-B-011 — if the append fails
/// (disk full, read-only audit path, poisoned chain) the caller must NOT serve
/// the request. Mirrors `mem-mcp::authorize`, which already 500s on an audit
/// error rather than performing an unaudited read/write.
fn audit_event(
    app: &AppState,
    ev: MemoryEvent,
    actor: &str,
    resource: &str,
    detail: &str,
) -> Result<(), ApiError> {
    let mut chain = lock(&app.audit);
    chain.append(ev, actor, resource, detail, now_ms()).map(|_| ()).map_err(|e| {
        tracing::error!("audit append failed for {actor} on {resource}: {e}");
        ise(format!("audit append failed; refusing to proceed: {e}"))
    })
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
    gate_with_grant(app, headers, org, resource, op, detail).map(|(sub, _)| sub)
}

/// Like [`gate`], but also returns the caller's full minted [`CapabilityGrant`],
/// so a handler can apply per-item grant intersection (e.g. filter cross-tenant
/// neighbours to the tenants this caller may actually read — MEM-B-007). The grant
/// is the SAME one `gate` authorizes `resource` against; the extra return lets a
/// read handler police the *other* nodes it surfaces, not just the anchor.
fn gate_with_grant(
    app: &AppState,
    headers: &HeaderMap,
    org: &str,
    resource: &str,
    op: Op,
    detail: &str,
) -> Result<(String, CapabilityGrant), ApiError> {
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
                // Best-effort on the deny path: the outcome is already a refusal.
                let _ = audit_event(app, MemoryEvent::Denied, &sub, resource, "no membership");
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
            // Fail closed: an allowed op that cannot be audited must not run.
            audit_event(app, ev, &sub, resource, detail)?;
            Ok((sub, grant))
        }
        Err(e) => {
            let _ = audit_event(app, MemoryEvent::Denied, &sub, resource, &e.to_string());
            Err(forbidden("not authorized for resource"))
        }
    }
}

/// FUA-MEMORIES-01 / MEM-B-007: resolve an id prefix and confirm the node lives in
/// `repo`. `Recall::resolve_prefix` is tenant-blind — it matches across ALL
/// tenants — so a caller authorized on `repo` could pass a prefix of a node in a
/// tenant their membership excludes and read its content/neighbours (a
/// cross-tenant IDOR). A prefix that does not resolve, OR resolves into another
/// tenant, reads as 404 (never 403) so existence does not leak. This mirrors the
/// guard already applied on the MCP surface (`mem-mcp` `call_neighbors`/
/// `call_verify`); it is lifted here so both surfaces share one rule.
fn resolve_in_tenant(
    store: &MemoryDagStore<MemoryNode>,
    rc: &Recall<'_>,
    repo: &str,
    prefix: &str,
) -> Result<(mem_core::ContentHash, MemoryNode), ApiError> {
    // PBA-L6b-017: resolve among THIS tenant's nodes only, so a foreign node
    // sharing the prefix can neither match nor make it ambiguous (a 404-vs-200
    // existence oracle). The repo re-check below stays as defence in depth.
    let id = rc
        .resolve_prefix_in(repo, prefix)
        .map_err(ise)?
        .ok_or_else(|| not_found("node id prefix did not resolve"))?;
    match store.get_node(&id).map_err(ise)? {
        Some(n) if n.repo == repo => Ok((id, n)),
        _ => Err(not_found("node id prefix did not resolve")),
    }
}

/// Grant-intersection read check (R3): may this caller's grant READ `repo`? Used
/// to filter individual cross-tenant items (e.g. neighbours reached via a
/// cross-DAG `AnalogousTo` edge) so neither their content nor existence leaks to a
/// caller whose membership excludes that tenant (MEM-B-007).
fn grant_can_read(grant: &CapabilityGrant, repo: &str) -> bool {
    grant.check(&repo_resource(repo), Op::Read, now_ms()).is_ok()
}

/// PBA-L6b-001: the ONE way an HTTP handler fetches neighbours for a caller —
/// the anchor's own tenant plus any tenant the caller's grant can READ. Shared by
/// `/neighbors` and `/verify` (and mirrored by MCP `memory.neighbors`) so the
/// grant intersection cannot be forgotten on one surface again.
fn readable_neighbors(
    rc: &Recall<'_>,
    grant: &CapabilityGrant,
    repo: &str,
    id: &mem_core::ContentHash,
    budget: usize,
) -> Result<Vec<NeighborItem>, ApiError> {
    rc.neighbors_readable(id, budget, |t| t == repo || grant_can_read(grant, t))
        .map_err(ise)
}

/// MEM-B-008: intersect a membership grant with the attenuation a BYOM connect
/// token declares. `verify_connect_token` used to read ONLY `sub`, so the token's
/// `scope`/`tenants` were silently discarded and `byom` minted the principal's
/// FULL membership grant — an Owner/Admin `*` read+write for the whole TTL, though
/// the token was presented to the user as read-only on a named tenant list. The
/// effective authority is now the INTERSECTION of membership and token: never
/// wider than either.
///   * `tenants` present → restrict resources to exactly those repos the token
///     names AND the membership already permits (drops a `*` wildcard).
///   * write is allowed only when the token's `scope` names a write-y capability
///     (`write` or `propose`); an absent scope defaults to read-only (least
///     privilege) so a legacy claimless token cannot silently escalate.
///
/// The narrowed grant is re-signed with the gateway key so it still verifies.
fn attenuate_grant(grant: &CapabilityGrant, claims: &ConnectClaims, sk: &SigningKey) -> CapabilityGrant {
    let token_allows_write = claims
        .scope
        .as_deref()
        .map(|s| s.split(',').any(|t| matches!(t.trim(), "write" | "propose")))
        .unwrap_or(false);
    let now = now_ms();

    let mut narrowed = grant.clone();
    match &claims.tenants {
        Some(tenants) => {
            narrowed.allowed_resources = tenants
                .iter()
                .map(|t| {
                    let rid = repo_resource(t);
                    let can_read = grant.check(&rid, Op::Read, now).is_ok();
                    let can_write = token_allows_write && grant.check(&rid, Op::Write, now).is_ok();
                    ResourceScope { resource_id: rid, can_read, can_write }
                })
                .collect();
        }
        None => {
            // No tenant restriction — keep the membership scopes but gate write.
            for r in narrowed.allowed_resources.iter_mut() {
                r.can_write = r.can_write && token_allows_write;
            }
        }
    }
    if !token_allows_write {
        narrowed.policy = PolicyProfile::ReadOnly;
    }
    narrowed.sign_with(sk);
    narrowed
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
    let (_sub, grant) =
        gate_with_grant(&app, &headers, &org, &repo_resource(&repo), Op::Read, "neighbors")?;
    let rc = recaller(&app);
    // MEM-B-007: the anchor must live in the authorized tenant (cross-tenant → 404).
    let (id, _node) = resolve_in_tenant(&app.store, &rc, &repo, &id_prefix)?;
    // MEM-B-007 / PBA-L6b-001 (grant intersection, R3): a neighbour in another
    // tenant is shown only if this caller's grant can READ that tenant; others are
    // dropped (before the budget) so neither content nor existence leaks.
    let ns = readable_neighbors(&rc, &grant, &repo, &id, budget(&q, 20))?;
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
    // PBA-L6b-001: hold the caller's grant (not just a yes/no) so the neighbours
    // this route serializes — and the verdict derived from them — are filtered to
    // the tenants the caller may read, exactly as `/neighbors` does. `gate()` here
    // left MEM-B-007 half-fixed: /verify returned cross-tenant neighbour content.
    let (_sub, grant) =
        gate_with_grant(&app, &headers, &org, &repo_resource(&repo), Op::Read, "verify")?;
    let rc = recaller(&app);
    // MEM-B-007: resolve within the authorized tenant — a prefix of a node in
    // another tenant reads as 404 (was a cross-tenant IDOR returning its content).
    let (id, node) = resolve_in_tenant(&app.store, &rc, &repo, &id_prefix)?;
    let ns = readable_neighbors(&rc, &grant, &repo, &id, 64)?;
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

/// citrate-chain id the external checkpoints bind against.
const CHAIN_ID_40204: u64 = 40204;

/// `GET /api/orgs/:org/health` — authenticated per-org liveness (200 for a member
/// of the org). Distinct from the unauthenticated `/api/health` global liveness.
async fn org_health(
    State(app): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    gate(&app, &headers, &org, "*", Op::Read, "health")?;
    Ok(Json(json!({ "ok": true, "org": org })))
}

#[derive(Deserialize)]
struct CheckpointBody {
    /// Tenant/repo the checkpoint is recorded under (authz scope).
    repo: String,
    /// The EXTERNAL audit-chain root to checkpoint (a hash / merkle head).
    root: String,
}

#[derive(Deserialize)]
struct CheckpointQuery {
    repo: String,
}

/// Wrap a checkpoint record with a gateway ed25519 signature. ed25519 is
/// deterministic (RFC 8032), so re-signing the same record on read reproduces the
/// same signature — the signature need not be persisted separately.
///
/// The response is SELF-VERIFYING: `signed_payload_hex` is the exact byte string
/// the signature covers, so an offline verifier does
/// `ed25519_verify(hex(signature), hex(signed_payload_hex), hex(gateway_pubkey))`
/// with NO JSON re-serialization (avoiding cross-language canonicalization drift).
/// `signed_payload_hex` decodes to the UTF-8 compact JSON of `record`, so a caller
/// can independently confirm it matches the `record` object it was handed.
fn sign_checkpoint(
    key: &SigningKey,
    record: &mem_sync::chain::ChainAnchorRecord,
) -> Result<Value, ApiError> {
    let bytes = serde_json::to_vec(record).map_err(ise)?;
    let sig = key.sign(&bytes);
    Ok(json!({
        "record": record,
        "signature": hex::encode(sig.to_bytes()),
        "gateway_pubkey": hex::encode(key.verifying_key().to_bytes()),
        "alg": "ed25519",
        "signed_over": "compact-json(record) — the exact bytes are signed_payload_hex",
        "signed_payload_hex": hex::encode(&bytes),
    }))
}

fn map_sync_err(e: mem_sync::SyncError) -> ApiError {
    match e {
        // RPC unreachable / malformed / wrong-chain — an upstream failure.
        mem_sync::SyncError::Chain(m) => {
            ApiError::new(StatusCode::BAD_GATEWAY, format!("chain checkpoint failed: {m}"))
        }
        other => ise(other),
    }
}

/// `POST /api/orgs/:org/checkpoint` — submit an EXTERNAL audit root and receive a
/// hash-chained, gateway-signed checkpoint bound to the live 40204 head (WP-5.3
/// read-only half). Body: `{ "repo": "<tenant>", "root": "<audit-chain head>" }`.
/// Fail-closed: an unreachable chain RPC records nothing and returns 502.
async fn submit_checkpoint(
    State(app): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CheckpointBody>,
) -> Result<Json<Value>, ApiError> {
    if body.repo.is_empty() {
        return Err(bad("repo required"));
    }
    let root = body.root.trim().to_string();
    if root.is_empty() {
        return Err(bad("root required"));
    }
    if root.len() > 256 {
        return Err(bad("root too long (max 256 chars; submit a hash, not a document)"));
    }
    let _sub = gate(
        &app,
        &headers,
        &org,
        &repo_resource(&body.repo),
        Op::Write,
        "checkpoint",
    )?;
    let rpc = app
        .chain_rpc
        .as_ref()
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "chain RPC not configured — external checkpoint binding is disabled",
            )
        })?
        .clone();
    let store = app.store.clone();
    let write_gate = app.write_gate.clone();
    let repo = body.repo.clone();
    let now = now_ms();
    // ureq is blocking; run off the async runtime. Hold the single-writer gate for
    // the fetch+write so the checkpoint is recorded atomically (fail-closed).
    let record = tokio::task::spawn_blocking(move || {
        let _gate = lock(&write_gate);
        mem_sync::chain::checkpoint_external_root(&store, &repo, &root, &rpc, CHAIN_ID_40204, now)
    })
    .await
    .map_err(ise)?
    .map_err(map_sync_err)?;
    Ok(Json(sign_checkpoint(&app.signing_key, &record)?))
}

/// `GET /api/orgs/:org/checkpoint?repo=<tenant>` — read back the latest signed
/// external-root checkpoint for a tenant. 204 when the tenant has never
/// checkpointed (never a fabricated record).
async fn read_checkpoint(
    State(app): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
    Query(q): Query<CheckpointQuery>,
) -> Result<Response, ApiError> {
    if q.repo.is_empty() {
        return Err(bad("repo query param required"));
    }
    let _sub = gate(
        &app,
        &headers,
        &org,
        &repo_resource(&q.repo),
        Op::Read,
        "checkpoint_read",
    )?;
    let store = app.store.clone();
    let repo = q.repo.clone();
    let record = tokio::task::spawn_blocking(move || {
        mem_sync::chain::latest_external_checkpoint(&store, &repo)
    })
    .await
    .map_err(ise)?
    .map_err(map_sync_err)?;
    match record {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some(rec) => Ok(Json(sign_checkpoint(&app.signing_key, &rec)?).into_response()),
    }
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
    obj.insert(
        "chain".into(),
        json!({
            "checkpoint_binding": app.chain_rpc.is_some(),
            "chain_id": CHAIN_ID_40204,
            "note": "external-root checkpoints bind to the live 40204 head (read-only); the on-chain WRITE (root → AnchorRegistry) is the deferred WP-5.3 follow-up (needs a funded signer + Rule-8).",
        }),
    );
    Ok(Json(Value::Object(obj)))
}

/// POST /connect/token — trade a verified OIDC id_token for a long-lived HS256
/// connect token, so a user/agent never handles a raw secret. This is what a
/// `citrate connect` one-click calls after login: it presents the id_token as a
/// bearer, and gets back a token it writes to its config. `authenticate` fails
/// closed when OIDC isn't configured; minting needs `MEM_CONNECT_SECRET`. It
/// asserts IDENTITY only — authz is still enforced at use time (the BYOM handler
/// checks org membership), so minting for any authenticated user is safe.
async fn connect_token(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let sub = authenticate(&app, &headers)?;
    let secret = app.connect_secret.as_ref().ok_or_else(|| {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, "connect minting not configured")
    })?;
    // MEM-B-008: short-lived and read-only by default. The token's authority is
    // the intersection of membership and these claims at use time; this
    // gateway-minted path deliberately asserts only read (a write BYOM token is
    // minted through the webapp with an explicit `read,propose` scope). Was 30
    // days + no scope, i.e. a long-lived full-org bearer credential.
    const TTL_SECS: usize = 15 * 60; // 15 minutes
    let now_secs = (now_ms() / 1000) as usize;
    let token = mint_connect_token(secret, &sub, Some("read"), &[], now_secs, TTL_SECS)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "mint failed"))?;
    Ok(Json(json!({
        "connect_token": token,
        "sub": sub,
        "scope": "read",
        "expires_in": TTL_SECS,
    })))
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
    let claims =
        verify_connect_token(secret, &token).map_err(|_| unauthorized("connect token rejected"))?;
    if claims.sub != sub {
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
    // MEM-B-008: the connect token is an ATTENUATION — intersect the membership
    // grant with the token's declared scope/tenants so it can never authorize more
    // than the user was shown (and never write when the token is read-only).
    let grant = attenuate_grant(&grant, &claims, &app.signing_key);
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

// --------------------------------------------------------------------------
// MEM-B-008 tests: a BYOM connect token is an ATTENUATION of the membership
// grant, never a silent full-membership credential. These exercise the pure
// core of the fix (`attenuate_grant`) — the same intersection `byom` applies.
// --------------------------------------------------------------------------

#[cfg(test)]
mod byom_attenuation_tests {
    use super::*;
    use crate::auth::signing_key_from_seed;
    use crate::control::{Membership, Role};

    fn owner_membership() -> Membership {
        Membership {
            sub: "owner-1".into(),
            org: "citrate-federation".into(),
            role: Role::OrgOwner,
            scopes: vec![],
            parent: None,
        }
    }

    /// MEM-B-008 tripwire: a token minted with `scope:"read"` must NOT authorize a
    /// write, even for an OrgOwner whose membership is `*` read+write. Before the
    /// fix the scope was dropped at verify and the owner's full grant was used.
    #[test]
    fn read_scope_denies_write_even_for_owner() {
        let sk = signing_key_from_seed("seed");
        let now = now_ms();
        let grant = mint_grant(&sk, "iss", &owner_membership(), now, GRANT_TTL_MS);
        assert!(
            grant.check("repo:citrate-chain/memory", Op::Write, now).is_ok(),
            "precondition: owner membership can write before attenuation"
        );
        let claims = ConnectClaims { sub: "owner-1".into(), scope: Some("read".into()), tenants: None };
        let att = attenuate_grant(&grant, &claims, &sk);
        assert!(att.check("repo:citrate-chain/memory", Op::Read, now).is_ok(), "read is preserved");
        assert!(
            att.check("repo:citrate-chain/memory", Op::Write, now).is_err(),
            "MEM-B-008: a read-scope token must not authorize write, even for an owner"
        );
    }

    /// A token's `tenants` list restricts the owner's `*` wildcard to exactly those
    /// repos — a tenant the token does not name is denied though membership covered it.
    #[test]
    fn tenants_claim_restricts_owner_wildcard() {
        let sk = signing_key_from_seed("seed");
        let now = now_ms();
        let grant = mint_grant(&sk, "iss", &owner_membership(), now, GRANT_TTL_MS);
        let claims = ConnectClaims {
            sub: "owner-1".into(),
            scope: Some("read,propose".into()),
            tenants: Some(vec!["citrate-landing".into()]),
        };
        let att = attenuate_grant(&grant, &claims, &sk);
        assert!(att.check("repo:citrate-landing/memory", Op::Write, now).is_ok(), "named tenant writable (propose ⇒ write)");
        assert!(
            att.check("repo:citrate-chain/memory", Op::Read, now).is_err(),
            "MEM-B-008: a tenant the token did not name is denied, though the owner membership covered it"
        );
    }

    /// A claimless (legacy) token defaults to read-only — least privilege, never a
    /// silent full-membership grant.
    #[test]
    fn absent_scope_defaults_to_read_only() {
        let sk = signing_key_from_seed("seed");
        let now = now_ms();
        let grant = mint_grant(&sk, "iss", &owner_membership(), now, GRANT_TTL_MS);
        let claims = ConnectClaims { sub: "owner-1".into(), scope: None, tenants: None };
        let att = attenuate_grant(&grant, &claims, &sk);
        assert!(att.check("repo:x/memory", Op::Read, now).is_ok());
        assert!(
            att.check("repo:x/memory", Op::Write, now).is_err(),
            "MEM-B-008: absent scope ⇒ least privilege (read-only)"
        );
    }
}

// --------------------------------------------------------------------------
// MEM-B-007 tests: `resolve_in_tenant` confines the tenant-blind
// `resolve_prefix` so a caller authorized on one repo cannot resolve (and thus
// read) a node in another tenant — a cross-tenant IDOR. Cross-tenant reads as
// 404, never 403, so existence does not leak.
// --------------------------------------------------------------------------

#[cfg(test)]
mod resolve_in_tenant_tests {
    use super::*;
    use mem_core::{Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION};
    use mem_store::kv::InMemoryKv;

    fn node(repo: &str, content: &str) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Rationale,
            repo: repo.into(),
            author: "ingest".into(),
            source_ref: SourceRef::DagNative { key: content.into() },
            content: content.as_bytes().to_vec(),
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

    #[test]
    fn foreign_tenant_prefix_reads_as_404_not_the_node() {
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        // A node that lives ONLY in citrate-chain.
        let secret = node("citrate-chain", "the chain's private decision");
        let id = store.put_node(&secret).unwrap();
        let prefix = &id.to_hex()[..12];
        let rc = Recall::new(&store);

        // A caller authorized on citrate-landing resolves the citrate-chain prefix:
        // MEM-B-007 — must be 404, and must NOT return the foreign node.
        let cross = resolve_in_tenant(&store, &rc, "citrate-landing", prefix);
        match cross {
            Err(e) => assert_eq!(e.status, StatusCode::NOT_FOUND, "cross-tenant hit must be 404 (existence must not leak)"),
            Ok((_, n)) => panic!("MEM-B-007: cross-tenant resolve leaked node from repo {:?}", n.repo),
        }

        // The owning tenant still resolves it (no false negative).
        let same = match resolve_in_tenant(&store, &rc, "citrate-chain", prefix) {
            Ok(v) => v,
            Err(e) => panic!("owning tenant must resolve, got status {}", e.status),
        };
        assert_eq!(same.0, id);
        assert_eq!(same.1.repo, "citrate-chain");
    }
}

// PBA R2 (2026-09-24 pre-bounty audit) regression tests: the lane-L6b PoCs with
// inverted assertions, driven through the real handlers.
#[cfg(test)]
#[path = "http_pba_r2_tests.rs"]
mod pba_r2_tests;
