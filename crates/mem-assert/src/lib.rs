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
    /// PBA-L6b-002 follow-up: relayed (non-author) first inserts whose unsigned
    /// embedding was dropped because it cannot be attributed to the author.
    pub unembedded: usize,
}

/// Largest clock skew tolerated on a relayed node's `observed_at` (5 minutes).
pub const RELAY_CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;

fn wall_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// PBA-L6b-002 follow-up (front-run): the advisory state an [`Asserter`] mints a
/// node with. A node inserted for the first time by someone OTHER than its author
/// (a relay) must be in exactly this shape — the signature covers only the id, so
/// any other status / valid_to / confidence / tier / anchors on a relayed copy is
/// unattributable and would land while still verifying as the author's.
fn is_as_asserted(n: &MemoryNode, now_ms: u64) -> bool {
    n.status == Status::Active
        && n.valid_to.is_none()
        && n.confidence == [BelnapValue::True]
        && n.trust_tier == TrustTier::AgentAsserted
        && n.anchors.is_empty()
        && n.valid_from <= n.observed_at
        && n.observed_at <= now_ms.saturating_add(RELAY_CLOCK_SKEW_MS)
}

/// Verify, then append a diff to a store. Rejected wholesale if any signature
/// fails (no partial poison). Content-addressed → idempotent.
///
/// `caller` is the author identity (ed25519 pubkey hex, i.e. the value an
/// [`Asserter`] writes into `author` / `provenance.asserter`) of the principal
/// performing the merge, or `None` for a caller with no signing identity.
///
/// The signature over a node covers only its content id, and over an edge only
/// `from‖to‖kind`, so the advisory fields (status, valid_to, confidence,
/// embedding, trust tier, anchors, valid_from/observed_at; edge quarantine /
/// provenance) are unsigned. Signing them would change what `compute_id` and the
/// edge key cover and re-address the store, so instead (PBA-L6b-002 and its R2
/// follow-ups) **only an item's author may set its advisory state**:
///   * **Existing node:** folded with the monotone CRDT join
///     ([`join::merge_node`], the rule federation sync uses). A join that changes
///     nothing is a no-op; one that WOULD change the node needs `caller ==
///     author`, else the whole diff is refused ([`AssertError::NotAuthor`]).
///   * **New node from a non-author (relay / front-run):** must carry the
///     as-asserted state ([`is_as_asserted`]), else refused. Its unsigned
///     `embedding` is dropped (a non-author copy never contributes one; the
///     author, or a backfill, supplies it) and counted in
///     [`MergeReport::unembedded`].
///   * **Supersession:** a non-quarantined `Supersedes` edge may only retire a
///     node whose author is the caller (extends the MEM-B-010 plane guard to
///     cross-author supersession on this path). Another principal's node is
///     retired through `confirm_edge` under a write grant, not a diff. A NEW
///     Supersedes *proposal* onto another principal's node must be timestamped
///     within [`RELAY_CLOCK_SKEW_MS`] of now, and an identical, already-stored
///     edge is a no-op (so relaying an applied handoff is not refused).
///   * **Relayed times:** a non-author copy's `valid_from` / `observed_at` are
///     floored at `now - RELAY_CLOCK_SKEW_MS` (no retroactive presence).
///   * **Existing edge:** promotion from quarantined to load-bearing, or any other
///     change, needs the caller to be the stored edge's asserter
///     ([`AssertError::EdgePromotion`] / [`AssertError::NotAuthor`]). A
///     quarantined copy over a confirmed edge stays a no-op (MEM-B-009).
///   * **New edge from a non-asserter:** only as a quarantined proposal; a relayed
///     load-bearing edge is refused (its unsigned quarantine flag cannot be
///     attributed — it may be someone's proposal with the flag cleared).
///
/// All checks run before any write, so a refused diff writes nothing.
///
/// Non-quarantined `Supersedes` edges are *applied*, not just stored (WP-1.4):
/// each one atomically writes the edge and transitions its target, guarded by
/// the cycle check. A rejected supersession (cycle/missing node) errors after
/// the diff's nodes and other edges have landed — safe, because everything is
/// idempotent: re-merging the same diff re-applies cleanly and the bad edge
/// stays rejected. Quarantined supersedes edges are stored as plain proposals.
///
/// Not gated here by design: `mem-sync` `merge_bundle` (replica merge between
/// trust-root-authorized peers) applies the same join without an author gate.
pub fn apply_diff(
    store: &MemoryDagStore<MemoryNode>,
    diff: &MemoryDiff,
    caller: Option<&str>,
) -> Result<MergeReport, AssertError> {
    diff.verify()?;
    let is_caller = |who: &str| caller == Some(who);
    let now = wall_now_ms();

    // ---- nodes ----
    let mut to_write: Vec<MemoryNode> = Vec::with_capacity(diff.nodes.len());
    let mut unembedded = 0;
    for n in &diff.nodes {
        let id = n.compute_id();
        let own = is_caller(&n.author);
        // A non-author copy never contributes the unsigned embedding.
        let mut incoming = n.clone();
        if !own {
            incoming.embedding = None;
            // Pass 2: valid_from / observed_at are unsigned too. A non-author copy
            // may not place the node earlier than "about now" (retroactive
            // presence in as_of snapshots); the author supplies the real times.
            let floor = now.saturating_sub(RELAY_CLOCK_SKEW_MS);
            incoming.valid_from = incoming.valid_from.max(floor);
            incoming.observed_at = incoming.observed_at.max(floor);
        }
        match store.get_node(&id)? {
            None => {
                if !own {
                    if !is_as_asserted(n, now) {
                        return Err(AssertError::NotAuthor(format!(
                            "node {} (first insert by a non-author must carry the as-asserted state)",
                            &id.to_hex()[..12]
                        )));
                    }
                    if n.embedding.is_some() {
                        unembedded += 1;
                    }
                }
                to_write.push(incoming);
            }
            Some(stored) => {
                let (joined, changed, _) = join::merge_node(&stored, &incoming);
                if !changed {
                    continue; // identical or stale copy — nothing to do
                }
                if !own {
                    return Err(AssertError::NotAuthor(format!("node {}", &id.to_hex()[..12])));
                }
                to_write.push(joined);
            }
        }
    }

    // ---- edges ----
    let mut plain: Vec<Edge> = Vec::new();
    let mut supersessions: Vec<&Edge> = Vec::new();
    for e in &diff.edges {
        let label = || format!("edge {}->{} {:?}", &e.from.to_hex()[..10], &e.to.to_hex()[..10], e.kind);
        let stored = store.out_edges(&e.from)?.into_iter().find(|x| x.to == e.to && x.kind == e.kind);
        if stored.as_ref() == Some(e) {
            continue; // identical and already applied (incl. a Supersedes) — no-op
        }
        if let Some(stored) = &stored {
            let by_asserter = is_caller(&stored.provenance.asserter);
            if stored.quarantined && !e.quarantined && !by_asserter {
                return Err(AssertError::EdgePromotion(label()));
            }
            if e.quarantined && !stored.quarantined {
                continue; // MEM-B-009: the store no-ops a demoting write
            }
            if stored != e && !by_asserter {
                return Err(AssertError::NotAuthor(label()));
            }
        }
        let target_author = if e.kind == EdgeKind::Supersedes {
            match diff.nodes.iter().find(|n| n.compute_id() == e.to) {
                Some(n) => Some(n.author.clone()),
                None => store.get_node(&e.to)?.map(|n| n.author),
            }
        } else {
            None
        };
        // Pass 2: a NEW Supersedes proposal onto another principal's node must be
        // current — a backdated proposal is a primed backdate for a later confirm.
        if e.kind == EdgeKind::Supersedes && e.quarantined && stored.is_none() {
            if let Some(author) = &target_author {
                if !is_caller(author) && e.provenance.at.abs_diff(now) > RELAY_CLOCK_SKEW_MS {
                    return Err(AssertError::NotAuthor(format!(
                        "{} (supersedes proposal on another principal's node must be timestamped now)",
                        label()
                    )));
                }
            }
        }
        if e.kind == EdgeKind::Supersedes && !e.quarantined {
            // Cross-author supersession: only the target's author may retire it.
            if let Some(author) = target_author {
                if !is_caller(&author) {
                    return Err(AssertError::NotAuthor(format!("{} (supersedes another principal's node)", label())));
                }
            }
            supersessions.push(e);
            continue;
        }
        // A NEW load-bearing edge relayed by a non-asserter: its unsigned
        // quarantine flag cannot be attributed (it may be someone's proposal with
        // the flag cleared), so it is refused. Relaying a proposal is fine.
        if stored.is_none() && !e.quarantined && !is_caller(&e.provenance.asserter) {
            return Err(AssertError::NotAuthor(format!("{} (relayed load-bearing edge)", label())));
        }
        plain.push(e.clone());
    }

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
        unembedded,
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
        let report = apply_diff(&store, &restored, Some(a.pubkey_hex())).unwrap();
        assert_eq!(report.nodes, 2);
        assert_eq!(report.edges, 1);
        assert_eq!(store.node_count().unwrap(), 2);

        // idempotent re-merge
        apply_diff(&store, &restored, Some(a.pubkey_hex())).unwrap();
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
        apply_diff(&store, &d1, Some(a.pubkey_hex())).unwrap();

        // Session 2 supersedes it.
        let new = a.assert_node("r", NodeKind::Rationale, "X was wrong; it is Z", 2);
        let e = a.assert_edge(new.compute_id(), old_id, EdgeKind::Supersedes, 2);
        let mut d2 = MemoryDiff::new(a.pubkey_hex(), 2);
        d2.add_node(new);
        d2.add_edge(e);
        let report = apply_diff(&store, &d2, Some(a.pubkey_hex())).unwrap();
        assert_eq!(report.superseded, 1);

        let old_now = store.get_node(&old_id).unwrap().unwrap();
        assert_eq!(old_now.status, Status::Superseded);
        assert_eq!(old_now.valid_to, Some(2));

        // Idempotent re-merge: no second transition.
        assert_eq!(apply_diff(&store, &d2, Some(a.pubkey_hex())).unwrap().superseded, 0);
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

        let err = apply_diff(&store, &diff, Some(a.pubkey_hex())).unwrap_err();
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
        // A non-author's embedding is never applied: the copy is a no-op.
        let store = stored_with(&victim);
        let mut f = victim.clone();
        f.embedding = Some(mem_core::VersionedVector { model: "m".into(), data: vec![0.0] });
        let mut diff = MemoryDiff::new(mallory.pubkey_hex(), 6);
        diff.add_node(f);
        apply_diff(&store, &diff, Some(mallory.pubkey_hex())).unwrap();
        assert_eq!(store.get_node(&victim.compute_id()).unwrap().unwrap(), victim, "forged embedding ignored");
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

    /// Edge quarantine is unsigned: a diff can never promote someone else's stored
    /// proposal (confirm_edge is the path); only its own asserter may.
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
        for who in [Some(asserter(17).pubkey_hex()), None] {
            assert!(matches!(apply_diff(&store, &d, who), Err(AssertError::EdgePromotion(_))));
        }
        assert!(store.out_edges(&a.compute_id()).unwrap()[0].quarantined, "still a proposal");
        // Its own asserter may state it as load-bearing (they could assert it anew).
        let s_own = stored_with(&a);
        s_own.put_node(&b).unwrap();
        s_own.add_edge(&p).unwrap();
        apply_diff(&s_own, &d, Some(bob.pubkey_hex())).unwrap();
        assert!(!s_own.out_edges(&a.compute_id()).unwrap()[0].quarantined);

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

    // ---- R2 verifier follow-ups: cross-author supersession, front-run ----

    /// Non-author supersession of an existing node is refused before any write;
    /// the author (and only the author) may supersede their own node.
    #[test]
    fn pba_l6b_002_supersession_is_author_gated() {
        let bob = asserter(30);
        let mallory = asserter(31);
        let old = bob.assert_node("r", NodeKind::Rationale, "bob's claim", 1_000);
        let store = stored_with(&old);
        let mine = mallory.assert_node("r", NodeKind::Rationale, "mallory's override", 2_000);
        let e = mallory.assert_edge(mine.compute_id(), old.compute_id(), EdgeKind::Supersedes, 1);
        let mut d = MemoryDiff::new(mallory.pubkey_hex(), 2);
        d.add_node(mine.clone());
        d.add_edge(e);
        for who in [Some(mallory.pubkey_hex()), None] {
            assert!(matches!(apply_diff(&store, &d, who), Err(AssertError::NotAuthor(_))), "caller {who:?}");
        }
        assert_eq!(store.get_node(&old.compute_id()).unwrap().unwrap(), old, "untouched");
        assert!(store.get_node(&mine.compute_id()).unwrap().is_none(), "refused diff writes nothing");
        // A supersedes edge onto a node that arrives in the SAME diff is gated on
        // that node's author too.
        let ghost = bob.assert_node("r", NodeKind::Rationale, "bob's other claim", 1_000);
        let e2 = mallory.assert_edge(mine.compute_id(), ghost.compute_id(), EdgeKind::Supersedes, 5);
        let mut d2 = MemoryDiff::new(mallory.pubkey_hex(), 2);
        d2.add_node(mine.clone());
        d2.add_node(ghost.clone());
        d2.add_edge(e2);
        assert!(matches!(apply_diff(&store, &d2, Some(mallory.pubkey_hex())), Err(AssertError::NotAuthor(_))));
        // The author can.
        let newer = bob.assert_node("r", NodeKind::Rationale, "bob's revision", 3_000);
        let e3 = bob.assert_edge(newer.compute_id(), old.compute_id(), EdgeKind::Supersedes, 3_000);
        let mut d3 = MemoryDiff::new(bob.pubkey_hex(), 3);
        d3.add_node(newer);
        d3.add_edge(e3);
        assert_eq!(apply_diff(&store, &d3, Some(bob.pubkey_hex())).unwrap().superseded, 1);
        // A QUARANTINED supersedes proposal by a non-author is still just a proposal.
        // (timestamped now: a backdated cross-author proposal is refused, pass 2)
        let p = mallory.propose_edge(mine.compute_id(), ghost.compute_id(), EdgeKind::Supersedes, EdgeMethod::Nlp, None, super::wall_now_ms());
        let mut d4 = MemoryDiff::new(mallory.pubkey_hex(), 4);
        d4.add_node(mine.clone());
        d4.add_edge(p);
        let s2 = stored_with(&ghost);
        assert!(apply_diff(&s2, &d4, Some(mallory.pubkey_hex())).is_ok());
        assert_eq!(s2.get_node(&ghost.compute_id()).unwrap().unwrap().status, Status::Active);
    }

    /// Front-run: a first insert of someone else's node must carry the
    /// as-asserted advisory state; each forged field is refused.
    #[test]
    fn pba_l6b_002_non_author_first_insert_must_be_as_asserted() {
        let bob = asserter(32);
        let mallory = asserter(33);
        // Timestamped "now": a relayed copy's times are floored at now - skew (pass 2).
        let t = super::wall_now_ms();
        let n = bob.assert_node_with("r", NodeKind::Adr, "bob's pending", t, t, None);
        type Tweak = Box<dyn Fn(&mut MemoryNode)>;
        let tweaks: Vec<(&str, Tweak)> = vec![
            ("status", Box::new(|n| n.status = Status::Archived)),
            ("valid_to", Box::new(|n| n.valid_to = Some(1))),
            ("confidence", Box::new(|n| n.confidence = vec![BelnapValue::False])),
            ("tier", Box::new(|n| n.trust_tier = TrustTier::InferredAdvisory)),
            ("anchors", Box::new(|n| n.anchors = vec![mem_core::CodeAnchor { repo: "r".into(), path: "p".into(), symbol: None, line_start: 1, line_end: 2 }])),
            ("valid_from>observed_at", Box::new(|n| n.valid_from = n.observed_at + 1)),
            ("observed_at in the future", Box::new(|n| { n.observed_at = u64::MAX; n.valid_from = u64::MAX })),
        ];
        for (label, t) in &tweaks {
            let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
            let mut f = n.clone();
            t(&mut f);
            let mut d = MemoryDiff::new(mallory.pubkey_hex(), 1);
            d.add_node(f);
            let r = apply_diff(&store, &d, Some(mallory.pubkey_hex()));
            assert!(matches!(r, Err(AssertError::NotAuthor(_))), "{label}: {r:?}");
            assert!(store.get_node(&n.compute_id()).unwrap().is_none(), "{label}: nothing written");
        }
        // The as-asserted copy is accepted (relay), and so is anything from the author.
        let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let mut d = MemoryDiff::new(mallory.pubkey_hex(), 1);
        d.add_node(n.clone());
        apply_diff(&store, &d, Some(mallory.pubkey_hex())).unwrap();
        assert_eq!(store.get_node(&n.compute_id()).unwrap().unwrap(), n);
        let store2: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let mut own = n.clone();
        own.status = Status::Archived;
        let mut d2 = MemoryDiff::new(bob.pubkey_hex(), 1);
        d2.add_node(own);
        apply_diff(&store2, &d2, Some(bob.pubkey_hex())).unwrap();
        assert_eq!(store2.get_node(&n.compute_id()).unwrap().unwrap().status, Status::Archived);
    }

    /// Front-run of the unsigned embedding: a relayed first insert keeps no
    /// embedding (it cannot be attributed to the author); the author's own
    /// insert keeps it.
    #[test]
    fn pba_l6b_002_relayed_first_insert_drops_unattributable_embedding() {
        let bob = asserter(34);
        let emb = Some(mem_core::VersionedVector { model: "m".into(), data: vec![0.0; 4] });
        let n = bob.assert_node_with("r", NodeKind::Adr, "bob's pending", 1_000, 1_000, emb.clone());
        let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let mut d = MemoryDiff::new("relay", 1);
        d.add_node(n.clone());
        let r = apply_diff(&store, &d, Some(asserter(35).pubkey_hex())).unwrap();
        assert_eq!(r.unembedded, 1);
        assert_eq!(store.get_node(&n.compute_id()).unwrap().unwrap().embedding, None);
        // The author then supplies it (a join change by the author).
        apply_diff(&store, &d, Some(bob.pubkey_hex())).unwrap();
        assert_eq!(store.get_node(&n.compute_id()).unwrap().unwrap().embedding, emb);
        let store2: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        assert_eq!(apply_diff(&store2, &d, Some(bob.pubkey_hex())).unwrap().unembedded, 0);
        assert_eq!(store2.get_node(&n.compute_id()).unwrap().unwrap().embedding, emb);
    }

    /// A relayed NEW load-bearing edge (not by the caller) is refused — it could
    /// be someone's proposal with the unsigned quarantine flag cleared; relaying
    /// the proposal as a proposal is fine.
    #[test]
    fn pba_l6b_002_relayed_load_bearing_edge_is_refused() {
        let bob = asserter(36);
        let a = bob.assert_node("r", NodeKind::Rationale, "a", 1);
        let b = bob.assert_node("r", NodeKind::Rationale, "b", 1);
        let store = stored_with(&a);
        store.put_node(&b).unwrap();
        let p = bob.propose_edge(a.compute_id(), b.compute_id(), EdgeKind::References, EdgeMethod::Nlp, None, 2);
        let mut laundered = p.clone();
        laundered.quarantined = false;
        let mut d = MemoryDiff::new("relay", 2);
        d.add_edge(laundered.clone());
        assert!(matches!(apply_diff(&store, &d, Some(asserter(37).pubkey_hex())), Err(AssertError::NotAuthor(_))));
        assert!(store.out_edges(&a.compute_id()).unwrap().is_empty());
        let mut d2 = MemoryDiff::new("relay", 2);
        d2.add_edge(p.clone());
        apply_diff(&store, &d2, Some(asserter(37).pubkey_hex())).unwrap();
        apply_diff(&store, &d2, Some(asserter(37).pubkey_hex())).unwrap(); // idempotent
        assert!(store.out_edges(&a.compute_id()).unwrap()[0].quarantined);
        // The asserter's own load-bearing copy is accepted.
        apply_diff(&store, &d, Some(bob.pubkey_hex())).unwrap();
        assert!(!store.out_edges(&a.compute_id()).unwrap()[0].quarantined);
    }

    // ---- R2 verifier pass 2 ----

    /// Front-run residual: a relay may not give someone else's unstored node an
    /// early valid_from / observed_at (retroactive presence in as_of snapshots);
    /// re-relaying stays idempotent and the author can still set the real value.
    #[test]
    fn pba_l6b_002_p2_relayed_first_insert_times_are_clamped_to_now() {
        let bob = asserter(40);
        let real = bob.assert_node_with("r", NodeKind::Adr, "bob real", 1_000_000, 1_000_000, None);
        let mut early = real.clone();
        early.valid_from = 0;
        early.observed_at = 0;
        let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let relay = asserter(41);
        let mut d = MemoryDiff::new("relay", 1);
        d.add_node(early);
        let floor = super::wall_now_ms() - RELAY_CLOCK_SKEW_MS;
        apply_diff(&store, &d, Some(relay.pubkey_hex())).unwrap();
        let s1 = store.get_node(&real.compute_id()).unwrap().unwrap();
        assert!(s1.valid_from >= floor && s1.observed_at >= floor, "PBA-L6b-002 p2: relayed node backdated to {}/{}", s1.valid_from, s1.observed_at);
        apply_diff(&store, &d, Some(relay.pubkey_hex())).expect("re-relay is a no-op");
        let mut own = MemoryDiff::new(bob.pubkey_hex(), 2);
        own.add_node(real.clone());
        apply_diff(&store, &own, Some(bob.pubkey_hex())).unwrap();
        assert_eq!(store.get_node(&real.compute_id()).unwrap().unwrap().valid_from, 1_000_000, "author sets the real time");
    }

    /// Regression: relaying an identical, already-applied handoff that contains
    /// the author's own Supersedes edge is a no-op, not a refusal.
    #[test]
    fn pba_l6b_002_p2_relayed_already_applied_supersession_is_noop() {
        let bob = asserter(42);
        let a = bob.assert_node("r", NodeKind::Adr, "bob a", 1_000);
        let b = bob.assert_node("r", NodeKind::Adr, "bob b", 2_000);
        let sup = bob.assert_edge(b.compute_id(), a.compute_id(), EdgeKind::Supersedes, 2_000);
        let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let mut d = MemoryDiff::new(bob.pubkey_hex(), 2);
        d.add_node(a.clone());
        d.add_node(b.clone());
        d.add_edge(sup.clone());
        apply_diff(&store, &d, Some(bob.pubkey_hex())).unwrap();
        let stored_a = store.get_node(&a.compute_id()).unwrap().unwrap();
        let mut relay = MemoryDiff::new(bob.pubkey_hex(), 3);
        relay.add_node(stored_a);
        relay.add_node(b.clone());
        relay.add_edge(sup);
        apply_diff(&store, &relay, Some(asserter(43).pubkey_hex())).expect("identical applied handoff relays as a no-op");
    }

    /// A NEW quarantined Supersedes proposal onto another principal's node must
    /// carry a timestamp within the clock-skew window (no backdated proposals to
    /// confirm later).
    #[test]
    fn pba_l6b_002_p2_backdated_cross_author_proposal_is_refused() {
        let bob = asserter(44);
        let mal = asserter(45);
        let victim = bob.assert_node("r", NodeKind::Adr, "bob", 1_000);
        let store = stored_with(&victim);
        let mine = mal.assert_node("r", NodeKind::Rationale, "mine", 2_000);
        let old = mal.propose_edge(mine.compute_id(), victim.compute_id(), EdgeKind::Supersedes, EdgeMethod::Nlp, None, 1);
        let mut d = MemoryDiff::new(mal.pubkey_hex(), 1);
        d.add_node(mine.clone());
        d.add_edge(old);
        assert!(matches!(apply_diff(&store, &d, Some(mal.pubkey_hex())), Err(AssertError::NotAuthor(_))));
        let fresh = mal.propose_edge(mine.compute_id(), victim.compute_id(), EdgeKind::Supersedes, EdgeMethod::Nlp, None, super::wall_now_ms());
        let mut d2 = MemoryDiff::new(mal.pubkey_hex(), 1);
        d2.add_node(mine);
        d2.add_edge(fresh);
        apply_diff(&store, &d2, Some(mal.pubkey_hex())).expect("a current proposal relays");
    }
}
