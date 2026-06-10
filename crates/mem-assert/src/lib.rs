//! `mem-assert` — the Asserted-plane write path (MEM-S3 / D3.3).
//!
//! The Derived plane is a deterministic projection of git/markdown; the **Asserted
//! plane** is where agents and humans record knowledge that lives nowhere else —
//! rationales, "tried X, it failed", confirmations, analogies. Per the core
//! invariant, asserted content is **signed and append-only**, and canonical for
//! itself (its home is the DAG).
//!
//! A [`MemoryDiff`] is the session-subgraph an agent emits; the next agent
//! [`apply_diff`]s it — "git for agents", replacing the HANDOFF.md workflow.

use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use mem_core::{
    BelnapValue, ContentHash, Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind,
    Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION,
};
use mem_store::{MemoryDagStore, StoreError, SupersessionError};

#[derive(Debug, thiserror::Error)]
pub enum AssertError {
    #[error("assertion is unsigned")]
    Unsigned,
    #[error("signature invalid")]
    BadSignature,
    #[error("author pubkey malformed")]
    BadAuthor,
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("supersession rejected: {0}")]
    Supersession(#[from] SupersessionError),
    #[error("serialization error: {0}")]
    Serde(String),
    /// FUA-MEMORIES-02: the signed-assert path tried to write a plane/trust-tier
    /// it is not allowed to mint (e.g. Derived plane, or the high-trust
    /// DerivedDeterministic / HumanConfirmed tiers).
    #[error("not assertable: {0}")]
    NotAssertable(String),
}

/// A signing identity for assertions. The author is the hex of the ed25519
/// verifying key, which is also the node's `author` field (identity-bearing).
pub struct Asserter {
    pubkey_hex: String,
    sk: SigningKey,
}

impl Asserter {
    pub fn new(sk: SigningKey) -> Self {
        let pubkey_hex = hex::encode(sk.verifying_key().to_bytes());
        Self { pubkey_hex, sk }
    }

    pub fn pubkey_hex(&self) -> &str {
        &self.pubkey_hex
    }

    /// Build a signed Asserted node. The signature is over the node's
    /// content-addressed id (which already includes the author), so any edit to
    /// content/author breaks it.
    pub fn assert_node(&self, repo: &str, kind: NodeKind, content: &str, now_ms: u64) -> MemoryNode {
        use ed25519_dalek::Signer;
        let key = format!("assert:{}", &blake3::hash(content.as_bytes()).to_hex()[..16]);
        let mut node = MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Asserted,
            kind,
            repo: repo.to_string(),
            author: self.pubkey_hex.clone(), // set before compute_id (identity-bearing)
            source_ref: SourceRef::DagNative { key },
            content: content.as_bytes().to_vec(),
            valid_from: now_ms,
            valid_to: None,
            observed_at: now_ms,
            trust_tier: TrustTier::AgentAsserted,
            signature: None,
            embedding: None,
            confidence: vec![BelnapValue::True],
            anchors: vec![],
            status: Status::Active,
        };
        let sig = self.sk.sign(node.compute_id().as_bytes());
        node.signature = Some(sig.to_bytes().to_vec());
        node
    }

    /// Build a signed Asserted edge (not quarantined — a direct assertion is
    /// load-bearing at the `AgentAsserted` tier; inferred edges are a separate path).
    pub fn assert_edge(&self, from: ContentHash, to: ContentHash, kind: EdgeKind, now_ms: u64) -> Edge {
        use ed25519_dalek::Signer;
        let mut edge = Edge {
            from,
            to,
            kind,
            plane: Plane::Asserted,
            trust_tier: TrustTier::AgentAsserted,
            provenance: EdgeProvenance {
                method: EdgeMethod::Manual,
                asserter: self.pubkey_hex.clone(),
                at: now_ms,
                evidence: None,
            },
            confidence: vec![BelnapValue::True],
            quarantined: false,
            signature: None,
        };
        let sig = self.sk.sign(&edge.key());
        edge.signature = Some(sig.to_bytes().to_vec());
        edge
    }
}

fn verify_sig(pubkey_hex: &str, msg: &[u8], sig: &Option<Vec<u8>>) -> Result<(), AssertError> {
    let sig = sig.as_ref().ok_or(AssertError::Unsigned)?;
    let pk_bytes = hex::decode(pubkey_hex).map_err(|_| AssertError::BadAuthor)?;
    let pk: [u8; 32] = pk_bytes.as_slice().try_into().map_err(|_| AssertError::BadAuthor)?;
    let sig_bytes: [u8; 64] = sig.as_slice().try_into().map_err(|_| AssertError::BadSignature)?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|_| AssertError::BadAuthor)?;
    let signature = Signature::from_bytes(&sig_bytes);
    vk.verify_strict(msg, &signature).map_err(|_| AssertError::BadSignature)
}

/// FUA-MEMORIES-02: the write boundary for the signed-assert path. An agent's
/// self-signed assertion may only land on the **Asserted** plane at an
/// **AgentAsserted** or **InferredAdvisory** tier. The high-trust
/// `DerivedDeterministic` / `HumanConfirmed` tiers and the `Derived` plane must
/// come from deterministic ingest or an explicit human-confirmation path — never
/// an agent's claim. `trust_tier` is excluded from `compute_id` (so it is not
/// covered by the signature), which is exactly why it must be policed here.
fn assertable_plane_and_tier(plane: Plane, tier: TrustTier) -> Result<(), AssertError> {
    if plane != Plane::Asserted {
        return Err(AssertError::NotAssertable(format!(
            "plane must be Asserted on the assert path, got {plane:?}"
        )));
    }
    if !matches!(tier, TrustTier::AgentAsserted | TrustTier::InferredAdvisory) {
        return Err(AssertError::NotAssertable(format!(
            "trust_tier must be AgentAsserted or InferredAdvisory, got {tier:?}"
        )));
    }
    Ok(())
}

/// Verify a signed Asserted node (plane/tier policy + author + signature over id).
pub fn verify_node(node: &MemoryNode) -> Result<(), AssertError> {
    assertable_plane_and_tier(node.plane, node.trust_tier)?;
    verify_sig(&node.author, node.compute_id().as_bytes(), &node.signature)
}

/// Verify a signed Asserted edge (plane/tier policy + asserter + signature).
pub fn verify_edge(edge: &Edge) -> Result<(), AssertError> {
    assertable_plane_and_tier(edge.plane, edge.trust_tier)?;
    verify_sig(&edge.provenance.asserter, &edge.key(), &edge.signature)
}

/// Provenance answer for `blame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blame {
    pub author: String,
    pub plane: Plane,
    pub trust_tier: TrustTier,
}

pub fn blame(node: &MemoryNode) -> Blame {
    Blame {
        author: node.author.clone(),
        plane: node.plane,
        trust_tier: node.trust_tier,
    }
}

/// A session subgraph an agent emits and the next agent merges. The unit of
/// "git for agents" handoff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryDiff {
    pub author: String,
    pub created_at_ms: u64,
    pub nodes: Vec<MemoryNode>,
    pub edges: Vec<Edge>,
}

impl MemoryDiff {
    pub fn new(author: impl Into<String>, now_ms: u64) -> Self {
        Self {
            author: author.into(),
            created_at_ms: now_ms,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_node(&mut self, n: MemoryNode) {
        self.nodes.push(n);
    }

    pub fn add_edge(&mut self, e: Edge) {
        self.edges.push(e);
    }

    pub fn to_json(&self) -> Result<String, AssertError> {
        serde_json::to_string(self).map_err(|e| AssertError::Serde(e.to_string()))
    }

    pub fn from_json(s: &str) -> Result<Self, AssertError> {
        serde_json::from_str(s).map_err(|e| AssertError::Serde(e.to_string()))
    }

    /// Verify every node and edge signature. A diff is accepted only if all hold.
    pub fn verify(&self) -> Result<(), AssertError> {
        for n in &self.nodes {
            verify_node(n)?;
        }
        for e in &self.edges {
            verify_edge(e)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeReport {
    pub nodes: usize,
    pub edges: usize,
    /// How many supersedes edges transitioned their target Active → Superseded
    /// (WP-1.4; idempotent re-merges count 0 here).
    pub superseded: usize,
}

/// Verify, then append a diff to a store. Rejected wholesale if any signature
/// fails (no partial poison). Append-only + content-addressed → idempotent.
///
/// Non-quarantined `Supersedes` edges are *applied*, not just stored (WP-1.4):
/// each one atomically writes the edge and transitions its target, guarded by
/// the cycle check. A rejected supersession (cycle/missing node) errors after
/// the diff's nodes and other edges have landed — safe, because everything is
/// idempotent: re-merging the same diff re-applies cleanly and the bad edge
/// stays rejected. Quarantined supersedes edges are stored as plain proposals.
pub fn apply_diff(
    store: &MemoryDagStore<MemoryNode>,
    diff: &MemoryDiff,
) -> Result<MergeReport, AssertError> {
    diff.verify()?;
    let (supersessions, plain): (Vec<&Edge>, Vec<&Edge>) = diff
        .edges
        .iter()
        .partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);
    let plain: Vec<Edge> = plain.into_iter().cloned().collect();
    store.commit(&diff.nodes, &plain)?;
    let mut superseded = 0;
    for e in supersessions {
        if store.apply_supersession(e)?.transitioned {
            superseded += 1;
        }
    }
    Ok(MergeReport {
        nodes: diff.nodes.len(),
        edges: diff.edges.len(),
        superseded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_store::kv::InMemoryKv;

    fn asserter(seed: u8) -> Asserter {
        Asserter::new(SigningKey::from_bytes(&[seed; 32]))
    }

    #[test]
    fn asserted_node_verifies() {
        let a = asserter(1);
        let n = a.assert_node("citrate-memories", NodeKind::Rationale, "we chose rocksdb for durability", 5);
        assert_eq!(n.plane, Plane::Asserted);
        assert_eq!(n.trust_tier, TrustTier::AgentAsserted);
        assert!(verify_node(&n).is_ok());
        assert_eq!(blame(&n).author, a.pubkey_hex());
    }

    #[test]
    fn rejects_forged_trust_tier_and_plane() {
        // FUA-MEMORIES-02: `trust_tier` is EXCLUDED from compute_id, so a signer
        // can flip it to the high-trust DerivedDeterministic / HumanConfirmed
        // AFTER signing and the signature still verifies. And a self-signed node
        // can claim the Derived plane. The write boundary must refuse both.
        let a = asserter(7);
        let mut n = a.assert_node("r", NodeKind::Rationale, "an agent's claim", 1);
        assert!(verify_node(&n).is_ok());

        // Forge the highest trust tier (signature still valid — tier isn't signed).
        n.trust_tier = TrustTier::DerivedDeterministic;
        assert!(matches!(verify_node(&n), Err(AssertError::NotAssertable(_))));
        n.trust_tier = TrustTier::HumanConfirmed;
        assert!(matches!(verify_node(&n), Err(AssertError::NotAssertable(_))));

        // Claim the Derived plane (refused before the signature is even checked).
        let mut d = a.assert_node("r", NodeKind::Rationale, "x", 1);
        d.plane = Plane::Derived;
        assert!(matches!(verify_node(&d), Err(AssertError::NotAssertable(_))));
    }

    #[test]
    fn tampered_node_fails_verification() {
        let a = asserter(1);
        let mut n = a.assert_node("r", NodeKind::Rationale, "original", 5);
        n.content = b"forged".to_vec(); // id changes → signature no longer matches
        assert!(matches!(verify_node(&n), Err(AssertError::BadSignature)));
    }

    #[test]
    fn asserted_edge_verifies() {
        let a = asserter(2);
        let n1 = a.assert_node("r", NodeKind::Rationale, "a", 1);
        let n2 = a.assert_node("r", NodeKind::Claim(mem_core::ClaimStatus::Confirmed), "b", 1);
        let e = a.assert_edge(n1.compute_id(), n2.compute_id(), EdgeKind::References, 1);
        assert!(!e.quarantined);
        assert!(verify_edge(&e).is_ok());
    }

    #[test]
    fn diff_roundtrips_and_applies() {
        let a = asserter(3);
        let mut diff = MemoryDiff::new(a.pubkey_hex(), 10);
        let n1 = a.assert_node("r", NodeKind::Rationale, "first insight", 10);
        let n2 = a.assert_node("r", NodeKind::Rationale, "second insight", 10);
        let e = a.assert_edge(n1.compute_id(), n2.compute_id(), EdgeKind::References, 10);
        diff.add_node(n1);
        diff.add_node(n2);
        diff.add_edge(e);

        // serialize → deserialize → verify
        let json = diff.to_json().unwrap();
        let restored = MemoryDiff::from_json(&json).unwrap();
        assert_eq!(restored, diff);
        assert!(restored.verify().is_ok());

        // merge into a fresh store
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let report = apply_diff(&store, &restored).unwrap();
        assert_eq!(report.nodes, 2);
        assert_eq!(report.edges, 1);
        assert_eq!(store.node_count().unwrap(), 2);

        // idempotent re-merge
        apply_diff(&store, &restored).unwrap();
        assert_eq!(store.node_count().unwrap(), 2);
    }

    #[test]
    fn diff_supersession_transitions_target() {
        let a = asserter(5);
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));

        // Session 1 asserts the original understanding.
        let old = a.assert_node("r", NodeKind::Rationale, "we thought X because Y", 1);
        let old_id = old.compute_id();
        let mut d1 = MemoryDiff::new(a.pubkey_hex(), 1);
        d1.add_node(old);
        apply_diff(&store, &d1).unwrap();

        // Session 2 supersedes it.
        let new = a.assert_node("r", NodeKind::Rationale, "X was wrong; it is Z", 2);
        let e = a.assert_edge(new.compute_id(), old_id, EdgeKind::Supersedes, 2);
        let mut d2 = MemoryDiff::new(a.pubkey_hex(), 2);
        d2.add_node(new);
        d2.add_edge(e);
        let report = apply_diff(&store, &d2).unwrap();
        assert_eq!(report.superseded, 1);

        let old_now = store.get_node(&old_id).unwrap().unwrap();
        assert_eq!(old_now.status, Status::Superseded);
        assert_eq!(old_now.valid_to, Some(2));

        // Idempotent re-merge: no second transition.
        assert_eq!(apply_diff(&store, &d2).unwrap().superseded, 0);
    }

    #[test]
    fn diff_with_cyclic_supersession_is_rejected() {
        let a = asserter(6);
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let n1 = a.assert_node("r", NodeKind::Rationale, "first", 1);
        let n2 = a.assert_node("r", NodeKind::Rationale, "second", 1);
        let e12 = a.assert_edge(n1.compute_id(), n2.compute_id(), EdgeKind::Supersedes, 1);
        let e21 = a.assert_edge(n2.compute_id(), n1.compute_id(), EdgeKind::Supersedes, 1);
        let mut diff = MemoryDiff::new(a.pubkey_hex(), 1);
        diff.add_node(n1.clone());
        diff.add_node(n2);
        diff.add_edge(e12);
        diff.add_edge(e21);

        let err = apply_diff(&store, &diff).unwrap_err();
        assert!(matches!(err, AssertError::Supersession(SupersessionError::Cycle)));
        // The first supersession applied; the cycle-closing one is rejected forever
        // — Acyclic holds.
        assert_eq!(store.get_node(&n1.compute_id()).unwrap().unwrap().status, Status::Active);
    }

    #[test]
    fn forged_diff_is_rejected_wholesale() {
        let a = asserter(4);
        let mut diff = MemoryDiff::new(a.pubkey_hex(), 1);
        let mut n = a.assert_node("r", NodeKind::Rationale, "real", 1);
        n.content = b"tampered".to_vec();
        diff.add_node(n);

        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        assert!(apply_diff(&store, &diff).is_err());
        assert_eq!(store.node_count().unwrap(), 0, "no partial application of a forged diff");
    }
}
