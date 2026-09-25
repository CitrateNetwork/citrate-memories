//! Order-independent join of a content-addressed node's advisory (non-identity)
//! fields — the Belnap-CRDT node merge (MEM-B-002), shared by BOTH write paths
//! that can meet an id already in the store:
//!
//! - `mem-sync` `merge_bundle` (federation replica merge), and
//! - [`apply_diff`](crate::apply_diff) (the `memory.merge_diff` MCP/BYOM path) —
//!   PBA-L6b-002: that path used to overwrite an existing node last-writer-wins.
//!
//! Moved here verbatim from `mem-sync` (which depends on this crate) so the two
//! paths cannot drift apart again.

use mem_core::{join_confidence, CodeAnchor, MemoryNode, Status, TrustTier, VersionedVector};

pub fn status_rank(s: Status) -> u8 {
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
pub fn trust_conservatism(t: TrustTier) -> u8 {
    match t {
        TrustTier::InferredAdvisory => 0,
        TrustTier::AgentAsserted => 1,
        TrustTier::HumanConfirmed => 2,
        TrustTier::DerivedDeterministic => 3,
    }
}

/// The least-trusted of two tiers (commutative, associative, idempotent).
pub fn less_trusted(a: TrustTier, b: TrustTier) -> TrustTier {
    if trust_conservatism(a) <= trust_conservatism(b) {
        a
    } else {
        b
    }
}

/// A total, order-independent comparison key for an embedding: `(model, bits)`.
/// `f32` is not `Ord`, so compare the raw IEEE-754 bit patterns.
pub fn embedding_key(v: &VersionedVector) -> (String, Vec<u32>) {
    (v.model.clone(), v.data.iter().map(|f| f.to_bits()).collect())
}

/// Deterministic tiebreak: the lexicographically smallest `(model, bits)`. Absent
/// on either side adopts the present one; absent on both stays absent.
pub fn min_embedding(a: &Option<VersionedVector>, b: &Option<VersionedVector>) -> Option<VersionedVector> {
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
pub fn min_opt_bytes(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> Option<Vec<u8>> {
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
pub fn union_anchors(a: &[CodeAnchor], b: &[CodeAnchor]) -> Vec<CodeAnchor> {
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
pub fn merge_node(local: &MemoryNode, remote: &MemoryNode) -> (MemoryNode, bool, usize) {
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
