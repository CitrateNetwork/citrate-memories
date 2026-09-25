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

pub mod join;

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
    /// PBA-L6b-002: a diff would change the stored state of an existing node or
    /// edge that the merging principal did not author. Signatures cover only the
    /// content id (node) or `from‖to‖kind` (edge), so a non-author could otherwise
    /// rewrite status / valid_to / confidence / embedding / provenance while the
    /// item stays attributed to — and verifying as — the original author.
    #[error("not the author of existing {0}; refusing to change another principal's signed state")]
    NotAuthor(String),
    /// PBA-L6b-002: a diff tried to promote a stored quarantined proposal to a
    /// load-bearing edge. The quarantine flag is not signed; promotion goes
    /// through `confirm_edge`, never a diff.
    #[error("diff may not promote quarantined edge {0}; use confirm_edge")]
    EdgePromotion(String),
}

/// A signing identity for assertions. The author is the hex of the ed25519
/// verifying key, which is also the node's `author` field (identity-bearing).
///
/// `Clone` so a shared identity (e.g. the gateway's `Arc<Asserter>`) can be handed
/// by value to a per-request MCP session (`MemoryMcpServer::new_with_asserter`).
#[derive(Clone)]
pub struct Asserter {
    pubkey_hex: String,
    sk: SigningKey,
}

impl Asserter {
    pub fn new(sk: SigningKey) -> Self {
        let pubkey_hex = hex::encode(sk.verifying_key().to_bytes());
        Self { pubkey_hex, sk }
    }

    /// FWA-C10-04 — bind authorship to the authenticated principal.
    ///
    /// The gateway holds ONE signing key but serves many principals; using it
    /// directly made every node's `author` the gateway pubkey, so `blame()`
    /// could not tell two users apart. Derive a deterministic per-principal
    /// ed25519 sub-identity from the gateway key + the authenticated `sub`
    /// (domain-separated HKDF-like blake3 of the gateway secret scalar bytes and
    /// the sub). The subkey:
    /// - is unique per `sub` → `blame()` now identifies the actor, not the gateway;
    /// - is deterministic → the same principal always signs under the same author,
    ///   so content-addressing and idempotent re-merge are preserved;
    /// - cannot be derived without the gateway secret → a client cannot forge
    ///   another principal's author (no per-user key escrow needed in v1).
    ///
    /// Full SIWE/OIDC-wallet–bound identities (citrate-identity) remain the v2
    /// upgrade; this closes the attribution gap without that integration.
    pub fn for_principal(gateway_sk: &SigningKey, sub: &str) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"mem-assert:principal-subkey:v1");
        h.update(&gateway_sk.to_bytes());
        h.update(&(sub.len() as u64).to_le_bytes());
        h.update(sub.as_bytes());
        let seed: [u8; 32] = *h.finalize().as_bytes();
        Self::new(SigningKey::from_bytes(&seed))
    }

    pub fn pubkey_hex(&self) -> &str {
        &self.pubkey_hex
    }

    /// Build a signed Asserted node. The signature is over the node's
    /// content-addressed id (which already includes the author), so any edit to
    /// content/author breaks it.
    ///
    /// Stamps `valid_from = observed_at = now_ms` and leaves the node
    /// unembedded. Callers that know the claim's real-world date, or that can
    /// embed in the tenant's vector space, should use [`assert_node_with`]
    /// instead: an unembedded node is invisible to `Recall::search`, which only
    /// indexes nodes whose `embedding` is `Some`.
    ///
    /// [`assert_node_with`]: Asserter::assert_node_with
    pub fn assert_node(&self, repo: &str, kind: NodeKind, content: &str, now_ms: u64) -> MemoryNode {
        self.assert_node_with(repo, kind, content, now_ms, now_ms, None)
    }

    /// [`assert_node`](Asserter::assert_node) with an explicit real-world
    /// `valid_from` and an optional embedding.
    ///
    /// Both extras are safe by construction: `compute_id` covers
    /// `{schema_version, plane, kind, repo, author, source_ref, content}` only,
    /// so neither `valid_from` nor `embedding` is identity-bearing and neither
    /// can invalidate the signature. That is also what makes them backfillable
    /// on nodes already in a store, in place, with no change of id.
    ///
    /// `embedding` MUST come from the same model the tenant's other nodes were
    /// built with. The index skips vectors from a mismatched model, so a wrong
    /// space is indistinguishable from no vector at all.
    pub fn assert_node_with(
        &self,
        repo: &str,
        kind: NodeKind,
        content: &str,
        valid_from_ms: u64,
        observed_at_ms: u64,
        embedding: Option<mem_core::VersionedVector>,
    ) -> MemoryNode {
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
            valid_from: valid_from_ms,
            valid_to: None,
            observed_at: observed_at_ms,
            trust_tier: TrustTier::AgentAsserted,
            signature: None,
            embedding,
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

    /// Build a signed **proposal** edge (MEM-S4 WP-4.1): quarantined at the
    /// `InferredAdvisory` tier — advisory until a `confirm` promotes it, so a
    /// model's output can never be load-bearing on arrival (anti-poisoning,
    /// R1). `method` records who inferred it (`Nlp` / `Analogy`); `evidence`
    /// is the human-auditable why.
    pub fn propose_edge(
        &self,
        from: ContentHash,
        to: ContentHash,
        kind: EdgeKind,
        method: EdgeMethod,
        evidence: Option<String>,
        now_ms: u64,
    ) -> Edge {
        use ed25519_dalek::Signer;
        let mut edge = Edge {
            from,
            to,
            kind,
            plane: Plane::Asserted,
            trust_tier: TrustTier::InferredAdvisory,
            provenance: EdgeProvenance {
                method,
                asserter: self.pubkey_hex.clone(),
                at: now_ms,
                evidence,
            },
            confidence: vec![BelnapValue::True],
            quarantined: true,
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
/// fails (no partial poison). Content-addressed → idempotent.
///
/// `caller` is the author identity (ed25519 pubkey hex, i.e. the value an
/// [`Asserter`] writes into `author` / `provenance.asserter`) of the principal
/// performing the merge, or `None` for a caller with no signing identity.
///
/// **Existing ids are never overwritten last-writer-wins (PBA-L6b-002).** The
/// signature over a node covers only its content id, and over an edge only
/// `from‖to‖kind`, so the advisory fields (status, valid_to, confidence,
/// embedding, trust tier, anchors; edge quarantine / provenance) are unsigned.
/// For an id already in the store:
///   * a node is folded with the monotone CRDT join ([`join::merge_node`], the
///     same rule federation sync uses). If the join changes nothing (identical or
///     stale copy) it is a no-op; if it WOULD change the stored node, the caller
///     must be the node's author, else the whole diff is refused
///     ([`AssertError::NotAuthor`]). An author can therefore retire their own
///     claim but never resurrect a superseded one.
///   * an edge may never be promoted from quarantined to load-bearing by a diff
///     ([`AssertError::EdgePromotion`]; use `confirm_edge`), and any other change
///     to a stored edge requires the caller to be its asserter. A quarantined
///     copy over a confirmed edge stays a no-op (MEM-B-009).
///
/// All checks run before any write, so a refused diff writes nothing.
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
    caller: Option<&str>,
) -> Result<MergeReport, AssertError> {
    diff.verify()?;
    let is_caller = |who: &str| caller == Some(who);

    // ---- nodes: new ids land as-is; existing ids take the monotone join ----
    let mut to_write: Vec<MemoryNode> = Vec::with_capacity(diff.nodes.len());
    for n in &diff.nodes {
        let id = n.compute_id();
        match store.get_node(&id)? {
            None => to_write.push(n.clone()),
            Some(stored) => {
                let (joined, changed, _) = join::merge_node(&stored, n);
                if !changed {
                    continue; // identical or stale copy — nothing to do
                }
                if !is_caller(&stored.author) {
                    return Err(AssertError::NotAuthor(format!("node {}", &id.to_hex()[..12])));
                }
                to_write.push(joined);
            }
        }
    }

    // ---- edges: no promotion by diff; changes only by the asserter ----
    for e in &diff.edges {
        let stored = store.out_edges(&e.from)?.into_iter().find(|x| x.to == e.to && x.kind == e.kind);
        let Some(stored) = stored else { continue };
        let label = || format!("edge {}->{} {:?}", &e.from.to_hex()[..10], &e.to.to_hex()[..10], e.kind);
        if stored.quarantined && !e.quarantined {
            return Err(AssertError::EdgePromotion(label()));
        }
        if e.quarantined && !stored.quarantined {
            continue; // MEM-B-009: the store no-ops a demoting write
        }
        if stored != *e && !is_caller(&stored.provenance.asserter) {
            return Err(AssertError::NotAuthor(label()));
        }
    }

    let (supersessions, plain): (Vec<&Edge>, Vec<&Edge>) = diff
        .edges
        .iter()
        .partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);
    let plain: Vec<Edge> = plain.into_iter().cloned().collect();
    store.commit(&to_write, &plain)?;
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
    fn fwa_c10_04_authorship_is_bound_to_principal_not_gateway() {
        // One gateway key, two principals → two DISTINCT, deterministic authors,
        // each different from the raw gateway author, and `blame()` names the actor.
        let gateway = SigningKey::from_bytes(&[9u8; 32]);
        let gateway_author = Asserter::new(gateway.clone());

        let alice = Asserter::for_principal(&gateway, "did:alice");
        let bob = Asserter::for_principal(&gateway, "did:bob");

        assert_ne!(alice.pubkey_hex(), bob.pubkey_hex(), "distinct principals → distinct authors");
        assert_ne!(alice.pubkey_hex(), gateway_author.pubkey_hex(), "principal author != gateway key");
        assert_ne!(bob.pubkey_hex(), gateway_author.pubkey_hex());

        // Deterministic: re-deriving the same principal yields the same author
        // (preserves content-addressing / idempotent re-merge).
        let alice2 = Asserter::for_principal(&gateway, "did:alice");
        assert_eq!(alice.pubkey_hex(), alice2.pubkey_hex(), "same principal → stable author");

        // `blame()` now identifies the actual actor and the node still verifies.
        let n = alice.assert_node("r", NodeKind::Rationale, "alice's claim", 1);
        assert!(verify_node(&n).is_ok());
        assert_eq!(blame(&n).author, alice.pubkey_hex(), "blame names the principal, not the gateway");
        assert_ne!(blame(&n).author, gateway_author.pubkey_hex());
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
        let report = apply_diff(&store, &restored, None).unwrap();
        assert_eq!(report.nodes, 2);
        assert_eq!(report.edges, 1);
        assert_eq!(store.node_count().unwrap(), 2);

        // idempotent re-merge
        apply_diff(&store, &restored, None).unwrap();
        assert_eq!(store.node_count().unwrap(), 2);
    }

    #[test]
    fn proposed_edge_is_quarantined_signed_and_advisory() {
        let a = asserter(7);
        let n1 = a.assert_node("r", NodeKind::Rationale, "anchor", 1);
        let n2 = a.assert_node("r", NodeKind::Rationale, "peer", 1);
        let e = a.propose_edge(
            n1.compute_id(),
            n2.compute_id(),
            EdgeKind::AnalogousTo,
            EdgeMethod::Analogy,
            Some("cosine 0.81, structure 3/4".into()),
            2,
        );
        assert!(e.quarantined, "proposals start quarantined (R1)");
        assert_eq!(e.trust_tier, TrustTier::InferredAdvisory);
        assert_eq!(e.provenance.method, EdgeMethod::Analogy);
        assert!(verify_edge(&e).is_ok(), "proposal passes the FUA-02 write boundary");
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
        apply_diff(&store, &d1, None).unwrap();

        // Session 2 supersedes it.
        let new = a.assert_node("r", NodeKind::Rationale, "X was wrong; it is Z", 2);
        let e = a.assert_edge(new.compute_id(), old_id, EdgeKind::Supersedes, 2);
        let mut d2 = MemoryDiff::new(a.pubkey_hex(), 2);
        d2.add_node(new);
        d2.add_edge(e);
        let report = apply_diff(&store, &d2, None).unwrap();
        assert_eq!(report.superseded, 1);

        let old_now = store.get_node(&old_id).unwrap().unwrap();
        assert_eq!(old_now.status, Status::Superseded);
        assert_eq!(old_now.valid_to, Some(2));

        // Idempotent re-merge: no second transition.
        assert_eq!(apply_diff(&store, &d2, None).unwrap().superseded, 0);
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

        let err = apply_diff(&store, &diff, None).unwrap_err();
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
        assert!(apply_diff(&store, &diff, None).is_err());
        assert_eq!(store.node_count().unwrap(), 0, "no partial application of a forged diff");
    }

    // ---- PBA-L6b-002: existing ids are joined monotonically, author-gated ----

    fn stored_with(n: &MemoryNode) -> MemoryDagStore<MemoryNode> {
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        store.put_node(n).unwrap();
        store
    }

    /// A non-author cannot change ANY unsigned advisory field of another
    /// principal's stored node; the diff is refused and nothing is written.
    #[test]
    fn pba_l6b_002_non_author_cannot_change_existing_node() {
        let bob = asserter(10);
        let mallory = asserter(11);
        let victim = bob.assert_node("r", NodeKind::Adr, "bob's ADR", 5);
        type Tweak = Box<dyn Fn(&mut MemoryNode)>;
        let tweaks: Vec<Tweak> = vec![
            Box::new(|n| n.status = Status::Archived),
            Box::new(|n| n.confidence = vec![BelnapValue::False]),
            Box::new(|n| n.valid_to = Some(1)),
            Box::new(|n| n.embedding = Some(mem_core::VersionedVector { model: "m".into(), data: vec![0.0] })),
            Box::new(|n| n.trust_tier = TrustTier::InferredAdvisory),
        ];
        for (i, tweak) in tweaks.iter().enumerate() {
            let store = stored_with(&victim);
            let mut forged = victim.clone();
            tweak(&mut forged);
            let fresh = mallory.assert_node("r", NodeKind::Rationale, &format!("mallory's own note {i}"), 6);
            let mut diff = MemoryDiff::new(mallory.pubkey_hex(), 6);
            diff.add_node(fresh.clone());
            diff.add_node(forged);
            let err = apply_diff(&store, &diff, Some(mallory.pubkey_hex())).unwrap_err();
            assert!(matches!(err, AssertError::NotAuthor(_)), "tweak {i}: got {err:?}");
            assert_eq!(store.get_node(&victim.compute_id()).unwrap().unwrap(), victim, "tweak {i}: victim unchanged");
            assert!(store.get_node(&fresh.compute_id()).unwrap().is_none(), "tweak {i}: refused diff writes nothing");
            // No signing identity at all is not the author either.
            assert!(matches!(apply_diff(&store, &diff, None), Err(AssertError::NotAuthor(_))));
        }
    }

    /// The author may retire their own claim (monotone), and the stored node is
    /// the JOIN — a later stale copy cannot resurrect it (no LWW, even for the author).
    #[test]
    fn pba_l6b_002_author_update_is_a_monotone_join() {
        let bob = asserter(12);
        let n = bob.assert_node("r", NodeKind::Adr, "bob's ADR", 5);
        let store = stored_with(&n);
        let mut retired = n.clone();
        retired.status = Status::Archived;
        retired.valid_to = Some(9);
        let mut d = MemoryDiff::new(bob.pubkey_hex(), 9);
        d.add_node(retired);
        apply_diff(&store, &d, Some(bob.pubkey_hex())).unwrap();
        let now = store.get_node(&n.compute_id()).unwrap().unwrap();
        assert_eq!(now.status, Status::Archived);
        assert_eq!(now.valid_to, Some(9));

        // Author re-submits the original Active copy: the join keeps Archived.
        let mut stale = MemoryDiff::new(bob.pubkey_hex(), 10);
        stale.add_node(n.clone());
        apply_diff(&store, &stale, Some(bob.pubkey_hex())).unwrap();
        assert_eq!(store.get_node(&n.compute_id()).unwrap().unwrap().status, Status::Archived, "no resurrection");
        // …and so is a NON-author's stale copy: a no-op, not an error.
        apply_diff(&store, &stale, Some(asserter(13).pubkey_hex())).unwrap();
        assert_eq!(store.get_node(&n.compute_id()).unwrap().unwrap().status, Status::Archived);
    }

    /// Identical re-merge by anyone is a no-op success (the handoff flow).
    #[test]
    fn pba_l6b_002_identical_remerge_by_non_author_is_ok() {
        let bob = asserter(14);
        let n = bob.assert_node("r", NodeKind::Rationale, "handoff", 1);
        let store = stored_with(&n);
        let mut d = MemoryDiff::new(bob.pubkey_hex(), 1);
        d.add_node(n.clone());
        assert!(apply_diff(&store, &d, Some(asserter(15).pubkey_hex())).is_ok());
        assert!(apply_diff(&store, &d, None).is_ok());
        assert_eq!(store.get_node(&n.compute_id()).unwrap().unwrap(), n);
    }

    /// Edge quarantine is unsigned: a diff can never promote a stored proposal —
    /// not even one re-submitted by its own asserter (confirm_edge is the path).
    #[test]
    fn pba_l6b_002_diff_cannot_promote_quarantined_edge() {
        let bob = asserter(16);
        let a = bob.assert_node("r", NodeKind::Rationale, "a", 1);
        let b = bob.assert_node("r", NodeKind::Rationale, "b", 1);
        let store = stored_with(&a);
        store.put_node(&b).unwrap();
        let p = bob.propose_edge(a.compute_id(), b.compute_id(), EdgeKind::AnalogousTo, EdgeMethod::Nlp, None, 2);
        store.add_edge(&p).unwrap();
        let mut promoted = p.clone();
        promoted.quarantined = false;
        let mut d = MemoryDiff::new(bob.pubkey_hex(), 3);
        d.add_edge(promoted);
        for who in [Some(bob.pubkey_hex()), Some(asserter(17).pubkey_hex()), None] {
            assert!(matches!(apply_diff(&store, &d, who), Err(AssertError::EdgePromotion(_))));
        }
        assert!(store.out_edges(&a.compute_id()).unwrap()[0].quarantined, "still a proposal");

        // Re-submitting the proposal unchanged is fine for anyone.
        let mut same = MemoryDiff::new(bob.pubkey_hex(), 3);
        same.add_edge(p.clone());
        assert!(apply_diff(&store, &same, Some(asserter(18).pubkey_hex())).is_ok());
    }

    /// A non-author cannot re-attribute (re-sign / re-provenance) a stored edge;
    /// its asserter can re-state it.
    #[test]
    fn pba_l6b_002_non_asserter_cannot_change_stored_edge() {
        let bob = asserter(19);
        let mallory = asserter(20);
        let a = bob.assert_node("r", NodeKind::Rationale, "a", 1);
        let b = bob.assert_node("r", NodeKind::Rationale, "b", 1);
        let store = stored_with(&a);
        store.put_node(&b).unwrap();
        let e = bob.assert_edge(a.compute_id(), b.compute_id(), EdgeKind::References, 2);
        store.add_edge(&e).unwrap();
        let hers = mallory.assert_edge(a.compute_id(), b.compute_id(), EdgeKind::References, 3);
        let mut d = MemoryDiff::new(mallory.pubkey_hex(), 3);
        d.add_edge(hers);
        assert!(matches!(apply_diff(&store, &d, Some(mallory.pubkey_hex())), Err(AssertError::NotAuthor(_))));
        assert_eq!(store.out_edges(&a.compute_id()).unwrap()[0], e, "bob's edge intact");

        let restated = bob.assert_edge(a.compute_id(), b.compute_id(), EdgeKind::References, 4);
        let mut d2 = MemoryDiff::new(bob.pubkey_hex(), 4);
        d2.add_edge(restated.clone());
        apply_diff(&store, &d2, Some(bob.pubkey_hex())).unwrap();
        assert_eq!(store.out_edges(&a.compute_id()).unwrap()[0], restated);
    }

    /// A quarantined copy over a confirmed edge stays a silent no-op (MEM-B-009),
    /// not a NotAuthor error — a stale proposal must not fail a handoff.
    #[test]
    fn pba_l6b_002_stale_quarantined_copy_over_confirmed_edge_is_noop() {
        let bob = asserter(21);
        let a = bob.assert_node("r", NodeKind::Rationale, "a", 1);
        let b = bob.assert_node("r", NodeKind::Rationale, "b", 1);
        let store = stored_with(&a);
        store.put_node(&b).unwrap();
        let confirmed = bob.assert_edge(a.compute_id(), b.compute_id(), EdgeKind::References, 2);
        store.add_edge(&confirmed).unwrap();
        let p = asserter(22).propose_edge(a.compute_id(), b.compute_id(), EdgeKind::References, EdgeMethod::Nlp, None, 3);
        let mut d = MemoryDiff::new(asserter(22).pubkey_hex(), 3);
        d.add_edge(p);
        apply_diff(&store, &d, Some(asserter(22).pubkey_hex())).unwrap();
        assert_eq!(store.out_edges(&a.compute_id()).unwrap()[0], confirmed);
    }

    /// PBA-L6b-002 class tripwire: the diff write path must never commit the
    /// diff's raw nodes (last-writer-wins over an existing id), must fold existing
    /// ids through the shared monotone join and the author gate, and the MCP
    /// merge_diff tool must hand apply_diff the caller's real identity.
    #[test]
    fn pba_l6b_002_tripwire_apply_diff_is_join_and_author_gated() {
        let src = include_str!("lib.rs");
        let raw_commit = concat!("commit(&diff", ".nodes");
        assert!(!src.contains(raw_commit), "PBA-L6b-002: apply_diff commits raw diff nodes (LWW overwrite)");
        let start = src.find("pub fn apply_diff(").expect("apply_diff");
        let body = &src[start..start + src[start..].find("\n}\n").expect("end of apply_diff")];
        for must in ["caller: Option<&str>", "join::merge_node(", "AssertError::NotAuthor", "AssertError::EdgePromotion"] {
            assert!(body.contains(must), "PBA-L6b-002: apply_diff lost `{must}`");
        }
        let mcp = include_str!("../../mem-mcp/src/lib.rs");
        assert!(
            mcp.contains("apply_diff(self.store, &diff, caller.as_deref())"),
            "PBA-L6b-002: memory.merge_diff must pass the caller's author identity to apply_diff"
        );
    }

    /// Mutation-hardening: the stored-edge lookup matches on BOTH `to` and
    /// `kind` — a different edge from the same node (same kind, other target) is
    /// not "the stored copy" of a new edge.
    #[test]
    fn pba_l6b_002_edge_lookup_is_by_full_key() {
        let bob = asserter(23);
        let mallory = asserter(24);
        let a = bob.assert_node("r", NodeKind::Rationale, "a", 1);
        let b = bob.assert_node("r", NodeKind::Rationale, "b", 1);
        let c = bob.assert_node("r", NodeKind::Rationale, "c", 1);
        let store = stored_with(&a);
        store.put_node(&b).unwrap();
        store.put_node(&c).unwrap();
        let p = bob.propose_edge(a.compute_id(), b.compute_id(), EdgeKind::References, EdgeMethod::Nlp, None, 2);
        store.add_edge(&p).unwrap();
        // A NEW load-bearing edge a->c (same kind, other target) by someone else.
        let e = mallory.assert_edge(a.compute_id(), c.compute_id(), EdgeKind::References, 3);
        let mut d = MemoryDiff::new(mallory.pubkey_hex(), 3);
        d.add_edge(e);
        apply_diff(&store, &d, Some(mallory.pubkey_hex())).unwrap();
        // And a->b with another kind is also a new edge.
        let e2 = mallory.assert_edge(a.compute_id(), b.compute_id(), EdgeKind::Implements, 3);
        let mut d2 = MemoryDiff::new(mallory.pubkey_hex(), 3);
        d2.add_edge(e2);
        apply_diff(&store, &d2, Some(mallory.pubkey_hex())).unwrap();
        assert_eq!(store.out_edges(&a.compute_id()).unwrap().len(), 3);
    }
}
