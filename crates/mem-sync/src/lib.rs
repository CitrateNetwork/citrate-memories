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

use mem_core::{join_confidence, ContentHash, Edge, EdgeKind, MemoryNode, Plane, Status};
use mem_store::{MemoryDagStore, StoreError, SupersessionError};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("serialization error: {0}")]
    Serde(String),
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

/// Merge a remote node's advisory state into the local copy (same content id).
/// Returns the merged node and whether anything changed / contradicted.
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

    // Embedding is advisory side-data: keep local, adopt remote only if absent.
    if merged.embedding.is_none() {
        merged.embedding = remote.embedding.clone();
    }

    let changed = merged != *local;
    (merged, changed, contradictions)
}

/// Merge a remote bundle into the local store (the CRDT join). Idempotent and
/// order-insensitive in the final state; see the module docs for the one
/// documented exception (concurrent contradictory supersessions).
pub fn merge_bundle(
    store: &MemoryDagStore<MemoryNode>,
    bundle: &SyncBundle,
) -> Result<MergeOutcome, SyncError> {
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
    store.put_meta(&anchor_key(repo), &bytes)?;
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

#[cfg(test)]
mod tests;
