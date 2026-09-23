//! `mem-sync` — federation sync for citrate-memories (MEM-S5).
//!
//! **WP-5.1 Belnap-CRDT merge.** A tenant DAG replica exports a [`SyncBundle`];
//! a peer merges it with [`merge_bundle`]. The merge is a state-based CRDT:
//!
//! - **Nodes/edges are grow-only and content-addressed** → set union is
//!   conflict-free and idempotent.
//! - **Confidence merges via Belnap `join`** (verified bounded lattice):
//!   `True ⊔ False = Both` *surfaces* a genuine cross-replica contradiction
//!   instead of silently picking a winner.
//! - **Status merges monotonically** (Active < Superseded < Archived): a
//!   correction seen by either replica survives the merge; `valid_to` keeps the
//!   earliest supersession time (matching the local first-wins rule).
//! - **Quarantine merges monotonically downward**: an edge confirmed by either
//!   replica is confirmed after the merge (promotion is one-way, R1).
//! - **Supersedes edges are applied, not just stored** — each routes through
//!   `apply_supersession`, so the local `Acyclic` invariant always holds.
//!
//! **Known, documented limit:** two replicas that *concurrently* asserted
//! opposite supersessions (`a⊃b` here, `b⊃a` there) converge in **status**
//! (both nodes end Superseded — both views were corrected) but each replica
//! rejects the other's cycle-closing edge, so the supersedes edge sets differ
//! by that pair. That is a real epistemic conflict, surfaced in
//! [`MergeOutcome::rejected_supersessions`]; a global order resolves it — which
//! is exactly what chain-anchoring (WP-5.2 / phase 2) provides.
//!
//! **Trust at the boundary:** Asserted-plane nodes/edges must carry valid
//! signatures (and pass the FUA-02 plane/tier policy) or they are rejected
//! individually — a peer cannot poison the Asserted plane. Derived-plane
//! content is a deterministic projection of git/markdown and is accepted
//! as-is in v1; anchored-root comparison (WP-5.2) is the tamper-evidence for
//! it, and re-ingest from source is always available.
//!
//! **WP-5.2 merkle anchoring.** [`tenant_root`] computes a deterministic
//! merkle root over a tenant's nodes *and their advisory state* (status,
//! `valid_to`, edge quarantine flags — fields outside the content id), so
//! tampering with "what is current" is detectable, not just tampering with
//! content. [`anchor_tenant`] records a hash-chained [`AnchorRecord`];
//! submission of the root to citrate-chain (via `AuditChain.chain_anchor`) is
//! the phase-2 operator step.

use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, BTreeSet};

use mem_authz::{AuthzError, CapabilityGrant, Op};
use mem_core::{
    join_confidence, CodeAnchor, ContentHash, Edge, EdgeKind, EdgeMethod, MemoryNode, Plane,
    Status, TrustTier, VersionedVector,
};
use mem_store::{MemoryDagStore, StoreError, SupersessionError};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("serialization error: {0}")]
    Serde(String),
    /// WP-5.3: a chain-anchor checkpoint binding failed (RPC unreachable,
    /// malformed response, or wrong chain). Fail closed — never record an
    /// unverified checkpoint.
    #[error("chain anchor error: {0}")]
    Chain(String),
    /// FWA-C10-01/02: the merging peer's grant does not authorize a Write to a
    /// repo the bundle touches. The merge is refused wholesale (no partial,
    /// no-poison) — mirroring the MCP `merge_diff` authz loop, which authorizes
    /// EVERY touched repo before any write lands.
    #[error("merge denied: {0}")]
    Denied(#[from] AuthzError),
    /// FWA-C10-01/02: a bundle edge references an endpoint node that is neither
    /// in the bundle nor in the local store, so its owning repo cannot be
    /// resolved and therefore cannot be authorized. Fail closed.
    #[error("merge denied: edge endpoint {0} owns no resolvable repo (cannot authorize)")]
    UnresolvableEndpoint(String),
    /// MEM-B-004 / WP-MEM trust-root anchoring: the presenting peer's grant is
    /// not issued by the trusted grant root (its `issuer_pubkey` does not match
    /// the `trust_root` the merge was called with). `CapabilityGrant::check`
    /// verifies a grant's signature against the key carried *inside* the grant, so
    /// a peer that mints and self-signs a `*` read+write grant would otherwise
    /// pass — exactly the forgeable-grant hole the MCP `_meta` path closed
    /// (MEM-B-001). Fail closed: the whole bundle is refused.
    #[error("merge denied: grant issuer is not the trusted grant root")]
    UntrustedIssuer,
}

/// Map a tenant `repo` to the capability-grant resource id. MUST match the MCP
/// path's mapping (`mem-mcp` `authorize`: `format!("repo:{repo}/memory")`) so
/// the two write paths authorize against the SAME resource namespace.
pub(crate) fn resource_for(repo: &str) -> String {
    format!("repo:{repo}/memory")
}

/// A tenant replica's exported state: the unit of federation transfer. Plain
/// data — ship it over any transport (file, HTTP, gossip).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBundle {
    pub repo: String,
    pub exported_at_ms: u64,
    pub nodes: Vec<MemoryNode>,
    pub edges: Vec<Edge>,
}

impl SyncBundle {
    pub fn to_json(&self) -> Result<String, SyncError> {
        serde_json::to_string(self).map_err(|e| SyncError::Serde(e.to_string()))
    }

    pub fn from_json(s: &str) -> Result<Self, SyncError> {
        serde_json::from_str(s).map_err(|e| SyncError::Serde(e.to_string()))
    }
}

/// Export one tenant's nodes and the edges leaving them (cross-tenant edges
/// included — the peer may or may not hold the far endpoint; dangling edges
/// are tolerated by the store and resolve when the far tenant syncs).
pub fn export_tenant(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
    now_ms: u64,
) -> Result<SyncBundle, SyncError> {
    let (nodes, edges) = tenant_state(store, repo)?;
    Ok(SyncBundle { repo: repo.to_string(), exported_at_ms: now_ms, nodes, edges })
}

/// What a merge did — every rejection is counted, never silent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    pub nodes_added: usize,
    /// Existing nodes whose advisory state changed under the merge.
    pub nodes_merged: usize,
    pub edges_added: usize,
    /// Existing edges whose quarantine/confidence changed.
    pub edges_merged: usize,
    /// Active → Superseded transitions applied from the bundle's supersessions.
    pub superseded: usize,
    /// Supersedes edges rejected by the local guards (cycle / missing target).
    pub rejected_supersessions: usize,
    /// Asserted-plane items whose signature or plane/tier policy failed.
    pub rejected_signatures: usize,
    /// Confidence dimensions that became `Both` (True ⊔ False) in this merge —
    /// genuine cross-replica contradictions an agent should resolve.
    pub contradictions: usize,
}

fn status_rank(s: Status) -> u8 {
    match s {
        Status::Active => 0,
        Status::Superseded => 1,
        Status::Archived => 2,
    }
}

// ---------------------------------------------------------------------------
// MEM-B-002: order-independent merge of advisory (non-identity) fields.
//
// The CRDT is a *state-based* merge, so it must be a join-semilattice:
// commutative, associative and idempotent. `merge_node`/the edge merge used to
// keep the LOCAL value of `embedding`/`trust_tier`/`plane`/`provenance`/
// `signature`/`anchors` — i.e. first-writer-wins *on the receiving replica* — so
// two replicas that exchanged bundles in different orders never converged. Every
// non-identity field is now folded by an explicit, order-independent rule.
// ---------------------------------------------------------------------------

/// Trust rank where a SMALLER number is LESS trusted. The merge keeps the
/// least-trusted value on a conflict (monotone downward, matching the one-way
/// quarantine rule) — conservative *and* order-independent.
fn trust_conservatism(t: TrustTier) -> u8 {
    match t {
        TrustTier::InferredAdvisory => 0,
        TrustTier::AgentAsserted => 1,
        TrustTier::HumanConfirmed => 2,
        TrustTier::DerivedDeterministic => 3,
    }
}

/// The least-trusted of two tiers (commutative, associative, idempotent).
fn less_trusted(a: TrustTier, b: TrustTier) -> TrustTier {
    if trust_conservatism(a) <= trust_conservatism(b) {
        a
    } else {
        b
    }
}

/// A total, order-independent comparison key for an embedding: `(model, bits)`.
/// `f32` is not `Ord`, so compare the raw IEEE-754 bit patterns.
fn embedding_key(v: &VersionedVector) -> (String, Vec<u32>) {
    (v.model.clone(), v.data.iter().map(|f| f.to_bits()).collect())
}

/// Deterministic tiebreak: the lexicographically smallest `(model, bits)`. Absent
/// on either side adopts the present one; absent on both stays absent.
fn min_embedding(a: &Option<VersionedVector>, b: &Option<VersionedVector>) -> Option<VersionedVector> {
    match (a, b) {
        (None, x) | (x, None) => x.clone(),
        (Some(x), Some(y)) => {
            if embedding_key(x) <= embedding_key(y) {
                Some(x.clone())
            } else {
                Some(y.clone())
            }
        }
    }
}

/// Deterministic tiebreak over an optional signature (smallest bytes; present
/// beats absent). Both sides carry a valid signature over the *same* content id,
/// so either is verifiable — the choice only needs to be replica-independent.
fn min_opt_bytes(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> Option<Vec<u8>> {
    match (a, b) {
        (None, x) | (x, None) => x.clone(),
        (Some(x), Some(y)) => {
            if x <= y {
                Some(x.clone())
            } else {
                Some(y.clone())
            }
        }
    }
}

/// Grow-only set union of code anchors (sorted + deduped by a total key), so
/// `anchors` converge instead of silently keeping the receiver's copy.
fn union_anchors(a: &[CodeAnchor], b: &[CodeAnchor]) -> Vec<CodeAnchor> {
    let key = |c: &CodeAnchor| {
        (c.repo.clone(), c.path.clone(), c.symbol.clone(), c.line_start, c.line_end)
    };
    let mut all: Vec<CodeAnchor> = a.iter().chain(b).cloned().collect();
    all.sort_by_key(|x| key(x));
    all.dedup_by(|x, y| key(x) == key(y));
    all
}

/// Merge a remote node's advisory state into the local copy (same content id).
/// Returns the merged node and whether anything changed / contradicted.
///
/// MEM-B-002: EVERY field outside the content id is folded by an
/// order-independent rule, so replicas converge regardless of gossip order.
fn merge_node(local: &MemoryNode, remote: &MemoryNode) -> (MemoryNode, bool, usize) {
    let mut merged = local.clone();

    let joined = join_confidence(&local.confidence, &remote.confidence);
    let contradictions = joined
        .iter()
        .zip(local.confidence.iter().chain(std::iter::repeat(&mem_core::BelnapValue::Neither)))
        .filter(|(now, before)| now.is_contradiction() && !before.is_contradiction())
        .count();
    merged.confidence = joined;

    // Monotone status: a correction seen anywhere survives everywhere.
    if status_rank(remote.status) > status_rank(merged.status) {
        merged.status = remote.status;
        merged.valid_to = remote.valid_to;
    } else if remote.status == merged.status {
        // Same rank: keep the earliest end-of-validity (first correction wins).
        merged.valid_to = match (merged.valid_to, remote.valid_to) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }

    // Advisory fields — order-independent joins (MEM-B-002). Least-trusted wins;
    // embedding/signature use a deterministic tiebreak; anchors grow-only union;
    // bitemporal marks keep the earliest observation.
    merged.trust_tier = less_trusted(local.trust_tier, remote.trust_tier);
    merged.embedding = min_embedding(&local.embedding, &remote.embedding);
    merged.signature = min_opt_bytes(&local.signature, &remote.signature);
    merged.anchors = union_anchors(&local.anchors, &remote.anchors);
    merged.valid_from = local.valid_from.min(remote.valid_from);
    merged.observed_at = local.observed_at.min(remote.observed_at);

    let changed = merged != *local;
    (merged, changed, contradictions)
}

/// A total, order-independent selection key for an edge's advisory (non-key)
/// fields. Ordered so the merge deterministically keeps the LEAST-trusted edge
/// (matching the one-way quarantine rule), then breaks ties on
/// plane/provenance/signature — making the edge merge commutative (MEM-B-002).
fn edge_advisory_key(e: &Edge) -> (u8, u8, u8, String, u64, String, Vec<u8>) {
    let plane_rank = match e.plane {
        Plane::Derived => 0,
        Plane::Asserted => 1,
    };
    let method_rank = match e.provenance.method {
        EdgeMethod::Trailer => 0,
        EdgeMethod::Ingest => 1,
        EdgeMethod::Nlp => 2,
        EdgeMethod::Analogy => 3,
        EdgeMethod::Manual => 4,
    };
    (
        trust_conservatism(e.trust_tier),
        plane_rank,
        method_rank,
        e.provenance.asserter.clone(),
        e.provenance.at,
        e.provenance.evidence.clone().unwrap_or_default(),
        e.signature.clone().unwrap_or_default(),
    )
}

/// Resolve the repo that owns a node id: prefer the bundle's own nodes (so a
/// self-contained bundle authorizes without a store round-trip), else the local
/// store. `None` if neither knows the endpoint — caller fails closed.
fn endpoint_repo(
    store: &MemoryDagStore<MemoryNode>,
    in_bundle: &BTreeMap<ContentHash, String>,
    endpoint: &ContentHash,
) -> Result<Option<String>, SyncError> {
    if let Some(repo) = in_bundle.get(endpoint) {
        return Ok(Some(repo.clone()));
    }
    Ok(store.get_node(endpoint)?.map(|n| n.repo))
}

/// FWA-C10-01/02 — the unified federation-merge authorization gate.
///
/// Collect EVERY repo the bundle would write to (node repos + the owning repo of
/// each edge endpoint, resolved from the bundle's own nodes or the local store)
/// and require `grant` to authorize a `Write` to each, BEFORE any node/edge
/// lands. This mirrors the MCP `merge_diff` authz loop (mem-mcp/src/lib.rs:665-695)
/// so the two write paths cannot diverge again: a peer can only merge into repos
/// its capability grant actually covers. Fails closed (whole bundle refused) on
/// any denied repo or any edge endpoint whose owning repo cannot be resolved.
fn authorize_bundle(
    store: &MemoryDagStore<MemoryNode>,
    grant: &CapabilityGrant,
    trust_root: &[u8],
    bundle: &SyncBundle,
    now_ms: u64,
) -> Result<(), SyncError> {
    // MEM-B-004 / WP-MEM trust-root anchoring. `grant.check` (below) verifies the
    // grant's signature against the `issuer_pubkey` carried *inside* the grant, so
    // a peer that mints its own ed25519 keypair and self-signs a `*` read+write
    // grant would authorize itself for every repo. Bind the issuer to the trusted
    // grant root first — the same anchoring the MCP `_meta` path uses (MEM-B-001).
    // A grant whose issuer key is not the root is refused wholesale, fail closed.
    if grant.issuer_pubkey.as_slice() != trust_root {
        return Err(SyncError::UntrustedIssuer);
    }
    let in_bundle: BTreeMap<ContentHash, String> =
        bundle.nodes.iter().map(|n| (n.compute_id(), n.repo.clone())).collect();

    let mut repos: BTreeSet<String> = bundle.nodes.iter().map(|n| n.repo.clone()).collect();
    for e in &bundle.edges {
        for endpoint in [&e.from, &e.to] {
            match endpoint_repo(store, &in_bundle, endpoint)? {
                Some(repo) => {
                    repos.insert(repo);
                }
                None => {
                    return Err(SyncError::UnresolvableEndpoint(endpoint.to_hex()));
                }
            }
        }
    }

    for repo in &repos {
        grant.check(&resource_for(repo), Op::Write, now_ms)?;
    }
    Ok(())
}

/// Merge a remote bundle into the local store (the CRDT join). Idempotent and
/// order-insensitive in the final state; see the module docs for the one
/// documented exception (concurrent contradictory supersessions).
///
/// **FWA-C10-01/02 + MEM-B-004 (authorization).** The merging peer presents a
/// [`CapabilityGrant`] and the caller supplies `trust_root`, the ed25519 public
/// key that legitimately issues grants (the gateway/operator key). The merge (a)
/// requires the grant's `issuer_pubkey` to equal `trust_root` — a self-signed
/// grant a peer mints for itself is refused, closing the forgeable-grant hole the
/// MCP `_meta` path closed (MEM-B-001) — and (b) authorizes a `Write` to EVERY
/// repo the bundle touches (node repos + edge-endpoint repos) before any write
/// lands, the same gate the MCP `merge_diff` path uses. A peer therefore cannot
/// inject Derived-plane nodes or unsigned Derived `Supersedes` edges into a repo
/// its grant does not cover, and cannot mint that grant itself. Asserted-plane
/// items must still carry a valid signature (per-item), unchanged. Use
/// [`merge_bundle_trusted`] only for in-process, already-trusted ingest (it grants
/// `*` and is its own trust root), never across a federation boundary.
pub fn merge_bundle(
    store: &MemoryDagStore<MemoryNode>,
    bundle: &SyncBundle,
    grant: &CapabilityGrant,
    trust_root: &[u8],
    now_ms: u64,
) -> Result<MergeOutcome, SyncError> {
    // FWA-C10-01/02 + MEM-B-004: authorize the WHOLE bundle first — the grant must
    // be issued by `trust_root` AND authorize a Write to every repo the bundle
    // touches — fail closed, no partial application, before a single node or edge
    // can reach the store.
    authorize_bundle(store, grant, trust_root, bundle, now_ms)?;

    let mut out = MergeOutcome::default();

    // ---- nodes ----
    let mut nodes = bundle.nodes.clone();
    nodes.sort_by_key(|n| n.compute_id());
    for remote in &nodes {
        // The Asserted plane is signed or it doesn't enter (anti-poisoning).
        if remote.plane == Plane::Asserted && mem_assert::verify_node(remote).is_err() {
            out.rejected_signatures += 1;
            continue;
        }
        let id = remote.compute_id();
        match store.get_node(&id)? {
            None => {
                store.put_node(remote)?;
                out.nodes_added += 1;
            }
            Some(local) => {
                let (merged, changed, contradictions) = merge_node(&local, remote);
                out.contradictions += contradictions;
                if changed {
                    store.put_node(&merged)?;
                    out.nodes_merged += 1;
                }
            }
        }
    }

    // ---- edges ----
    let mut edges = bundle.edges.clone();
    edges.sort_by_key(|e| e.key());
    let (supersessions, plain): (Vec<Edge>, Vec<Edge>) = edges
        .into_iter()
        .partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);

    for remote in &plain {
        if remote.plane == Plane::Asserted && mem_assert::verify_edge(remote).is_err() {
            out.rejected_signatures += 1;
            continue;
        }
        let existing = store
            .out_edges(&remote.from)?
            .into_iter()
            .find(|e| e.to == remote.to && e.kind == remote.kind);
        match existing {
            None => {
                store.add_edge(remote)?;
                out.edges_added += 1;
            }
            Some(local) => {
                let mut merged = local.clone();
                // One-way promotion: confirmed anywhere = confirmed everywhere.
                merged.quarantined = local.quarantined && remote.quarantined;
                merged.confidence = join_confidence(&local.confidence, &remote.confidence);
                // MEM-B-002: converge the advisory group (trust_tier / plane /
                // provenance / signature) deterministically — least-trusted, then
                // a total tiebreak — instead of silently keeping the receiver's
                // copy, so the edge merge is order-independent.
                if edge_advisory_key(remote) < edge_advisory_key(&local) {
                    merged.trust_tier = remote.trust_tier;
                    merged.plane = remote.plane;
                    merged.provenance = remote.provenance.clone();
                    merged.signature = remote.signature.clone();
                }
                if merged != local {
                    store.add_edge(&merged)?;
                    out.edges_merged += 1;
                }
            }
        }
    }

    for remote in &supersessions {
        if remote.plane == Plane::Asserted && mem_assert::verify_edge(remote).is_err() {
            out.rejected_signatures += 1;
            continue;
        }
        let existed = store
            .out_edges(&remote.from)?
            .iter()
            .any(|e| e.to == remote.to && e.kind == remote.kind);
        match store.apply_supersession(remote) {
            Ok(r) => {
                out.superseded += usize::from(r.transitioned);
                out.edges_added += usize::from(!existed);
            }
            Err(SupersessionError::Store(e)) => return Err(SyncError::Store(e)),
            Err(_) => out.rejected_supersessions += 1,
        }
    }

    Ok(out)
}

/// FWA-C10-01/02 — an explicit, signed `*`-grant for **in-process, already-trusted**
/// ingest only (self-merge, a local replica re-importing its own export, the
/// deterministic git/markdown ingestor). Naming the trust decision keeps it
/// auditable and greppable — a federation/transport caller must NEVER use this;
/// it must pass the connecting peer's real [`CapabilityGrant`] to [`merge_bundle`].
pub fn trusted_local_grant() -> CapabilityGrant {
    use ed25519_dalek::SigningKey;
    use mem_authz::{PolicyProfile, ResourceScope};
    // A throwaway, process-local signing key — the grant is self-issued and
    // covers every resource for Read+Write. It exists solely so the trusted
    // ingest path flows through the SAME authz gate as the untrusted one (no
    // second, unguarded code path can exist).
    //
    // MEM-B-018: the key is seeded from the OS CSPRNG per call, not a published
    // constant (`[0xA1u8; 32]`). The grant is its own trust root, so an ephemeral
    // key is functionally identical while removing a hardcoded, known signing key
    // from the tree. Mirrors `mem-mcp`'s ephemeral-identity fallback.
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).expect("OS randomness for trusted-local grant key");
    let sk = SigningKey::from_bytes(&seed);
    let mut g = CapabilityGrant {
        id: "mem-sync:trusted-local-ingest".into(),
        issuer: "mem-sync".into(),
        recipient: "mem-sync:local".into(),
        allowed_resources: vec![ResourceScope {
            resource_id: "*".into(),
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
    g.sign_with(&sk);
    g
}

/// Convenience for in-process, already-trusted ingest: [`merge_bundle`] with the
/// [`trusted_local_grant`]. Use ONLY where the bundle is the local replica's own
/// state (self-merge, re-import) or comes from the deterministic ingestor — never
/// for a bundle received over a federation transport.
pub fn merge_bundle_trusted(
    store: &MemoryDagStore<MemoryNode>,
    bundle: &SyncBundle,
) -> Result<MergeOutcome, SyncError> {
    // Self-issued grant: it IS its own trust root (MEM-B-004).
    let grant = trusted_local_grant();
    let root = grant.issuer_pubkey.clone();
    merge_bundle(store, bundle, &grant, &root, 0)
}

// ---------------------------------------------------------------------------
// WP-5.2: merkle anchoring
// ---------------------------------------------------------------------------

/// A tenant's nodes (sorted by id) and the deduped edges leaving them (sorted
/// by key) — the replica-independent canonical order.
fn tenant_state(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
) -> Result<(Vec<MemoryNode>, Vec<Edge>), SyncError> {
    let mut nodes: Vec<MemoryNode> = store
        .all_nodes()?
        .into_iter()
        .filter(|n| n.repo == repo)
        .collect();
    nodes.sort_by_key(|n| n.compute_id());
    let mut edges: Vec<Edge> = Vec::new();
    for n in &nodes {
        edges.extend(store.out_edges(&n.compute_id())?);
    }
    edges.sort_by_key(|e| e.key());
    edges.dedup_by_key(|e| e.key());
    Ok((nodes, edges))
}

/// Deterministic merkle root over a tenant's state. Leaves cover the content
/// ids **and the advisory fields outside them** (status, `valid_to`, edge
/// quarantine), so "what is current" is tamper-evident, not just "what was
/// said". Leaf order is the sorted id/key order → replica-independent.
pub fn tenant_root(store: &MemoryDagStore<MemoryNode>, repo: &str) -> Result<ContentHash, SyncError> {
    let (nodes, edges) = tenant_state(store, repo)?;
    let mut leaves: Vec<[u8; 32]> = Vec::with_capacity(nodes.len() + edges.len());
    for n in &nodes {
        let mut h = blake3::Hasher::new();
        h.update(n.compute_id().as_bytes());
        h.update(&[status_rank(n.status)]);
        h.update(&n.valid_to.unwrap_or(0).to_le_bytes());
        leaves.push(*h.finalize().as_bytes());
    }
    for e in &edges {
        let mut h = blake3::Hasher::new();
        h.update(&e.key());
        h.update(&[u8::from(e.quarantined)]);
        leaves.push(*h.finalize().as_bytes());
    }

    if leaves.is_empty() {
        return Ok(ContentHash(*blake3::hash(repo.as_bytes()).as_bytes()));
    }
    while leaves.len() > 1 {
        leaves = leaves
            .chunks(2)
            .map(|pair| {
                let mut h = blake3::Hasher::new();
                h.update(&pair[0]);
                if pair.len() == 2 {
                    h.update(&pair[1]);
                }
                *h.finalize().as_bytes()
            })
            .collect();
    }
    Ok(ContentHash(leaves[0]))
}

/// A hash-chained anchor of a tenant's state at a point in time. The latest
/// record lives in the store's meta CF; `prev` links back so reordering or
/// dropping an anchor is detectable. Phase 2 submits `root` to citrate-chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorRecord {
    pub repo: String,
    pub root: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub anchored_at_ms: u64,
    /// blake3 of the previous record's JSON (hex), `None` for the first anchor.
    pub prev: Option<String>,
}

fn anchor_key(repo: &str) -> Vec<u8> {
    format!("anchor:{repo}").into_bytes()
}

/// Read the latest anchor for a tenant.
pub fn latest_anchor(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
) -> Result<Option<AnchorRecord>, SyncError> {
    match store.get_meta(&anchor_key(repo))? {
        None => Ok(None),
        Some(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).map_err(|e| SyncError::Serde(e.to_string()))?,
        )),
    }
}

/// Compute the tenant's current root and append a new hash-chained anchor.
pub fn anchor_tenant(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
    now_ms: u64,
) -> Result<AnchorRecord, SyncError> {
    let root = tenant_root(store, repo)?;
    let prev = store
        .get_meta(&anchor_key(repo))?
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string());
    let (nodes, edges) = tenant_state(store, repo)?;
    let record = AnchorRecord {
        repo: repo.to_string(),
        root: root.to_hex(),
        node_count: nodes.len(),
        edge_count: edges.len(),
        anchored_at_ms: now_ms,
        prev,
    };
    let bytes = serde_json::to_vec(&record).map_err(|e| SyncError::Serde(e.to_string()))?;
    store.put_meta(repo, &anchor_key(repo), &bytes)?;
    Ok(record)
}

/// Does the tenant's current state still match its latest anchor?
/// `Ok(None)` = never anchored; `Ok(Some(true))` = matches.
pub fn verify_anchor(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
) -> Result<Option<bool>, SyncError> {
    let Some(record) = latest_anchor(store, repo)? else {
        return Ok(None);
    };
    Ok(Some(tenant_root(store, repo)?.to_hex() == record.root))
}

#[cfg(feature = "chain")]
pub mod chain;

#[cfg(feature = "http")]
pub mod transport;

#[cfg(test)]
mod tests;
