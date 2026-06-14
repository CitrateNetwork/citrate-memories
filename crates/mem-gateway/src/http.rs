//! The axum HTTP/JSON read API + 3D-layout endpoint (feature `server`).
//!
//! Every route is **Org-scoped** and passes through the same gate as the core:
//! authenticate the subject → check the **Org boundary** → check the repo-tenant
//! scope (`authorize`). Auth **fails closed**: with no OIDC verifier configured and
//! dev-auth off, every request is refused. Dev-auth (`x-dev-sub` header, behind an
//! explicit opt-in) is for the prototype + local dev only.

use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use mem_authz::Op;
use mem_index::Embedder;
use mem_query::Recall;

use crate::authz::authorize;
use crate::layout::{build_scene, SceneEdge};
use crate::org::{ControlPlane, OrgId};
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
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Authenticate the caller → their OIDC subject. Fails closed.
fn authn(headers: &HeaderMap, state: &AppState) -> Result<String, (StatusCode, String)> {
    if state.allow_dev_auth {
        if let Some(sub) = headers.get("x-dev-sub").and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()) {
            return Ok(sub.to_string());
        }
        return Err((StatusCode::UNAUTHORIZED, "dev-auth on: provide x-dev-sub".into()));
    }
    // Fail closed: real OIDC bearer verification is the M1 wiring (citrate-identity
    // JWKS). Until configured, refuse — never default-open.
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

/// Build the router. Mounted under `/api`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/orgs", get(list_orgs))
        .route("/api/orgs/:org/tenants", get(list_tenants))
        .route("/api/orgs/:org/tenants/:tenant/recall", get(recall))
        .route("/api/orgs/:org/tenants/:tenant/search", get(search))
        .route("/api/orgs/:org/tenants/:tenant/as_of", get(as_of))
        .route("/api/orgs/:org/nodes/:id", get(node))
        .route("/api/orgs/:org/nodes/:id/verify", get(verify))
        .route("/api/orgs/:org/nodes/:id/neighbors", get(neighbors))
        .route("/api/orgs/:org/analogy", get(analogy))
        .route("/api/orgs/:org/layout", get(layout))
        .with_state(state)
}
