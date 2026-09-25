//! PBA R2 regression tests (2026-09-24 pre-bounty adversarial audit, lane L6b).
//!
//! These are the audit PoCs (`evidence/memories/l6b_gateway_poc.rs`) turned into
//! regression tests with the assertions INVERTED: each drives the same private
//! axum handler the router binds (`verify`, `neighbors`, `byom`), with dev-auth
//! (`x-dev-sub`) standing in for OIDC — `authenticate()` sits upstream of every
//! defect here, so the entry point is faithful.
//!
//! Mounted from `http.rs` as a child module so it can reach the private handlers.

use super::*;
use crate::auth::signing_key_from_seed;
use crate::control::{Membership, Org, Role, Scope};
use axum::http::HeaderValue;
use mem_core::EdgeKind;
use mem_store::kv::InMemoryKv;

const ORG: &str = "org1";

fn app_with(store: MemoryDagStore<MemoryNode>, members: Vec<Membership>, connect_secret: Option<&str>) -> AppState {
    let control = Control {
        orgs: vec![Org {
            id: ORG.into(),
            name: ORG.into(),
            created_at_ms: 1,
            store_path: "mem".into(),
            status: OrgStatus::Active,
        }],
        memberships: members,
    };
    AppState {
        store: Arc::new(store),
        embedder: None,
        index_cache: Arc::new(TenantIndexCache::new()),
        write_gate: Arc::new(Mutex::new(())),
        audit: Arc::new(Mutex::new(AuditChain::new())),
        signing_key: signing_key_from_seed("l6b-test-seed"),
        control: Arc::new(RwLock::new(control)),
        org_id: Arc::new(ORG.into()),
        store_path: Arc::new("mem".into()),
        chain_rpc: None,
        issuer: Arc::new("l6b".into()),
        oidc: None,
        connect_secret: connect_secret.map(|s| Arc::new(s.to_string())),
        allow_dev_auth: true,
        layout_cache: Arc::new(Mutex::new(None)),
        ingest_queue: Arc::new(Mutex::new(VecDeque::new())),
    }
}

fn member(sub: &str, tenants: &[&str], write: bool) -> Membership {
    Membership {
        sub: sub.into(),
        org: ORG.into(),
        role: Role::Member,
        scopes: tenants
            .iter()
            .map(|t| Scope { resource_id: (*t).into(), can_read: true, can_write: write })
            .collect(),
        parent: None,
    }
}

fn dev(sub: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("x-dev-sub", HeaderValue::from_str(sub).unwrap());
    h
}

fn q(pairs: &[(&str, &str)]) -> Query<HashMap<String, String>> {
    Query(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

/// Store with a tenant-a node linked (confirmed AnalogousTo) to a secret tenant-b node.
fn cross_tenant_store() -> (MemoryDagStore<MemoryNode>, mem_core::ContentHash) {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let owner = Asserter::for_principal(&gw, "owner");
    let a = owner.assert_node("tenant-a", NodeKind::Rationale, "public note in tenant-a", 1);
    let b = owner.assert_node("tenant-b", NodeKind::Rationale, "SECRET-B: acquisition target is Foo Corp", 1);
    let a_id = store.put_node(&a).unwrap();
    let b_id = store.put_node(&b).unwrap();
    store.add_edge(&owner.assert_edge(a_id, b_id, EdgeKind::AnalogousTo, 2)).unwrap();
    (store, a_id)
}

// ---------------------------------------------------------------------------
// PBA-L6b-001 — /verify must not leak cross-tenant neighbours (MEM-B-007 residual)
// ---------------------------------------------------------------------------

/// PBA-L6b-001 (inverted PoC `l6b_verify_leaks_cross_tenant_neighbor_content`):
/// a Member scoped to tenant-a calls `/verify` on her own node; the tenant-b
/// neighbour reached over a cross-DAG edge must be neither shown nor counted.
#[tokio::test]
async fn pba_l6b_001_verify_does_not_leak_cross_tenant_neighbor() {
    let (store, a_id) = cross_tenant_store();
    let app = app_with(store, vec![member("mallory", &["tenant-a"], false)], None);

    // Precondition: tenant-b is forbidden to Mallory directly.
    let direct = recall(State(app.clone()), Path(ORG.into()), q(&[("repo", "tenant-b")]), dev("mallory")).await;
    assert_eq!(direct.err().map(|e| e.status), Some(StatusCode::FORBIDDEN), "precondition: tenant-b is forbidden");

    let prefix = a_id.to_hex()[..12].to_string();
    let v = match verify(State(app.clone()), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("mallory")).await {
        Ok(j) => j.0,
        Err(e) => panic!("verify on an own-tenant node must succeed, got {}", e.status),
    };
    let body = v.to_string();
    assert!(!body.contains("SECRET-B"), "PBA-L6b-001: /verify leaked forbidden tenant-b content: {body}");
    assert!(!body.contains("tenant-b"), "PBA-L6b-001: /verify leaked the forbidden tenant's existence: {body}");
    assert_eq!(v["neighbors"].as_array().map(|a| a.len()), Some(0), "no readable neighbours");
    assert_eq!(v["verdict"], "current");

    // /neighbors (the control) must agree.
    let nb = match neighbors(State(app.clone()), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("mallory")).await {
        Ok(j) => j.0,
        Err(e) => panic!("neighbors must succeed, got {}", e.status),
    };
    assert!(!nb.to_string().contains("SECRET-B"), "/neighbors filters too");
}

/// PBA-L6b-001 no-false-negative: a caller who CAN read both tenants still sees
/// the cross-tenant neighbour through `/verify` (the filter is grant-aware, not a
/// blanket same-tenant cut).
#[tokio::test]
async fn pba_l6b_001_verify_shows_cross_tenant_neighbor_to_authorized_reader() {
    let (store, a_id) = cross_tenant_store();
    let app = app_with(store, vec![member("alice", &["tenant-a", "tenant-b"], false)], None);
    let prefix = a_id.to_hex()[..12].to_string();
    let v = match verify(State(app), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("alice")).await {
        Ok(j) => j.0,
        Err(e) => panic!("verify must succeed, got {}", e.status),
    };
    let body = v.to_string();
    assert!(body.contains("SECRET-B"), "authorized reader must still see the tenant-b neighbour: {body}");
    assert_eq!(v["neighbors"].as_array().map(|a| a.len()), Some(1));
}

/// PBA-L6b-001 verdict channel: a Contradicts edge from a node in a tenant the
/// caller cannot read must not flip the caller-visible verdict (that would leak
/// the foreign edge's existence one bit at a time).
#[tokio::test]
async fn pba_l6b_001_verify_verdict_ignores_unreadable_neighbors() {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let owner = Asserter::for_principal(&gw, "owner");
    let a = owner.assert_node("tenant-a", NodeKind::Rationale, "claim in a", 1);
    let b = owner.assert_node("tenant-b", NodeKind::Rationale, "secret rebuttal in b", 1);
    let a_id = store.put_node(&a).unwrap();
    let b_id = store.put_node(&b).unwrap();
    store.add_edge(&owner.assert_edge(b_id, a_id, EdgeKind::Contradicts, 2)).unwrap();
    let prefix = a_id.to_hex()[..12].to_string();

    let app = app_with(store, vec![member("mallory", &["tenant-a"], false), member("alice", &["tenant-a", "tenant-b"], false)], None);
    let as_mallory = verify(State(app.clone()), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("mallory"))
        .await
        .ok()
        .map(|j| j.0)
        .unwrap_or_default();
    assert_eq!(as_mallory["verdict"], "current", "unreadable contradiction must not leak via the verdict");
    let as_alice = verify(State(app), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("alice"))
        .await
        .ok()
        .map(|j| j.0)
        .unwrap_or_default();
    assert_eq!(as_alice["verdict"], "contradicted", "a reader of both tenants sees the contradiction");
}

/// PBA-L6b-001 class tripwire: no caller-facing surface may fetch neighbours with
/// the tenant-blind `Recall::neighbors`. Every HTTP handler goes through
/// `readable_neighbors` (→ `Recall::neighbors_readable`), and so does MCP
/// `memory.neighbors`. A new `.neighbors(&…)` call in either file fails here.
#[test]
fn pba_l6b_001_tripwire_no_tenant_blind_neighbors_on_caller_surfaces() {
    let needle = concat!(".neigh", "bors(&");
    for (name, src) in [
        ("mem-gateway/src/http.rs", include_str!("http.rs")),
        ("mem-mcp/src/lib.rs", include_str!("../../mem-mcp/src/lib.rs")),
    ] {
        let hits: Vec<(usize, &str)> =
            src.lines().enumerate().filter(|(_, l)| l.contains(needle)).map(|(i, l)| (i + 1, l.trim())).collect();
        assert!(
            hits.is_empty(),
            "PBA-L6b-001: {name} calls tenant-blind Recall::neighbors; use neighbors_readable: {hits:?}"
        );
    }
    let http = include_str!("http.rs");
    let verify_fn = &http[http.find("async fn verify(").expect("verify handler")..];
    let verify_fn = &verify_fn[..verify_fn.find("\nasync fn ").unwrap_or(verify_fn.len())];
    assert!(
        verify_fn.contains("gate_with_grant(") && verify_fn.contains("readable_neighbors("),
        "PBA-L6b-001: /verify must hold the caller's grant and filter neighbours through readable_neighbors"
    );
}
