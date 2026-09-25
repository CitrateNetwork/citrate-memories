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
        byom_limits: Arc::new(ByomLimits::default()),
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

// ---------------------------------------------------------------------------
// PBA-L6b-002 — BYOM merge_diff must not overwrite another principal's node
// ---------------------------------------------------------------------------

use mem_core::{BelnapValue, Status, VersionedVector};

fn byom_headers(secret: &str, sub: &str, scope: &str) -> HeaderMap {
    let now_s = (now_ms() / 1000) as usize;
    let tok = mint_connect_token(secret, ORG, sub, Some(scope), &[], now_s, 900).unwrap();
    let mut h = HeaderMap::new();
    h.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {tok}")).unwrap());
    h
}

async fn byom_call(app: &AppState, sub: &str, h: HeaderMap, rpc: Value) -> String {
    let resp = match byom(State(app.clone()), Path(sub.into()), h, rpc.to_string()).await {
        Ok(r) => r,
        Err(e) => panic!("byom returned HTTP {}", e.status),
    };
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

fn merge_rpc(diff: &mem_assert::MemoryDiff) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"memory.merge_diff","arguments":{"diff": diff.to_json().unwrap()}}})
}

/// PBA-L6b-002 (inverted PoC `l6b_byom_merge_diff_overwrites_victims_signed_node`):
/// Mallory (Member write on tenant-a, connect token `read,propose`) re-submits
/// Bob's signed ADR with the unsigned advisory fields rewritten. The merge must be
/// refused and Bob's stored node must be byte-for-byte unchanged.
#[tokio::test]
async fn pba_l6b_002_byom_merge_diff_cannot_overwrite_victims_signed_node() {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let bob = Asserter::for_principal(&gw, "bob");
    let victim = bob.assert_node_with(
        "tenant-a",
        NodeKind::Adr,
        "ADR: we will require 2-of-3 approvals for treasury moves",
        1_000,
        1_000,
        Some(VersionedVector { model: "hash".into(), data: vec![0.5; 4] }),
    );
    let vid = store.put_node(&victim).unwrap();
    let before = store.get_node(&vid).unwrap().unwrap();

    let secret = "connect-secret";
    let app = app_with(
        store,
        vec![member("bob", &["tenant-a"], true), member("mallory", &["tenant-a"], true)],
        Some(secret),
    );

    let mut forged = victim.clone();
    forged.status = Status::Archived;
    forged.valid_to = Some(1);
    forged.valid_from = 0;
    forged.confidence = vec![BelnapValue::False];
    forged.embedding = Some(VersionedVector { model: "hash".into(), data: vec![0.0; 4] });
    let diff = mem_assert::MemoryDiff { author: "bob".into(), created_at_ms: 1, nodes: vec![forged], edges: vec![] };
    let body = byom_call(&app, "mallory", byom_headers(secret, "mallory", "read,propose"), merge_rpc(&diff)).await;

    assert!(body.contains("\"isError\":true"), "PBA-L6b-002: the overwrite must be refused, got {body}");
    let after = app.store.get_node(&vid).unwrap().unwrap();
    assert_eq!(after, before, "PBA-L6b-002: mallory rewrote bob's signed node");
    assert_eq!(after.status, Status::Active);
    assert_eq!(after.valid_to, None);
}

/// PBA-L6b-002 edge half: a quarantined proposal may not be laundered into a
/// load-bearing edge by re-submitting it with `quarantined=false` in a diff (the
/// edge signature covers only from‖to‖kind). Promotion goes through
/// `memory.confirm_edge`, never a diff.
#[tokio::test]
async fn pba_l6b_002_byom_merge_diff_cannot_launder_quarantined_edge() {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let bob = Asserter::for_principal(&gw, "bob");
    let a = bob.assert_node("tenant-a", NodeKind::Rationale, "old claim", 1);
    let b = bob.assert_node("tenant-a", NodeKind::Rationale, "new claim", 2);
    let a_id = store.put_node(&a).unwrap();
    let b_id = store.put_node(&b).unwrap();
    let proposal = bob.propose_edge(b_id, a_id, EdgeKind::AnalogousTo, mem_core::EdgeMethod::Nlp, None, 3);
    assert!(proposal.quarantined);
    store.add_edge(&proposal).unwrap();

    let secret = "connect-secret";
    let app = app_with(store, vec![member("mallory", &["tenant-a"], true)], Some(secret));
    let mut laundered = proposal.clone();
    laundered.quarantined = false;
    let diff = mem_assert::MemoryDiff { author: "bob".into(), created_at_ms: 1, nodes: vec![], edges: vec![laundered] };
    let body = byom_call(&app, "mallory", byom_headers(secret, "mallory", "read,propose"), merge_rpc(&diff)).await;

    assert!(body.contains("\"isError\":true"), "PBA-L6b-002: quarantine laundering must be refused, got {body}");
    let stored = app.store.out_edges(&b_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert!(stored[0].quarantined, "PBA-L6b-002: the proposal was promoted by a diff");
}

/// PBA-L6b-002 no-regression: re-merging an IDENTICAL copy of another principal's
/// node (the normal "git for agents" handoff/idempotent re-merge) still succeeds.
#[tokio::test]
async fn pba_l6b_002_byom_identical_remerge_still_succeeds() {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let bob = Asserter::for_principal(&gw, "bob");
    let n = bob.assert_node("tenant-a", NodeKind::Rationale, "bob's handoff note", 1);
    store.put_node(&n).unwrap();
    let fresh = bob.assert_node("tenant-a", NodeKind::Rationale, "bob's second note", 2);

    let secret = "connect-secret";
    let app = app_with(store, vec![member("mallory", &["tenant-a"], true)], Some(secret));
    let diff = mem_assert::MemoryDiff { author: "bob".into(), created_at_ms: 1, nodes: vec![n, fresh.clone()], edges: vec![] };
    let body = byom_call(&app, "mallory", byom_headers(secret, "mallory", "read,propose"), merge_rpc(&diff)).await;
    assert!(body.contains("\"isError\":false"), "an identical re-merge + new node must be accepted, got {body}");
    assert!(app.store.get_node(&fresh.compute_id()).unwrap().is_some(), "the new node landed");
}


// ---------------------------------------------------------------------------
// PBA-L6b-017 — prefix resolution must not be a cross-tenant existence oracle
// ---------------------------------------------------------------------------

/// PBA-L6b-017 (inverted PoC `l6b_prefix_ambiguity_is_cross_tenant_existence_oracle`):
/// Mallory grinds an own-tenant node sharing a 4-hex prefix with a guessed id in a
/// tenant she cannot read. With tenant-scoped resolution the response is the SAME
/// (200, her node) whether or not the foreign target exists.
#[tokio::test]
async fn pba_l6b_017_prefix_resolution_is_not_a_cross_tenant_oracle() {
    let gw = signing_key_from_seed("l6b-test-seed");
    let owner = Asserter::for_principal(&gw, "owner");
    let target = owner.assert_node("tenant-b", NodeKind::Rationale, "we are laying off the infra team", 1);
    let target_hex = target.compute_id().to_hex();
    let mallory = Asserter::for_principal(&gw, "mallory");
    let mut i = 0u64;
    let own = loop {
        let n = mallory.assert_node("tenant-a", NodeKind::Rationale, &format!("note {i}"), 1);
        if n.compute_id().to_hex()[..4] == target_hex[..4] {
            break n;
        }
        i += 1;
    };
    let own_hex = own.compute_id().to_hex();
    let prefix = own_hex[..4].to_string();

    let probe = |with_target: bool| {
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        store.put_node(&own).unwrap();
        if with_target {
            store.put_node(&target).unwrap();
        }
        app_with(store, vec![member("mallory", &["tenant-a"], false)], None)
    };
    for with_target in [false, true] {
        let v = verify(State(probe(with_target)), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("mallory")).await;
        match v {
            Ok(j) => assert_eq!(j.0["id"], own_hex.as_str(), "resolves to her own node (target present={with_target})"),
            Err(e) => panic!("PBA-L6b-017: response differs by foreign existence (present={with_target}) -> {}", e.status),
        }
        let n = neighbors(State(probe(with_target)), Path(ORG.into()), q(&[("repo", "tenant-a"), ("id", &prefix)]), dev("mallory")).await;
        assert!(n.is_ok(), "PBA-L6b-017: /neighbors differs by foreign existence (present={with_target})");
    }
}

/// PBA-L6b-017 class tripwire: caller-facing surfaces never use the tenant-blind
/// `Recall::resolve_prefix` (ambiguity across tenants = existence oracle).
#[test]
fn pba_l6b_017_tripwire_no_tenant_blind_prefix_resolution_on_caller_surfaces() {
    let needle = concat!(".resolve_", "prefix(");
    for (name, src) in [
        ("mem-gateway/src/http.rs", include_str!("http.rs")),
        ("mem-mcp/src/lib.rs", include_str!("../../mem-mcp/src/lib.rs")),
    ] {
        let hits: Vec<(usize, &str)> =
            src.lines().enumerate().filter(|(_, l)| l.contains(needle)).map(|(i, l)| (i + 1, l.trim())).collect();
        assert!(hits.is_empty(), "PBA-L6b-017: {name} uses tenant-blind resolve_prefix: {hits:?}");
    }
}

// ---------------------------------------------------------------------------
// PBA-L6b-018 — BYOM request work is bounded (lines/request, per-principal rate)
// ---------------------------------------------------------------------------

fn recall_line(i: usize) -> String {
    json!({"jsonrpc":"2.0","id":i,"method":"tools/call",
        "params":{"name":"memory.recall","arguments":{"repo":"tenant-a"}}})
    .to_string()
}

/// PBA-L6b-018: a single BYOM POST carrying more than `BYOM_MAX_LINES` JSON-RPC
/// calls is refused up front (413) instead of running thousands of full-store
/// scans on a runtime worker.
#[tokio::test]
async fn pba_l6b_018_byom_refuses_too_many_lines_per_request() {
    let (store, _) = cross_tenant_store();
    let secret = "connect-secret";
    let app = app_with(store, vec![member("mallory", &["tenant-a"], false)], Some(secret));
    let body: Vec<String> = (0..1000).map(recall_line).collect();
    let r = byom(State(app.clone()), Path("mallory".into()), byom_headers(secret, "mallory", "read"), body.join("\n")).await;
    assert_eq!(r.err().map(|e| e.status), Some(StatusCode::PAYLOAD_TOO_LARGE), "PBA-L6b-018: 1000 calls in one request ran");
}

/// PBA-L6b-018 no-false-negative: a normal multi-call request still works.
#[tokio::test]
async fn pba_l6b_018_byom_small_batch_still_served() {
    let (store, _) = cross_tenant_store();
    let secret = "connect-secret";
    let app = app_with(store, vec![member("mallory", &["tenant-a"], false)], Some(secret));
    let body: Vec<String> = (0..4).map(recall_line).collect();
    let r = byom(State(app.clone()), Path("mallory".into()), byom_headers(secret, "mallory", "read"), body.join("\n")).await;
    let resp = match r {
        Ok(r) => r,
        Err(e) => panic!("small batch refused: {}", e.status),
    };
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes).lines().count(), 4, "one response per call");
}

/// PBA-L6b-018: a principal that keeps sending full batches is rate-limited
/// (429) — the budget is per principal, so another member is unaffected.
#[tokio::test]
async fn pba_l6b_018_byom_per_principal_rate_limit() {
    let (store, _) = cross_tenant_store();
    let secret = "connect-secret";
    let app = app_with(
        store,
        vec![member("mallory", &["tenant-a"], false), member("alice", &["tenant-a"], false)],
        Some(secret),
    );
    let batch: Vec<String> = (0..8).map(recall_line).collect();
    let batch = batch.join("\n");
    let mut limited = false;
    for _ in 0..64 {
        let r = byom(State(app.clone()), Path("mallory".into()), byom_headers(secret, "mallory", "read"), batch.clone()).await;
        if let Err(e) = r {
            assert_eq!(e.status, StatusCode::TOO_MANY_REQUESTS);
            limited = true;
            break;
        }
    }
    assert!(limited, "PBA-L6b-018: 512 calls in a burst were never rate-limited");
    let other = byom(State(app.clone()), Path("alice".into()), byom_headers(secret, "alice", "read"), recall_line(1)).await;
    assert!(other.is_ok(), "another principal keeps its own budget");
}

/// PBA-L6b-018 tripwire: the BYOM handler must keep its line cap, per-principal
/// budget, and off-runtime (spawn_blocking) execution.
#[test]
fn pba_l6b_018_tripwire_byom_work_is_bounded() {
    let http = include_str!("http.rs");
    let f = &http[http.find("async fn byom(").expect("byom handler")..];
    let f = &f[..f.find("\n}\n").unwrap_or(f.len())];
    for must in ["BYOM_MAX_LINES", "try_spend(", "acquire_owned()", "spawn_blocking("] {
        assert!(f.contains(must), "PBA-L6b-018: byom lost `{must}`");
    }
    assert!(!f.contains("for line in body.lines()"), "PBA-L6b-018: byom runs raw body lines inline again");
}

// ---------------------------------------------------------------------------
// PBA-L3c-032 (gateway half) — BYOM connect tokens are org-bound
// ---------------------------------------------------------------------------

fn hs256(secret: &str, claims: Value) -> HeaderMap {
    let tok = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    let mut h = HeaderMap::new();
    h.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {tok}")).unwrap());
    h
}

/// PBA-L3c-032: a connect token minted for a DIFFERENT org (the webapp puts the
/// org in the token, the gateway used to ignore it) must be refused here, and a
/// token with no org claim at all is refused too (fail closed).
#[tokio::test]
async fn pba_l3c_032_byom_refuses_token_for_another_org() {
    let (store, _) = cross_tenant_store();
    let secret = "connect-secret";
    let app = app_with(store, vec![member("mallory", &["tenant-a"], false)], Some(secret));
    let exp = (now_ms() / 1000) as usize + 600;
    let other = hs256(secret, json!({"sub": "mallory", "exp": exp, "scope": "read", "org": "some-other-org"}));
    let r = byom(State(app.clone()), Path("mallory".into()), other, recall_line(1)).await;
    assert_eq!(r.err().map(|e| e.status), Some(StatusCode::FORBIDDEN), "PBA-L3c-032: foreign-org token accepted");
    let none = hs256(secret, json!({"sub": "mallory", "exp": exp, "scope": "read"}));
    let r = byom(State(app.clone()), Path("mallory".into()), none, recall_line(1)).await;
    assert_eq!(r.err().map(|e| e.status), Some(StatusCode::FORBIDDEN), "PBA-L3c-032: org-less token accepted");
    let ok = hs256(secret, json!({"sub": "mallory", "exp": exp, "scope": "read", "org": ORG}));
    assert!(byom(State(app), Path("mallory".into()), ok, recall_line(1)).await.is_ok(), "own-org token works");
}

/// PBA-L6b-018 mutation-hardening: the line cap is inclusive (exactly
/// BYOM_MAX_LINES calls are served).
#[tokio::test]
async fn pba_l6b_018_byom_line_cap_is_inclusive() {
    let (store, _) = cross_tenant_store();
    let secret = "connect-secret";
    let app = app_with(store, vec![member("mallory", &["tenant-a"], false)], Some(secret));
    let body: Vec<String> = (0..BYOM_MAX_LINES).map(recall_line).collect();
    let r = byom(State(app.clone()), Path("mallory".into()), byom_headers(secret, "mallory", "read"), body.join("\n")).await;
    assert!(r.is_ok(), "exactly BYOM_MAX_LINES calls must be served");
    let body: Vec<String> = (0..=BYOM_MAX_LINES).map(recall_line).collect();
    let r = byom(State(app), Path("mallory".into()), byom_headers(secret, "mallory", "read"), body.join("\n")).await;
    assert_eq!(r.err().map(|e| e.status), Some(StatusCode::PAYLOAD_TOO_LARGE));
}

/// PBA-L6b-018 mutation-hardening: token-bucket arithmetic (burst inclusive,
/// exact refill rate, spend subtracts, cap at burst, per-principal buckets).
#[test]
fn pba_l6b_018_try_spend_bucket_math() {
    let l = ByomLimits::default();
    let t0 = 1_000_000u64;
    assert!(l.try_spend("a", BYOM_BURST_CALLS as usize, t0), "a full burst is allowed (inclusive)");
    assert!(!l.try_spend("a", 1, t0), "bucket empty");
    // 500 ms at 2 calls/s refills exactly one call.
    assert!(l.try_spend("a", 1, t0 + 500), "one call refilled after 500 ms");
    assert!(!l.try_spend("a", 1, t0 + 500), "…and only one");
    // A failed spend spends nothing.
    assert!(!l.try_spend("a", 2, t0 + 1_000), "only 1 token after another 500 ms");
    assert!(l.try_spend("a", 1, t0 + 1_000), "the failed spend left the token in place");
    // Refill caps at the burst.
    assert!(!l.try_spend("a", BYOM_BURST_CALLS as usize + 1, t0 + 10_000_000), "never more than a burst");
    assert!(l.try_spend("a", BYOM_BURST_CALLS as usize, t0 + 10_000_000));
    // Buckets are per principal.
    assert!(l.try_spend("b", BYOM_BURST_CALLS as usize, t0));
}

// ---------------------------------------------------------------------------
// R2 verifier follow-ups (verify.json, 2026-09-25)
// ---------------------------------------------------------------------------

/// PBA-L6b-002 follow-up (verifier probe `v_l6b002_supersedes_edge_bypass`,
/// inverted): a non-author may not retire another principal's node by adding her
/// own node plus a NEW signed Supersedes edge (backdated to t=1).
#[tokio::test]
async fn pba_l6b_002_byom_cannot_supersede_another_principals_node() {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let bob = Asserter::for_principal(&gw, "bob");
    let victim = bob.assert_node("tenant-a", NodeKind::Adr, "ADR: 2-of-3 approvals for treasury", 1_000);
    let vid = store.put_node(&victim).unwrap();
    let secret = "connect-secret";
    let app = app_with(store, vec![member("bob", &["tenant-a"], true), member("mallory", &["tenant-a"], true)], Some(secret));
    let mal = Asserter::for_principal(&gw, "mallory");
    let mine = mal.assert_node("tenant-a", NodeKind::Rationale, "throwaway", 2_000);
    let e = mal.assert_edge(mine.compute_id(), vid, EdgeKind::Supersedes, 1);
    let diff = mem_assert::MemoryDiff { author: mal.pubkey_hex().into(), created_at_ms: 1, nodes: vec![mine.clone()], edges: vec![e] };
    let body = byom_call(&app, "mallory", byom_headers(secret, "mallory", "read,propose"), merge_rpc(&diff)).await;
    assert!(body.contains("\"isError\":true"), "PBA-L6b-002: cross-author supersession accepted: {body}");
    let after = app.store.get_node(&vid).unwrap().unwrap();
    assert_eq!((after.status, after.valid_to), (Status::Active, None), "PBA-L6b-002: bob's node was retired/backdated");
    assert!(app.store.get_node(&mine.compute_id()).unwrap().is_none(), "refused diff writes nothing");

    // No regression: bob supersedes his OWN node through BYOM.
    let newer = bob.assert_node("tenant-a", NodeKind::Adr, "ADR v2: 3-of-5", 3_000);
    let e2 = bob.assert_edge(newer.compute_id(), vid, EdgeKind::Supersedes, 3_000);
    let d2 = mem_assert::MemoryDiff { author: bob.pubkey_hex().into(), created_at_ms: 3, nodes: vec![newer], edges: vec![e2] };
    let body = byom_call(&app, "bob", byom_headers(secret, "bob", "read,propose"), merge_rpc(&d2)).await;
    assert!(body.contains("\"isError\":false"), "author supersession must work: {body}");
    let after = app.store.get_node(&vid).unwrap().unwrap();
    assert_eq!((after.status, after.valid_to), (Status::Superseded, Some(3_000)));
}

/// PBA-L6b-002 follow-up (verifier probe `v_l6b002_frontrun_new_node_forged_fields`,
/// inverted): a copy of bob's signed node that is not yet stored may not be
/// inserted by someone else with forged advisory state.
#[tokio::test]
async fn pba_l6b_002_byom_cannot_front_run_new_node_with_forged_fields() {
    let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    let gw = signing_key_from_seed("l6b-test-seed");
    let bob = Asserter::for_principal(&gw, "bob");
    let victim = bob.assert_node("tenant-a", NodeKind::Adr, "ADR: bob's pending handoff", 1_000);
    let vid = victim.compute_id();
    let secret = "connect-secret";
    let app = app_with(store, vec![member("bob", &["tenant-a"], true), member("mallory", &["tenant-a"], true)], Some(secret));
    let mut forged = victim.clone();
    forged.status = Status::Archived;
    forged.valid_to = Some(1);
    forged.confidence = vec![BelnapValue::False];
    let diff = mem_assert::MemoryDiff { author: "x".into(), created_at_ms: 1, nodes: vec![forged], edges: vec![] };
    let body = byom_call(&app, "mallory", byom_headers(secret, "mallory", "read,propose"), merge_rpc(&diff)).await;
    assert!(body.contains("\"isError\":true"), "PBA-L6b-002: forged front-run accepted: {body}");
    assert!(app.store.get_node(&vid).unwrap().is_none(), "PBA-L6b-002: forged copy landed");
    // Bob's own merge then lands his real state.
    let d2 = mem_assert::MemoryDiff { author: bob.pubkey_hex().into(), created_at_ms: 2, nodes: vec![victim.clone()], edges: vec![] };
    let body = byom_call(&app, "bob", byom_headers(secret, "bob", "read,propose"), merge_rpc(&d2)).await;
    assert!(body.contains("\"isError\":false"), "{body}");
    let a = app.store.get_node(&vid).unwrap().unwrap();
    assert_eq!((a.status, a.valid_to, a.confidence.clone()), (Status::Active, None, vec![BelnapValue::True]));
}
