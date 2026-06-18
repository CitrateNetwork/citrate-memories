//! The axum HTTP/JSON read API + 3D-layout endpoint (feature `server`).
//!
//! Every route is **Org-scoped** and passes through the same gate as the core:
//! authenticate the subject → check the **Org boundary** → check the repo-tenant
//! scope (`authorize`). Auth **fails closed**: with no OIDC verifier configured and
//! dev-auth off, every request is refused. Dev-auth (`x-dev-sub` header, behind an
//! explicit opt-in) is for the prototype + local dev only.

use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use mem_assert::{apply_diff, MemoryDiff};
use mem_authz::{MemoryEvent, Op, ResourceScope};
use mem_core::{ClaimStatus, ContentHash, EdgeKind, EdgeMethod, NodeKind};
use mem_index::Embedder;
use mem_query::Recall;

use crate::authz::{authorize, can_delegate, derive_grant};
use crate::layout::{build_scene, SceneEdge};
use crate::org::{ControlPlane, Membership, Org, OrgId, OrgStatus, Role};
use crate::registry::OrgEngines;
use crate::GatewayError;

type ApiResult = Result<Json<Value>, (StatusCode, String)>;
/// Org → (node-count it was built for, the cached scene JSON).
type LayoutCache = std::sync::Mutex<std::collections::HashMap<OrgId, (usize, Arc<Value>)>>;

/// Shared gateway state (cheap to clone — everything is Arc).
#[derive(Clone)]
pub struct AppState {
    pub control: Arc<RwLock<ControlPlane>>,
    pub engines: Arc<OrgEngines>,
    pub issuer_key: Arc<ed25519_dalek::SigningKey>,
    /// Only true behind an explicit opt-in; never in production.
    pub allow_dev_auth: bool,
    pub grant_ttl_ms: u64,
    /// Query embedder for `search` (matches the store's model). `None` ⇒ search is
    /// unavailable (returns a clear error rather than wrong results).
    pub embedder: Option<Arc<dyn Embedder>>,
    /// Cached 3D scene per Org, keyed by node-count (the store-state proxy): the PCA
    /// projection is expensive, so compute once and reuse until the graph changes.
    pub layout_cache: Arc<LayoutCache>,
    /// Real OIDC bearer verifier (gap G-2). When present, a `Bearer` token is
    /// verified against the authority JWKS (iss+aud+exp). `None` ⇒ no OIDC
    /// configured (dev-auth or fail-closed only).
    pub oidc: Option<Arc<crate::oidc::OidcVerifier>>,
    /// Signing identity for the write routes (gap G-1). `None` ⇒ assert/propose
    /// return 503 (read-only deployment).
    pub asserter: Option<Arc<mem_assert::Asserter>>,
    /// Tamper-evident audit chain for mutations (and denials). `None` ⇒ writes
    /// proceed unaudited (dev/tests); production wires a persistent chain.
    pub audit: Option<Arc<std::sync::Mutex<mem_authz::AuditChain>>>,
    /// Where the durable control plane lives (gap G-3). When set, provisioning
    /// mutations (gap G-4) persist through it. `None` ⇒ in-memory only (tests).
    pub control_path: Option<Arc<std::path::PathBuf>>,
}

impl AppState {
    /// Construct with an empty layout cache.
    pub fn new(
        control: Arc<RwLock<ControlPlane>>,
        engines: Arc<OrgEngines>,
        issuer_key: Arc<ed25519_dalek::SigningKey>,
        allow_dev_auth: bool,
        grant_ttl_ms: u64,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        Self {
            control,
            engines,
            issuer_key,
            allow_dev_auth,
            grant_ttl_ms,
            embedder,
            layout_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            oidc: None,
            asserter: None,
            audit: None,
            control_path: None,
        }
    }

    /// Where to persist the control plane after provisioning mutations (gap G-4).
    pub fn with_control_path(mut self, path: Arc<std::path::PathBuf>) -> Self {
        self.control_path = Some(path);
        self
    }

    /// Attach a real OIDC bearer verifier (gap G-2). With this set, the gateway
    /// verifies `Authorization: Bearer` tokens against the authority JWKS; the
    /// dev-auth header is only consulted when no verifier is configured.
    pub fn with_oidc(mut self, verifier: Arc<crate::oidc::OidcVerifier>) -> Self {
        self.oidc = Some(verifier);
        self
    }

    /// Attach the signing identity that the write routes use (gap G-1).
    pub fn with_asserter(mut self, asserter: Arc<mem_assert::Asserter>) -> Self {
        self.asserter = Some(asserter);
        self
    }

    /// Attach the audit chain that records mutations + denials (gap G-1).
    pub fn with_audit(mut self, audit: Arc<std::sync::Mutex<mem_authz::AuditChain>>) -> Self {
        self.audit = Some(audit);
        self
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The `Authorization: Bearer` token, if present and non-empty.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get("authorization").and_then(|v| v.to_str().ok())?;
    let tok = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?.trim();
    if tok.is_empty() { None } else { Some(tok) }
}

/// Authenticate the caller → their OIDC subject. Fails closed.
///
/// Order: a real OIDC verifier (gap G-2) takes precedence — when configured, a
/// `Bearer` token is verified against the authority JWKS (iss+aud+exp) and a bad
/// or missing token is refused. Dev-auth (`x-dev-sub`) is only consulted when no
/// verifier is configured (local/prototype). With neither, every request is
/// refused — never default-open.
fn authn(headers: &HeaderMap, state: &AppState) -> Result<String, (StatusCode, String)> {
    if let Some(verifier) = &state.oidc {
        let token = bearer_token(headers)
            .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token".to_string()))?;
        return verifier
            .verify(token)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid token".to_string()));
    }
    if state.allow_dev_auth {
        if let Some(sub) = headers.get("x-dev-sub").and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()) {
            return Ok(sub.to_string());
        }
        return Err((StatusCode::UNAUTHORIZED, "dev-auth on: provide x-dev-sub".into()));
    }
    Err((StatusCode::UNAUTHORIZED, "authentication not configured".into()))
}

fn map_gateway_err(e: GatewayError) -> (StatusCode, String) {
    let code = match e {
        GatewayError::NotAMember | GatewayError::Authz(_) => StatusCode::FORBIDDEN,
        GatewayError::OrgSuspended => StatusCode::FORBIDDEN,
        GatewayError::EngineUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        GatewayError::Attenuation(_) | GatewayError::NotADelegator => StatusCode::FORBIDDEN,
        GatewayError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

fn store_err(e: mem_store::StoreError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn resource_for(tenant: &str) -> String {
    format!("repo:{tenant}/memory")
}

/// Authorize a read on one repo-tenant; `Ok(())` or a ready HTTP error.
fn authz_read(state: &AppState, sub: &str, org: &OrgId, tenant: &str) -> Result<(), (StatusCode, String)> {
    let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    authorize(&control, sub, org, &resource_for(tenant), Op::Read, now_ms(), state.grant_ttl_ms, &state.issuer_key)
        .map_err(map_gateway_err)
}

/// Can this subject read this repo-tenant? (boolean form for layout filtering)
fn can_read(state: &AppState, sub: &str, org: &OrgId, tenant: &str) -> bool {
    authz_read(state, sub, org, tenant).is_ok()
}

/// Authorize a WRITE on one repo-tenant (gap G-1). A denial is audited (if a chain
/// is configured) before the HTTP error is returned, so refused mutations are
/// recorded too.
fn authz_write(state: &AppState, sub: &str, org: &OrgId, tenant: &str) -> Result<(), (StatusCode, String)> {
    let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    let res = authorize(&control, sub, org, &resource_for(tenant), Op::Write, now_ms(), state.grant_ttl_ms, &state.issuer_key);
    drop(control);
    if let Err(e) = &res {
        audit(state, MemoryEvent::Denied, sub, &resource_for(tenant), &format!("write denied: {e}"));
    }
    res.map_err(map_gateway_err)
}

/// Append a mutation/denial to the audit chain when one is configured. Best-effort
/// (a poisoned lock or IO error must not crash a request), but the chain itself is
/// tamper-evident + fsync-through.
fn audit(state: &AppState, event: MemoryEvent, actor: &str, resource: &str, detail: &str) {
    if let Some(chain) = &state.audit {
        if let Ok(mut guard) = chain.lock() {
            let _ = guard.append(event, actor, resource, detail, now_ms());
        }
    }
}

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

// ---- handlers ----

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "mem-gateway" }))
}

async fn list_orgs(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    let orgs: Vec<Value> = control
        .orgs_for(&sub)
        .into_iter()
        .map(|o| json!({ "id": o.id.as_str(), "name": o.name, "status": format!("{:?}", o.status) }))
        .collect();
    Ok(Json(json!({ "orgs": orgs })))
}

async fn list_tenants(State(state): State<AppState>, headers: HeaderMap, Path(org): Path<String>) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    // Distinct repos in the store that the caller may read.
    let mut repos: Vec<String> = engine
        .all_nodes()
        .map_err(store_err)?
        .into_iter()
        .map(|n| n.repo)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|r| can_read(&state, &sub, &org, r))
        .collect();
    repos.sort();
    Ok(Json(json!({ "tenants": repos })))
}

#[derive(Deserialize)]
struct BudgetQ {
    budget: Option<usize>,
}

async fn recall(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, tenant)): Path<(String, String)>,
    Query(q): Query<BudgetQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    authz_read(&state, &sub, &org, &tenant)?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let res = Recall::new(&engine).storyline(&tenant, q.budget.unwrap_or(20)).map_err(store_err)?;
    Ok(Json(recall_json(&res)))
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    budget: Option<usize>,
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, tenant)): Path<(String, String)>,
    Query(query): Query<SearchQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    authz_read(&state, &sub, &org, &tenant)?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let embedder = state
        .embedder
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "search unavailable: embedder not loaded".to_string()))?;
    let recall = Recall::with_embedder(&engine, Box::new(Arc::clone(embedder)));
    let res = recall.search(&tenant, &query.q, query.budget.unwrap_or(10)).map_err(store_err)?;
    Ok(Json(recall_json(&res)))
}

#[derive(Deserialize)]
struct AsOfQ {
    t: u64,
    budget: Option<usize>,
}

async fn as_of(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, tenant)): Path<(String, String)>,
    Query(q): Query<AsOfQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    authz_read(&state, &sub, &org, &tenant)?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let res = Recall::new(&engine).as_of(&tenant, q.t, q.budget.unwrap_or(20)).map_err(store_err)?;
    Ok(Json(recall_json(&res)))
}

/// Resolve a node by id-prefix within an Org, returning it only if the caller can
/// read its repo-tenant (so existence in an unreadable tenant doesn't leak).
fn resolve_readable(
    state: &AppState,
    sub: &str,
    org: &OrgId,
    engine: &crate::registry::Engine,
    id_prefix: &str,
) -> Result<mem_core::MemoryNode, (StatusCode, String)> {
    let recall = Recall::new(engine);
    let id = recall
        .resolve_prefix(id_prefix)
        .map_err(store_err)?
        .ok_or((StatusCode::NOT_FOUND, "no unique node for id".to_string()))?;
    let node = engine.get_node(&id).map_err(store_err)?.ok_or((StatusCode::NOT_FOUND, "node not found".to_string()))?;
    if !can_read(state, sub, org, &node.repo) {
        // Don't leak: an unreadable node reads as not-found.
        return Err((StatusCode::NOT_FOUND, "no unique node for id".to_string()));
    }
    Ok(node)
}

async fn node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, id)): Path<(String, String)>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let n = resolve_readable(&state, &sub, &org, &engine, &id)?;
    Ok(Json(node_json(&n)))
}

async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, id)): Path<(String, String)>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let n = resolve_readable(&state, &sub, &org, &engine, &id)?;
    let v = Recall::new(&engine)
        .verify(&n.compute_id())
        .map_err(store_err)?
        .ok_or((StatusCode::NOT_FOUND, "node not found".to_string()))?;
    let sig = match &v.signature {
        mem_query::SignatureStatus::NotApplicableDerived => json!({ "kind": "derived", "valid": true }),
        mem_query::SignatureStatus::Valid => json!({ "kind": "asserted", "valid": true }),
        mem_query::SignatureStatus::Invalid(e) => json!({ "kind": "asserted", "valid": false, "error": e }),
        mem_query::SignatureStatus::Missing => json!({ "kind": "asserted", "valid": false, "error": "missing" }),
    };
    Ok(Json(json!({
        "id": n.compute_id().to_hex(),
        "trustworthy": v.is_trustworthy(),
        "signature": sig,
        "superseded_by": v.superseded_by.iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
        "refuted_by": v.refuted_by.iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
        "contradicted": v.contradicted,
    })))
}

async fn neighbors(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, id)): Path<(String, String)>,
    Query(q): Query<BudgetQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let n = resolve_readable(&state, &sub, &org, &engine, &id)?;
    let nbrs = Recall::new(&engine).neighbors(&n.compute_id(), q.budget.unwrap_or(25)).map_err(store_err)?;
    let items: Vec<Value> = nbrs
        .into_iter()
        // grant-intersection: only show neighbours in tenants the caller can read.
        .filter(|nb| nb.node.as_ref().map(|x| can_read(&state, &sub, &org, &x.repo)).unwrap_or(true))
        .map(|nb| {
            json!({
                "edge_kind": format!("{:?}", nb.edge_kind),
                "direction": format!("{:?}", nb.direction),
                "quarantined": nb.quarantined,
                "node": nb.node.as_ref().map(node_item_json),
            })
        })
        .collect();
    Ok(Json(json!({ "id": n.compute_id().to_hex(), "neighbors": items })))
}

#[derive(Deserialize)]
struct AnalogyQ {
    repo: String,
    id: String,
    budget: Option<usize>,
}

async fn analogy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<AnalogyQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    authz_read(&state, &sub, &org, &q.repo)?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let recall = Recall::new(&engine);
    let id = recall.resolve_prefix(&q.id).map_err(store_err)?.ok_or((StatusCode::NOT_FOUND, "no unique node".to_string()))?;
    // Grant-intersection candidate set: only tenants the caller may read.
    let candidate_repos: Vec<String> = engine
        .all_nodes()
        .map_err(store_err)?
        .into_iter()
        .map(|n| n.repo)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|r| can_read(&state, &sub, &org, r))
        .collect();
    let cands = recall.analogies(&id, &candidate_repos, q.budget.unwrap_or(5)).map_err(store_err)?;
    let items: Vec<Value> = cands
        .into_iter()
        .filter(|c| can_read(&state, &sub, &org, &c.item.repo))
        .map(|c| json!({ "node": node_item_json(&c.item), "cosine": c.cosine, "structural": c.structural, "score": c.score }))
        .collect();
    Ok(Json(json!({ "analogues": items })))
}

/// The 3D constellation scene: every readable node with a position + visual
/// attributes, plus the edges among them. One request loads the whole scene.
async fn layout(State(state): State<AppState>, headers: HeaderMap, Path(org): Path<String>) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let value = build_scene_cached(&state, &sub, &org)?;
    Ok(Json((*value).clone()))
}

// ---- write routes (gap G-1): assert / propose_edge / confirm_edge ----
//
// Each is **write-scoped** (the Org boundary, then `Op::Write` on the repo-tenant),
// signs via the configured asserter, mutates the Org's isolated store, and audits
// the mutation. Denials are audited too. Mirrors the MCP `memory.*` write tools.

#[derive(Deserialize)]
struct AssertBody {
    content: String,
    #[serde(default)]
    kind: Option<String>,
}

/// Append a signed assertion to the Asserted plane of one repo-tenant.
async fn assert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, tenant)): Path<(String, String)>,
    Json(body): Json<AssertBody>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let Some(asserter) = state.asserter.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no signing identity configured".into()));
    };
    authz_write(&state, &sub, &org, &tenant)?;
    let kind = node_kind_from_str(body.kind.as_deref().unwrap_or("rationale"));
    let node = asserter.assert_node(&tenant, kind, &body.content, now_ms());
    let id = node.compute_id();
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    engine.put_node(&node).map_err(store_err)?;
    audit(&state, MemoryEvent::Write, &sub, &resource_for(&tenant), &format!("assert {} {}", node.kind.discriminant(), id.to_hex()));
    // The node count changed → drop the cached 3D scene for this Org.
    if let Ok(mut c) = state.layout_cache.lock() {
        c.remove(&org);
    }
    Ok(Json(json!({ "id": id.to_hex(), "kind": node.kind.discriminant(), "repo": tenant })))
}

#[derive(Deserialize)]
struct ProposeBody {
    from: String,
    to: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
}

/// Propose a QUARANTINED edge between two readable nodes (advisory until confirmed).
async fn propose_edge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<ProposeBody>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let Some(asserter) = state.asserter.clone() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no signing identity configured".into()));
    };
    let kind = edge_kind_from_str(body.kind.as_deref().unwrap_or("analogous_to"))
        .ok_or((StatusCode::BAD_REQUEST, "unknown edge kind".to_string()))?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let from = resolve_readable(&state, &sub, &org, &engine, &body.from)?;
    let to = resolve_readable(&state, &sub, &org, &engine, &body.to)?;
    // Write-gated on BOTH endpoint tenants.
    authz_write(&state, &sub, &org, &from.repo)?;
    authz_write(&state, &sub, &org, &to.repo)?;
    let (from_id, to_id) = (from.compute_id(), to.compute_id());
    let edge = asserter.propose_edge(from_id, to_id, kind, EdgeMethod::Manual, body.evidence, now_ms());
    engine.add_edge(&edge).map_err(store_err)?;
    audit(&state, MemoryEvent::Write, &sub, &resource_for(&from.repo), &format!("propose_edge {kind:?} {} -> {}", from_id.to_hex(), to_id.to_hex()));
    Ok(Json(json!({ "from": from_id.to_hex(), "to": to_id.to_hex(), "kind": format!("{kind:?}"), "quarantined": true })))
}

#[derive(Deserialize)]
struct ConfirmBody {
    from: String,
    to: String,
    kind: String,
}

/// Promote a quarantined proposal to load-bearing (HITL confirm).
async fn confirm_edge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<ConfirmBody>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let kind = edge_kind_from_str(&body.kind)
        .ok_or((StatusCode::BAD_REQUEST, "unknown edge kind".to_string()))?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let from = resolve_readable(&state, &sub, &org, &engine, &body.from)?;
    let to = resolve_readable(&state, &sub, &org, &engine, &body.to)?;
    authz_write(&state, &sub, &org, &from.repo)?;
    authz_write(&state, &sub, &org, &to.repo)?;
    let (from_id, to_id) = (from.compute_id(), to.compute_id());
    let outcome = engine
        .confirm_edge(&from_id, &to_id, kind)
        .map_err(|e| (StatusCode::CONFLICT, format!("confirm rejected: {e}")))?;
    let status = match outcome {
        mem_store::ConfirmOutcome::Confirmed => "confirmed",
        mem_store::ConfirmOutcome::AlreadyConfirmed => "already_confirmed",
        mem_store::ConfirmOutcome::NotFound => return Err((StatusCode::NOT_FOUND, "no such quarantined edge".into())),
    };
    audit(&state, MemoryEvent::Write, &sub, &resource_for(&from.repo), &format!("confirm_edge {kind:?} {} -> {}", from_id.to_hex(), to_id.to_hex()));
    Ok(Json(json!({ "from": from_id.to_hex(), "to": to_id.to_hex(), "kind": format!("{kind:?}"), "status": status })))
}

/// Build (or serve cached) the 3D scene for an Org, filtered to the tenants `sub` may
/// read. Cached per Org by node-count (the store-state proxy mem-query's index cache
/// uses). Shared by the HTTP handler and the startup warm so the expensive PCA runs
/// at most once per store-state.
///
/// (The cache is keyed by Org, not by member: in a multi-reader Org all members get
/// the same scene built for the first reader. Per-member tenant filtering of a shared
/// scene is an M1 refinement ⟦DECIDE⟧; for M0/dev the Org Owner loads the full scene.)
pub fn build_scene_cached(state: &AppState, sub: &str, org: &OrgId) -> Result<Arc<Value>, (StatusCode, String)> {
    let engine = state.engines.get(org).map_err(map_gateway_err)?;
    let node_count = engine.node_count().map_err(store_err)?;
    if let Ok(cache) = state.layout_cache.lock() {
        if let Some((n, scene)) = cache.get(org) {
            if *n == node_count {
                return Ok(Arc::clone(scene));
            }
        }
    }

    // Filter to readable tenants (decision cached per repo).
    let all = engine.all_nodes().map_err(store_err)?;
    let mut repo_ok: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let nodes: Vec<mem_core::MemoryNode> = all
        .into_iter()
        .filter(|n| *repo_ok.entry(n.repo.clone()).or_insert_with(|| can_read(state, sub, org, &n.repo)))
        .collect();

    // Edges among kept nodes (compute degree + the scene edge list in one pass).
    let kept: std::collections::HashSet<String> = nodes.iter().map(|n| n.compute_id().to_hex()).collect();
    let mut degree: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut edges: Vec<SceneEdge> = Vec::new();
    for n in &nodes {
        let from = n.compute_id();
        let from_hex = from.to_hex();
        for e in engine.out_edges(&from).map_err(store_err)? {
            let to_hex = e.to.to_hex();
            if kept.contains(&to_hex) {
                *degree.entry(from_hex.clone()).or_default() += 1;
                *degree.entry(to_hex.clone()).or_default() += 1;
                edges.push(SceneEdge { from: from_hex.clone(), to: to_hex, kind: format!("{:?}", e.kind), quarantined: e.quarantined });
            }
        }
    }

    let scene = build_scene(&nodes, edges, |id| degree.get(id).copied().unwrap_or(0));
    let value = Arc::new(serde_json::to_value(scene).unwrap_or_else(|_| json!({ "error": "serialize" })));
    if let Ok(mut cache) = state.layout_cache.lock() {
        cache.insert(org.clone(), (node_count, Arc::clone(&value)));
    }
    Ok(value)
}

// ---- JSON mappers (the API contract lives here, not in the engine types) ----

fn node_item_json(i: &mem_query::RecallItem) -> Value {
    json!({
        "id": i.id.to_hex(),
        "kind": i.kind.discriminant(),
        "lane": crate::layout::material_lane(&i.kind),
        "repo": i.repo,
        "title": i.title.split('\n').next().unwrap_or(""),
        "plane": format!("{:?}", i.plane).to_lowercase(),
        "trust": format!("{:?}", i.trust_tier),
        "status": format!("{:?}", i.status).to_lowercase(),
        "valid_from": i.valid_from,
        "score": i.score,
    })
}

fn node_json(n: &mem_core::MemoryNode) -> Value {
    json!({
        "id": n.compute_id().to_hex(),
        "kind": n.kind.discriminant(),
        "lane": crate::layout::material_lane(&n.kind),
        "repo": n.repo,
        "author": n.author,
        "title": String::from_utf8_lossy(&n.content).split('\n').next().unwrap_or("").to_string(),
        "plane": format!("{:?}", n.plane).to_lowercase(),
        "trust": format!("{:?}", n.trust_tier),
        "status": format!("{:?}", n.status).to_lowercase(),
        "valid_from": n.valid_from,
        "valid_to": n.valid_to,
        "source_ref": format!("{:?}", n.source_ref),
    })
}

fn recall_json(r: &mem_query::RecallResult) -> Value {
    json!({
        "repo": r.repo,
        "total_in_tenant": r.total_in_tenant,
        "watermark": r.watermark.as_ref().map(|w| json!({
            "head": w.head, "head_count": w.head_count, "ingested_at_ms": w.ingested_at_ms,
        })),
        "items": r.items.iter().map(node_item_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org::{Membership, Org, OrgStatus, Role};
    use mem_core::{MemoryNode, NodeKind, Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION};
    use mem_store::kv::InMemoryKv;
    use mem_store::MemoryDagStore;

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

    fn state_with(nodes: &[MemoryNode], member: Membership) -> (AppState, OrgId) {
        let org = OrgId::new("acme");
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        store.commit(nodes, &[]).unwrap();
        let engines = Arc::new(OrgEngines::new());
        engines.register(org.clone(), Arc::new(store)).unwrap();
        let mut cp = ControlPlane::new();
        cp.upsert_org(Org { id: org.clone(), name: "acme".into(), created_at_ms: 1, store_path: "mem".into(), status: OrgStatus::Active });
        cp.upsert_membership(member);
        let state = AppState::new(
            Arc::new(RwLock::new(cp)),
            engines,
            Arc::new(ed25519_dalek::SigningKey::from_bytes(&[5u8; 32])),
            true,
            3_600_000,
            None,
        );
        (state, org)
    }

    fn member(role: Role, scopes: Vec<mem_authz::ResourceScope>) -> Membership {
        Membership { sub: "u".into(), org: OrgId::new("acme"), role, scopes, parent: None }
    }

    /// Gap G-2 wiring: when an OIDC verifier is attached, `authn` verifies the
    /// bearer (and refuses on bad/missing), and the dev-auth header is ignored.
    #[test]
    fn authn_oidc_takes_precedence_and_fails_closed() {
        let (state, _) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        // dev-auth alone (no OIDC) accepts x-dev-sub.
        let mut dev = HeaderMap::new();
        dev.insert("x-dev-sub", "u".parse().unwrap());
        assert_eq!(authn(&dev, &state).unwrap(), "u");

        // Attach an OIDC verifier — now a bearer is required and verified; the
        // dev-auth header no longer opens the door.
        let jwks = serde_json::json!({ "keys": [{ "kty": "RSA", "use": "sig", "alg": "RS256",
            "kid": "k1", "n": "ndUDVqo8DrYXkGs7uzDoQ9vMUkF7ZjF6m9XKxwWCHg0MZqaPJtG3Bwi48bfTLmnEvTs_PtHpLtU6XtmCu8rdOzpTM10yK5qLRDd0jcNPhMjIZf5MP5VTebkUmp5A9eGxdhh1tkoSv0SM_BBoLNInq_X6TUVWNmvDxYD21YNY_Y6iNP-VsaDrxdn7BejnNrULCDidjbn2f15ma0F0xZg4kiurVMu72XzXpsbqUCN_on60jEa2auYJXCUShlJIfZQ4e449Se8o0X_NhjhxbgMfSAegRsI5i42oskNmU9z1TihFQXcWQG42aHbJVZmMOpnuOUn7i-_yjey49IFznr4IsQ",
            "e": "AQAB" }] }).to_string();
        let verifier = Arc::new(crate::oidc::OidcVerifier::from_jwks_json("https://auth.citrate.ai", "memrizz", &jwks).unwrap());
        let state = state.with_oidc(verifier);

        // dev-sub header is ignored; with no bearer → refused.
        assert!(authn(&dev, &state).is_err());
        // a bogus bearer → refused.
        let mut bad = HeaderMap::new();
        bad.insert("authorization", "Bearer not.a.jwt".parse().unwrap());
        assert!(authn(&bad, &state).is_err());
    }

    fn dev(sub: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-dev-sub", sub.parse().unwrap());
        h
    }
    fn with_signer(state: AppState) -> AppState {
        state.with_asserter(Arc::new(mem_assert::Asserter::new(ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]))))
    }

    /// G-1: a write-scoped member's assertion lands in the Org's store.
    #[tokio::test]
    async fn assert_writes_for_a_write_scoped_member() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let state = with_signer(state);
        let engine = state.engines.get(&org).unwrap();
        let before = engine.node_count().unwrap();
        let res = assert(
            State(state.clone()),
            dev("u"),
            Path((org.as_str().to_string(), "a".to_string())),
            Json(AssertBody { content: "we chose per-Org isolation".into(), kind: Some("rationale".into()) }),
        )
        .await;
        assert!(res.is_ok(), "owner assert should succeed: {res:?}");
        assert_eq!(engine.node_count().unwrap(), before + 1);
    }

    /// G-1 fail-closed: a read-only member is refused (and the write never lands).
    #[tokio::test]
    async fn assert_is_denied_for_a_read_only_member() {
        let ro = Membership {
            sub: "u".into(),
            org: OrgId::new("acme"),
            role: Role::Member,
            scopes: vec![mem_authz::ResourceScope { resource_id: "repo:a/memory".into(), can_read: true, can_write: false }],
            parent: None,
        };
        let (state, org) = state_with(&[node("a", "n1")], ro);
        let state = with_signer(state);
        let engine = state.engines.get(&org).unwrap();
        let before = engine.node_count().unwrap();
        let res = assert(
            State(state.clone()),
            dev("u"),
            Path((org.as_str().to_string(), "a".to_string())),
            Json(AssertBody { content: "x".into(), kind: None }),
        )
        .await;
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(engine.node_count().unwrap(), before, "denied write must not land");
    }

    /// G-1: without a signing identity, writes are 503 (read-only deployment).
    #[tokio::test]
    async fn assert_503_without_a_signing_identity() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let res = assert(
            State(state),
            dev("u"),
            Path((org.as_str().to_string(), "a".to_string())),
            Json(AssertBody { content: "x".into(), kind: None }),
        )
        .await;
        assert_eq!(res.unwrap_err().0, StatusCode::SERVICE_UNAVAILABLE);
    }

    fn scope(id: &str, r: bool, w: bool) -> ResourceScope {
        ResourceScope { resource_id: id.into(), can_read: r, can_write: w }
    }

    /// G-4: an Org Owner onboards an attenuated member; the delegation edge (parent) is recorded.
    #[tokio::test]
    async fn owner_can_onboard_an_attenuated_member() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let res = add_member(
            State(state.clone()),
            dev("u"),
            Path(org.as_str().to_string()),
            Json(AddMemberBody { sub: "newbie".into(), role: "member".into(), scopes: vec![scope("repo:a/memory", true, false)] }),
        )
        .await;
        assert!(res.is_ok(), "owner onboard should succeed: {res:?}");
        let cp = state.control.read().unwrap();
        assert_eq!(cp.membership("newbie", &org).unwrap().parent.as_deref(), Some("u"));
    }

    /// G-4 fail-closed: a plain member cannot onboard anyone.
    #[tokio::test]
    async fn a_plain_member_cannot_onboard() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        state.control.write().unwrap().upsert_membership(Membership {
            sub: "m".into(), org: org.clone(), role: Role::Member, scopes: vec![scope("repo:a/memory", true, false)], parent: Some("u".into()),
        });
        let res = add_member(
            State(state),
            dev("m"),
            Path(org.as_str().to_string()),
            Json(AddMemberBody { sub: "x".into(), role: "member".into(), scopes: vec![] }),
        )
        .await;
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    /// G-4 fail-closed: attenuation — an admin can't grant a scope they don't hold.
    #[tokio::test]
    async fn attenuation_is_enforced_on_onboard() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        state.control.write().unwrap().upsert_membership(Membership {
            sub: "d".into(), org: org.clone(), role: Role::OrgAdmin, scopes: vec![scope("repo:a/memory", true, false)], parent: Some("u".into()),
        });
        let res = add_member(
            State(state),
            dev("d"),
            Path(org.as_str().to_string()),
            Json(AddMemberBody { sub: "x".into(), role: "member".into(), scopes: vec![scope("repo:a/memory", true, true)] }),
        )
        .await;
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    /// G-4 (F-7): revoking an admin cascades to everyone delegated beneath them.
    #[tokio::test]
    async fn revoke_cascades_the_subtree() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        {
            let mut cp = state.control.write().unwrap();
            cp.upsert_membership(Membership { sub: "d".into(), org: org.clone(), role: Role::OrgAdmin, scopes: vec![], parent: Some("u".into()) });
            cp.upsert_membership(Membership { sub: "m".into(), org: org.clone(), role: Role::Member, scopes: vec![], parent: Some("d".into()) });
        }
        let res = revoke_member(State(state.clone()), dev("u"), Path((org.as_str().to_string(), "d".to_string()))).await;
        assert!(res.is_ok());
        let cp = state.control.read().unwrap();
        assert!(cp.membership("d", &org).is_none(), "admin removed");
        assert!(cp.membership("m", &org).is_none(), "delegated member cascaded");
    }

    /// Org creation: a new Org's store is initialized and the caller is bound as the
    /// founding Owner. (The in-memory engine is pre-registered so the no-rocksdb test
    /// build skips the RocksDB open; production opens a fresh isolated store.)
    #[tokio::test]
    async fn create_org_inits_a_store_and_binds_the_owner() {
        let (state, _acme) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let new_id = OrgId::new("newco");
        state.engines.register(new_id.clone(), Arc::new(MemoryDagStore::new(Box::new(InMemoryKv::new())))).unwrap();
        let res = create_org(State(state.clone()), dev("founder"), Json(CreateOrgBody { name: "NewCo".into(), id: None })).await;
        assert!(res.is_ok(), "create should succeed: {res:?}");
        let cp = state.control.read().unwrap();
        assert!(cp.org(&new_id).is_some(), "org reserved");
        let m = cp.membership("founder", &new_id).unwrap();
        assert_eq!(m.role, Role::OrgOwner);
        assert!(m.parent.is_none(), "founder is a root grant");
    }

    /// Org creation fail-closed: a duplicate id is refused (no store re-init).
    #[tokio::test]
    async fn create_org_rejects_a_duplicate() {
        let (state, _acme) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        // state_with already created "acme"; "Acme" slugs to "acme".
        let res = create_org(State(state), dev("u"), Json(CreateOrgBody { name: "Acme".into(), id: None })).await;
        assert_eq!(res.unwrap_err().0, StatusCode::CONFLICT);
    }

    /// Org creation: an unusable name (empty / no alphanumerics) is rejected.
    #[tokio::test]
    async fn create_org_rejects_an_unusable_name() {
        let (state, _) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let blank = create_org(State(state.clone()), dev("u"), Json(CreateOrgBody { name: "   ".into(), id: None })).await;
        assert_eq!(blank.unwrap_err().0, StatusCode::BAD_REQUEST);
        let punct = create_org(State(state), dev("u"), Json(CreateOrgBody { name: "!!!".into(), id: None })).await;
        assert_eq!(punct.unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    /// Audit viewer: Org-scoped + integrity-verified. Admin sees the whole chain,
    /// a member sees only their own actions.
    #[tokio::test]
    async fn audit_log_admin_sees_all_member_sees_own() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let mut chain = mem_authz::AuditChain::new();
        chain.append(MemoryEvent::Write, "u", "repo:a/memory", "assert x", 10).unwrap();
        chain.append(MemoryEvent::Denied, "intruder", "repo:a/memory", "denied", 11).unwrap();
        chain.append(MemoryEvent::Write, "m", "repo:a/memory", "m did a thing", 12).unwrap();
        let state = state.with_audit(Arc::new(std::sync::Mutex::new(chain)));
        state.control.write().unwrap().upsert_membership(Membership {
            sub: "m".into(), org: org.clone(), role: Role::Member, scopes: vec![scope("repo:a/memory", true, true)], parent: Some("u".into()),
        });

        // Owner (admin) sees all 3 records, chain intact.
        let v = audit_log(State(state.clone()), dev("u"), Path(org.as_str().to_string()), Query(AuditQ { limit: None })).await.unwrap().0;
        assert_eq!(v["intact"], true);
        assert_eq!(v["length"], 3);
        assert_eq!(v["records"].as_array().unwrap().len(), 3);

        // Member "m" sees only their own action.
        let v2 = audit_log(State(state), dev("m"), Path(org.as_str().to_string()), Query(AuditQ { limit: None })).await.unwrap().0;
        let recs = v2["records"].as_array().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["actor"], "m");
    }

    /// Ops surface: store counts + per-tenant anchor posture (root + live-match).
    #[tokio::test]
    async fn ops_reports_counts_and_anchor_posture() {
        let (state, org) = state_with(&[node("a", "n1"), node("a", "n2")], member(Role::OrgOwner, vec![]));
        let engine = state.engines.get(&org).unwrap();
        mem_sync::anchor_tenant(&engine, "a", 100).unwrap();
        let v = ops(State(state), dev("u"), Path(org.as_str().to_string())).await.unwrap().0;
        assert_eq!(v["store"]["nodes"], 2);
        assert_eq!(v["checkpoints"]["count"], 0);
        let a = v["tenants"].as_array().unwrap().iter().find(|t| t["repo"] == "a").unwrap();
        assert!(a["anchor"]["root"].is_string(), "anchor present: {a}");
        assert_eq!(a["anchor_valid"], true, "freshly anchored → live root matches");
    }

    /// Ops is admin-gated: a plain member is refused.
    #[tokio::test]
    async fn ops_requires_an_admin() {
        let plain = Membership { sub: "u".into(), org: OrgId::new("acme"), role: Role::Member, scopes: vec![scope("repo:a/memory", true, false)], parent: None };
        let (state, org) = state_with(&[node("a", "n1")], plain);
        let res = ops(State(state), dev("u"), Path(org.as_str().to_string())).await;
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    /// Org-boundary: a non-member can't read the audit log.
    #[tokio::test]
    async fn audit_log_refuses_a_non_member() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let res = audit_log(State(state), dev("stranger"), Path(org.as_str().to_string()), Query(AuditQ { limit: None })).await;
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    fn signed_diff(repo: &str) -> String {
        let asserter = mem_assert::Asserter::new(ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]));
        let n = asserter.assert_node(repo, NodeKind::Rationale, "session handoff note", 100);
        let mut diff = MemoryDiff::new(asserter.pubkey_hex().to_string(), 100);
        diff.add_node(n);
        diff.to_json().unwrap()
    }

    /// merge_diff: a writer's signed session subgraph is applied to the store.
    #[tokio::test]
    async fn merge_diff_applies_a_signed_subgraph_for_a_writer() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let engine = state.engines.get(&org).unwrap();
        let before = engine.node_count().unwrap();
        let res = merge_diff(State(state.clone()), dev("u"), Path(org.as_str().to_string()), Bytes::from(signed_diff("a"))).await;
        assert!(res.is_ok(), "writer merge should succeed: {res:?}");
        assert_eq!(engine.node_count().unwrap(), before + 1);
    }

    /// merge_diff fail-closed: a read-only member is refused (write-gated per repo).
    #[tokio::test]
    async fn merge_diff_is_denied_for_a_read_only_member() {
        let ro = Membership { sub: "u".into(), org: OrgId::new("acme"), role: Role::Member, scopes: vec![scope("repo:a/memory", true, false)], parent: None };
        let (state, org) = state_with(&[node("a", "n1")], ro);
        let engine = state.engines.get(&org).unwrap();
        let before = engine.node_count().unwrap();
        let res = merge_diff(State(state.clone()), dev("u"), Path(org.as_str().to_string()), Bytes::from(signed_diff("a"))).await;
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
        assert_eq!(engine.node_count().unwrap(), before, "denied merge must not land");
    }

    /// merge_diff: an empty diff is rejected (no unchecked-edge no-op).
    #[tokio::test]
    async fn merge_diff_rejects_an_empty_diff() {
        let (state, org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let empty = MemoryDiff::new("u".to_string(), 1).to_json().unwrap();
        let res = merge_diff(State(state), dev("u"), Path(org.as_str().to_string()), Bytes::from(empty)).await;
        assert_eq!(res.unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    const MCP_SECRET: &str = "test-mcp-secret";
    fn connect_token(sub: &str, org: &str) -> String {
        let exp = now_ms() / 1000 + 900;
        let claims = json!({ "sub": sub, "org": org, "exp": exp });
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(MCP_SECRET.as_bytes()),
        )
        .unwrap()
    }
    fn bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", format!("Bearer {token}").parse().unwrap());
        h
    }
    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// G-7: a valid connect token drives the MCP tool surface (tools/list + a recall).
    #[tokio::test]
    async fn mcp_http_serves_tools_for_a_valid_connect_token() {
        std::env::set_var("MEM_CONNECT_SECRET", MCP_SECRET);
        let (state, _org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let token = connect_token("u", "acme");

        // tools/list
        let list = mcp_http(State(state.clone()), bearer(&token), Path("u".to_string()),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string()).await;
        assert_eq!(list.status(), StatusCode::OK);
        let v = body_json(list).await;
        assert!(v["result"]["tools"].is_array(), "tools/list returns a tool array: {v}");

        // a real tool call: recall the tenant we seeded → goes through authz + the engine.
        let call = mcp_http(State(state), bearer(&token), Path("u".to_string()),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "memory.recall", "arguments": { "repo": "a" } } }).to_string()).await;
        assert_eq!(call.status(), StatusCode::OK);
        let v2 = body_json(call).await;
        assert!(v2.get("result").is_some(), "recall returns a result: {v2}");
    }

    #[tokio::test]
    async fn mcp_http_rejects_a_bad_token_and_a_subject_mismatch() {
        std::env::set_var("MEM_CONNECT_SECRET", MCP_SECRET);
        let (state, _org) = state_with(&[node("a", "n1")], member(Role::OrgOwner, vec![]));
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();

        // garbage bearer → 401
        let bad = mcp_http(State(state.clone()), bearer("not.a.jwt"), Path("u".to_string()), body.clone()).await;
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);

        // valid token for "u" replayed on someone else's endpoint path → 403
        let token = connect_token("u", "acme");
        let mism = mcp_http(State(state), bearer(&token), Path("intruder".to_string()), body).await;
        assert_eq!(mism.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn scene_includes_all_tenants_for_an_owner_and_caches() {
        let (state, org) = state_with(&[node("a", "n1"), node("b", "n2")], member(Role::OrgOwner, vec![]));
        let scene = build_scene_cached(&state, "u", &org).unwrap();
        assert_eq!(scene["node_count"], 2);
        // Second call is served from cache (same Arc value).
        let again = build_scene_cached(&state, "u", &org).unwrap();
        assert!(Arc::ptr_eq(&scene, &again), "second build must hit the cache");
    }

    #[test]
    fn scene_filters_to_readable_tenants_only() {
        // Member can read repo a only; the scene must omit repo b's node.
        let m = member(Role::Member, vec![mem_authz::ResourceScope { resource_id: "repo:a/memory".into(), can_read: true, can_write: false }]);
        let (state, org) = state_with(&[node("a", "visible"), node("b", "secret")], m);
        let scene = build_scene_cached(&state, "u", &org).unwrap();
        assert_eq!(scene["node_count"], 1, "only the readable tenant's node is in the scene");
        let repos: Vec<&str> = scene["nodes"].as_array().unwrap().iter().map(|n| n["repo"].as_str().unwrap()).collect();
        assert_eq!(repos, vec!["a"]);
    }
}

/// Merge a signed memory-diff (a session subgraph — the "git for agents" handoff).
/// Write-gated on EVERY repo the diff touches (node repos AND edge-endpoint repos,
/// FUA-MEMORIES-03), size-capped before/after parse (FUA-MEMORIES-05), and rejected
/// wholesale if any signature fails (`apply_diff` verifies first — no partial poison).
async fn merge_diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Bytes,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);

    const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
    const MAX_DIFF_NODES: usize = 10_000;
    const MAX_DIFF_EDGES: usize = 50_000;
    if body.len() > MAX_DIFF_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, format!("diff too large ({} bytes > {MAX_DIFF_BYTES} cap)", body.len())));
    }
    let text = std::str::from_utf8(&body).map_err(|_| (StatusCode::BAD_REQUEST, "diff is not utf-8".to_string()))?;
    let diff = MemoryDiff::from_json(text).map_err(|e| (StatusCode::BAD_REQUEST, format!("malformed diff: {e}")))?;
    if diff.nodes.len() > MAX_DIFF_NODES || diff.edges.len() > MAX_DIFF_EDGES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, format!("diff exceeds size cap ({} nodes, {} edges)", diff.nodes.len(), diff.edges.len())));
    }
    if diff.nodes.is_empty() && diff.edges.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty diff (no nodes, no edges)".to_string()));
    }

    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    // Authorize the repo of every node AND every edge endpoint (resolved from the
    // diff's own nodes, else from the store) — never just node repos.
    let in_diff: std::collections::BTreeMap<ContentHash, String> =
        diff.nodes.iter().map(|n| (n.compute_id(), n.repo.clone())).collect();
    let mut repos: std::collections::BTreeSet<String> = diff.nodes.iter().map(|n| n.repo.clone()).collect();
    for e in &diff.edges {
        for endpoint in [&e.from, &e.to] {
            let repo = match in_diff.get(endpoint) {
                Some(r) => r.clone(),
                None => match engine.get_node(endpoint).map_err(store_err)? {
                    Some(n) => n.repo.clone(),
                    None => return Err((StatusCode::BAD_REQUEST, format!("edge references unknown node {}", &endpoint.to_hex()[..12]))),
                },
            };
            repos.insert(repo);
        }
    }
    for repo in &repos {
        authz_write(&state, &sub, &org, repo)?;
    }
    let report = apply_diff(&engine, &diff).map_err(|e| (StatusCode::CONFLICT, format!("merge rejected: {e}")))?;
    audit(&state, MemoryEvent::Write, &sub, org.as_str(), &format!("merge_diff +{} nodes +{} edges by {}", report.nodes, report.edges, diff.author));
    if let Ok(mut c) = state.layout_cache.lock() {
        c.remove(&org);
    }
    Ok(Json(json!({ "nodes": report.nodes, "edges": report.edges, "superseded": report.superseded })))
}

// ---- Org creation + store-init (gap G-4 provisioning) ----

/// A path-safe Org id from a free-text name: lowercase, non-alphanumerics → single
/// dashes, trimmed. Bounds the result so it can never escape the data dir.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.chars().take(64).collect()
}

#[derive(Deserialize)]
struct CreateOrgBody {
    name: String,
    #[serde(default)]
    id: Option<String>,
}

/// Create a new Org: validate + reserve a unique id, **initialize its isolated
/// encrypted store** (its own RocksDB + keyring), bind the caller as the founding
/// Org Owner, and persist the control plane (G-3). Self-serve — any authenticated
/// subject may create an Org and becomes its Owner.
async fn create_org(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateOrgBody>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "org name is required".into()));
    }
    let id = body.id.map(|i| slugify(&i)).unwrap_or_else(|| slugify(&name));
    if id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "org id slug is empty after sanitization".into()));
    }
    let org_id = OrgId::new(&id);

    // Reserve the id (fail fast on a duplicate before touching the store).
    {
        let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
        if control.org(&org_id).is_some() {
            return Err((StatusCode::CONFLICT, format!("org '{id}' already exists")));
        }
    }

    let data_dir = std::env::var("MEM_GATEWAY_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let store_path = format!("{data_dir}/orgs/{id}.memdag");
    let org = Org {
        id: org_id.clone(),
        name: name.clone(),
        created_at_ms: now_ms(),
        store_path: store_path.clone(),
        status: OrgStatus::Active,
    };

    // Initialize the Org's isolated store (unless a test pre-registered an engine).
    if !state.engines.is_open(&org_id) {
        #[cfg(feature = "rocksdb")]
        {
            state.engines.open_org(&org).map_err(map_gateway_err)?;
        }
        #[cfg(not(feature = "rocksdb"))]
        {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "store backend (rocksdb) not built".to_string()));
        }
    }

    // Bind the founding Owner + persist (re-check uniqueness under the write lock).
    {
        let mut control = state.control.write().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
        if control.org(&org_id).is_some() {
            return Err((StatusCode::CONFLICT, format!("org '{id}' already exists")));
        }
        control.upsert_org(org.clone());
        control.upsert_membership(Membership {
            sub: sub.clone(),
            org: org_id.clone(),
            role: Role::OrgOwner,
            scopes: vec![],
            parent: None,
        });
    }
    persist_control(&state);
    audit(&state, MemoryEvent::Write, &sub, org_id.as_str(), &format!("create_org '{name}' (owner {sub})"));
    Ok(Json(json!({ "id": id, "name": name, "status": "Active", "store_path": store_path, "owner": sub })))
}

// ---- provisioning routes (gap G-4): the delegation tree ----
//
// Member onboarding + revocation within an Org. Onboarding is attenuation-gated
// (`can_delegate`: admin-only, role ceiling, no granting scopes you don't hold);
// revocation cascades the whole subtree (F-7). Both persist the durable control
// plane (gap G-3) and audit the change.

/// Persist the control plane after a provisioning mutation, if a durable path is
/// configured. Best-effort: a write failure is logged, not fatal to the response.
fn persist_control(state: &AppState) {
    if let Some(path) = &state.control_path {
        if let Ok(cp) = state.control.read() {
            if let Err(e) = cp.save_atomic(path) {
                eprintln!("mem-gateway: WARN — control plane persist failed: {e}");
            }
        }
    }
}

fn role_from_str(s: &str) -> Option<Role> {
    match s.to_ascii_lowercase().as_str() {
        "member" => Some(Role::Member),
        "org_admin" | "admin" | "orgadmin" => Some(Role::OrgAdmin),
        "org_owner" | "owner" | "orgowner" => Some(Role::OrgOwner),
        _ => None,
    }
}

/// The Org roster + delegation tree (Org-scoped; any member may see who's in their Org).
async fn list_members(State(state): State<AppState>, headers: HeaderMap, Path(org): Path<String>) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    // Org-boundary gate: caller must be a member.
    control.membership(&sub, &org).ok_or((StatusCode::FORBIDDEN, "not a member of org".to_string()))?;
    let members: Vec<Value> = control
        .members_of(&org)
        .into_iter()
        .map(|m| json!({
            "sub": m.sub,
            "role": format!("{:?}", m.role),
            "parent": m.parent,
            "scopes": m.scopes.iter().map(|s| json!({ "resource_id": s.resource_id, "can_read": s.can_read, "can_write": s.can_write })).collect::<Vec<_>>(),
        }))
        .collect();
    Ok(Json(json!({ "members": members })))
}

#[derive(Deserialize)]
struct AddMemberBody {
    sub: String,
    role: String,
    #[serde(default)]
    scopes: Vec<ResourceScope>,
}

/// Onboard / update a member under the caller (attenuation-gated). The new
/// membership's `parent` is the caller, recording the delegation edge.
async fn add_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<AddMemberBody>,
) -> ApiResult {
    let actor = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let role = role_from_str(&body.role).ok_or((StatusCode::BAD_REQUEST, "unknown role".to_string()))?;

    let mut control = state.control.write().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    // Org-boundary + delegation authority of the ACTOR.
    let delegator = control.membership(&actor, &org).ok_or((StatusCode::FORBIDDEN, "not a member of org".to_string()))?.clone();
    if let Err(e) = can_delegate(&delegator, role, &body.scopes) {
        drop(control);
        audit(&state, MemoryEvent::Denied, &actor, org.as_str(), &format!("add_member {} denied: {e}", body.sub));
        return Err(map_gateway_err(e));
    }
    control.upsert_membership(Membership {
        sub: body.sub.clone(),
        org: org.clone(),
        role,
        scopes: body.scopes.clone(),
        parent: Some(actor.clone()),
    });
    drop(control);
    persist_control(&state);
    audit(&state, MemoryEvent::Write, &actor, org.as_str(), &format!("add_member {} as {:?}", body.sub, role));
    Ok(Json(json!({ "sub": body.sub, "org": org.as_str(), "role": format!("{role:?}"), "parent": actor })))
}

/// Revoke a member AND the whole subtree delegated beneath them (F-7 cascade).
async fn revoke_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, target)): Path<(String, String)>,
) -> ApiResult {
    let actor = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let mut control = state.control.write().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    // Only an admin in the Org may revoke.
    let actor_m = control.membership(&actor, &org).ok_or((StatusCode::FORBIDDEN, "not a member of org".to_string()))?;
    if !actor_m.role.is_admin() {
        drop(control);
        audit(&state, MemoryEvent::Denied, &actor, org.as_str(), &format!("revoke {target} denied: not a delegator"));
        return Err((StatusCode::FORBIDDEN, "not authorized to revoke".to_string()));
    }
    let removed = control.revoke_cascade(&org, &target);
    drop(control);
    persist_control(&state);
    audit(&state, MemoryEvent::Write, &actor, org.as_str(), &format!("revoke_cascade {target} removed {}", removed.join(",")));
    Ok(Json(json!({ "revoked": removed })))
}

// ---- audit log viewer (the tamper-evident chain the write/provisioning routes feed) ----

#[derive(Deserialize)]
struct AuditQ {
    limit: Option<usize>,
}

/// The Org's audit trail + a live integrity verdict. Org-scoped: the caller must be
/// a member; an **admin** sees the whole chain, a **member** sees only their own
/// actions (planset §4.9). The chain is blake3 hash-linked — `verify_integrity`
/// re-checks the linkage on every read, so tampering surfaces here, visibly.
///
/// (The dev gateway runs one Org per process, so the shared chain is effectively
/// Org-scoped; per-Org isolation of a multi-Org chain is a documented refinement.)
async fn audit_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<AuditQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    let is_admin = {
        let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
        control.membership(&sub, &org).ok_or((StatusCode::FORBIDDEN, "not a member of org".to_string()))?.role.is_admin()
    };
    let Some(chain) = &state.audit else {
        return Ok(Json(json!({ "intact": true, "length": 0, "verified_through": 0, "records": [] })));
    };
    let guard = chain.lock().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "audit lock poisoned".to_string()))?;
    let verdict = guard.verify_integrity();
    let limit = q.limit.unwrap_or(200).min(1000);
    let records: Vec<Value> = guard
        .records()
        .iter()
        .rev()
        .filter(|r| is_admin || r.actor == sub)
        .take(limit)
        .map(|r| json!({
            "seq": r.sequence,
            "ts": r.timestamp_ms,
            "event": format!("{:?}", r.event),
            "actor": r.actor,
            "resource_id": r.resource_id,
            "detail": r.detail,
        }))
        .collect();
    Ok(Json(json!({
        "intact": verdict.is_ok(),
        "length": guard.len(),
        "verified_through": verdict.unwrap_or(0),
        "records": records,
    })))
}

// ---- ops / durability surface (WP-6.5): checkpoints + anchors ----

/// List an Org store's rolling checkpoints (`<store_path>.checkpoints/ckpt-<ms>`),
/// oldest→newest. A checkpoint dir IS a complete, independently-openable store.
fn list_checkpoints(store_path: &str) -> Vec<(String, u64)> {
    let dir = format!("{store_path}.checkpoints");
    let mut out: Vec<(String, u64)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("ckpt-").and_then(|s| s.parse::<u64>().ok()).map(|ms| (name, ms))
        })
        .collect();
    out.sort_by_key(|(_, ms)| *ms);
    out
}

/// Is the caller an admin in this Org? (ops is an Org-Owner/Admin concern.)
fn require_org_admin(state: &AppState, sub: &str, org: &OrgId) -> Result<(), (StatusCode, String)> {
    let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    let m = control.membership(sub, org).ok_or((StatusCode::FORBIDDEN, "not a member of org".to_string()))?;
    if m.role.is_admin() {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "ops requires an admin role".to_string()))
    }
}

/// Resolve an Org's on-disk store path from the control plane.
fn org_store_path(state: &AppState, org: &OrgId) -> Result<String, (StatusCode, String)> {
    let control = state.control.read().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned".to_string()))?;
    control.org(org).map(|o| o.store_path.clone()).ok_or((StatusCode::NOT_FOUND, "org not found".to_string()))
}

/// Durability snapshot for an Org: store counts, checkpoint inventory (recovery
/// points), and per-tenant anchor posture (the merkle tenant-root notarization +
/// whether the live state still matches it). Admin-gated.
async fn ops(State(state): State<AppState>, headers: HeaderMap, Path(org): Path<String>) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    require_org_admin(&state, &sub, &org)?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;

    let nodes = engine.node_count().map_err(store_err)?;
    let edges = engine.edge_count().map_err(store_err)?;

    let store_path = org_store_path(&state, &org)?;
    let ckpts = list_checkpoints(&store_path);
    let checkpoints = json!({
        "count": ckpts.len(),
        "latest_ms": ckpts.last().map(|(_, ms)| *ms),
        "dir": format!("{store_path}.checkpoints"),
    });

    // Per readable tenant: local anchor + whether the live root still matches it.
    let repos: std::collections::BTreeSet<String> = engine
        .all_nodes()
        .map_err(store_err)?
        .into_iter()
        .map(|n| n.repo)
        .filter(|r| can_read(&state, &sub, &org, r))
        .collect();
    let mut tenants: Vec<Value> = Vec::new();
    for repo in &repos {
        let anchor = mem_sync::latest_anchor(&engine, repo).ok().flatten();
        let valid = mem_sync::verify_anchor(&engine, repo).ok().flatten();
        let live_root = mem_sync::tenant_root(&engine, repo).ok().map(|h| h.to_hex());
        tenants.push(json!({
            "repo": repo,
            "anchor": anchor.map(|a| json!({
                "root": a.root, "node_count": a.node_count, "edge_count": a.edge_count, "anchored_at_ms": a.anchored_at_ms,
            })),
            "anchor_valid": valid,
            "live_root": live_root,
        }));
    }

    #[allow(unused_mut)]
    let mut chain_anchor = Value::Null;
    #[cfg(feature = "chain")]
    {
        if let Some(repo) = repos.iter().next() {
            if let Ok(Some(rec)) = mem_sync::chain::latest_chain_anchor(&engine, repo) {
                chain_anchor = json!({
                    "repo": repo,
                    "root": rec.anchor.root,
                    "block_number": rec.checkpoint.block_number,
                    "block_hash": rec.checkpoint.block_hash,
                    "chain_id": rec.checkpoint.chain_id,
                    "fetched_at_ms": rec.checkpoint.fetched_at_ms,
                });
            }
        }
    }

    Ok(Json(json!({
        "org": org.as_str(),
        "store": { "nodes": nodes, "edges": edges },
        "checkpoints": checkpoints,
        "tenants": tenants,
        "chain_anchor": chain_anchor,
    })))
}

#[derive(Deserialize)]
struct CheckpointQ {
    keep: Option<usize>,
}

/// Create a recovery checkpoint now and prune to the newest `keep` (default 3).
/// Admin-gated + audited. (A checkpoint is a complete store; restore is a deliberate
/// operator step — not exposed as a live route.)
async fn ops_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<CheckpointQ>,
) -> ApiResult {
    let sub = authn(&headers, &state)?;
    let org = OrgId::new(org);
    require_org_admin(&state, &sub, &org)?;
    let engine = state.engines.get(&org).map_err(map_gateway_err)?;
    let store_path = org_store_path(&state, &org)?;
    let keep = q.keep.unwrap_or(3).max(1);

    let dir = format!("{store_path}.checkpoints");
    std::fs::create_dir_all(&dir).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir {dir}: {e}")))?;
    let stamp = format!("ckpt-{:020}", now_ms());
    let dest = format!("{dir}/{stamp}");
    engine.checkpoint(&dest).map_err(store_err)?;
    // Prune oldest beyond `keep`.
    let mut all = list_checkpoints(&store_path);
    while all.len() > keep {
        let (name, _) = all.remove(0);
        let _ = std::fs::remove_dir_all(format!("{dir}/{name}"));
    }
    audit(&state, MemoryEvent::Write, &sub, org.as_str(), &format!("checkpoint {stamp} (keep {keep})"));
    Ok(Json(json!({ "created": stamp, "count": list_checkpoints(&store_path).len() })))
}

// ---- MCP-over-HTTP endpoint (gap G-7): BYOM ----
//
// An authenticated, non-streaming Streamable-HTTP MCP server. The bearer is the
// short-lived **connect token** minted by the app (`/connect/token`, HS256 over
// `MEM_CONNECT_SECRET`); we verify it, resolve the caller's Org + capability grant,
// and reuse `mem_mcp::MemoryMcpServer` — so every tool call (recall/search/…/assert/
// propose/confirm/merge_diff) is scoped by the grant and written to the SAME audit
// chain as the HTTP routes. JSON-RPC in the body, JSON-RPC out (202 for notifications).

#[derive(serde::Deserialize)]
struct ConnectClaims {
    sub: String,
    org: String,
}

/// Verify a connect token (HS256 / `MEM_CONNECT_SECRET`), returning its claims.
fn verify_connect_token(token: &str) -> Result<ConnectClaims, (StatusCode, String)> {
    let secret = std::env::var("MEM_CONNECT_SECRET")
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "BYOM connect is not configured (MEM_CONNECT_SECRET unset)".to_string()))?;
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_aud = false; // the connect token carries no audience
    jsonwebtoken::decode::<ConnectClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|d| d.claims)
    .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid or expired connect token".to_string()))
}

async fn mcp_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(path_sub): Path<String>,
    body: String,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Some(token) = bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let claims = match verify_connect_token(token) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    // The path segment is cosmetic; the token's subject is authoritative. Reject a
    // mismatch (a token replayed on someone else's endpoint path).
    if path_sub != claims.sub {
        return (StatusCode::FORBIDDEN, "token subject does not match endpoint").into_response();
    }
    let org = OrgId::new(claims.org);
    // Org boundary + capability grant (same derivation the HTTP routes use).
    let grant = {
        let control = match state.control.read() {
            Ok(c) => c,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "control lock poisoned").into_response(),
        };
        match control.membership(&claims.sub, &org) {
            Some(m) => derive_grant(m, now_ms(), state.grant_ttl_ms, &state.issuer_key),
            None => return (StatusCode::FORBIDDEN, "not a member of org").into_response(),
        }
    };
    let engine = match state.engines.get(&org) {
        Ok(e) => e,
        Err(e) => return map_gateway_err(e).into_response(),
    };
    // Bind a per-request MCP session to this Org's store + the caller's grant +
    // the gateway's shared asserter + audit chain (write tools available iff a
    // signing identity is configured).
    let mut server = match state.asserter.as_ref().map(|a| (**a).clone()) {
        Some(asserter) => mem_mcp::MemoryMcpServer::new_with_asserter(&engine, grant, asserter),
        None => mem_mcp::MemoryMcpServer::new(&engine, grant),
    };
    if let Some(emb) = &state.embedder {
        server = server.with_query_embedder(emb.clone());
    }
    if let Some(audit) = &state.audit {
        server = server.with_audit_chain(audit.clone());
    }
    match server.handle_line(body.trim()) {
        Some(resp) => ([(axum::http::header::CONTENT_TYPE, "application/json")], resp).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Build the router. Mounted under `/api`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/orgs", get(list_orgs).post(create_org))
        .route("/api/orgs/:org/tenants", get(list_tenants))
        .route("/api/orgs/:org/tenants/:tenant/recall", get(recall))
        .route("/api/orgs/:org/tenants/:tenant/search", get(search))
        .route("/api/orgs/:org/tenants/:tenant/as_of", get(as_of))
        .route("/api/orgs/:org/nodes/:id", get(node))
        .route("/api/orgs/:org/nodes/:id/verify", get(verify))
        .route("/api/orgs/:org/nodes/:id/neighbors", get(neighbors))
        .route("/api/orgs/:org/analogy", get(analogy))
        .route("/api/orgs/:org/layout", get(layout))
        // write routes (gap G-1)
        .route("/api/orgs/:org/tenants/:tenant/assert", post(assert))
        .route("/api/orgs/:org/edges/propose", post(propose_edge))
        .route("/api/orgs/:org/edges/confirm", post(confirm_edge))
        .route("/api/orgs/:org/merge_diff", post(merge_diff))
        // provisioning routes (gap G-4): the delegation tree
        .route("/api/orgs/:org/members", get(list_members).post(add_member))
        .route("/api/orgs/:org/members/:sub", delete(revoke_member))
        // audit log viewer
        .route("/api/orgs/:org/audit", get(audit_log))
        // ops / durability (WP-6.5)
        .route("/api/orgs/:org/ops", get(ops))
        .route("/api/orgs/:org/ops/checkpoint", post(ops_checkpoint))
        // MCP-over-HTTP endpoint (gap G-7): BYOM. Auth is the connect token, not OIDC.
        .route("/mcp/u/:sub", post(mcp_http))
        .with_state(state)
}
