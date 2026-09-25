use super::*;

use ed25519_dalek::SigningKey;
use mem_assert::Asserter;
use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};
use mem_core::{
    BelnapValue, CodeAnchor, EdgeMethod, EdgeProvenance, NodeKind, SourceRef, Status, TrustTier,
    VersionedVector, SCHEMA_VERSION,
};
use mem_store::kv::InMemoryKv;

fn store() -> MemoryDagStore<MemoryNode> {
    MemoryDagStore::new(Box::new(InMemoryKv::new()))
}

/// A signed grant authorizing Read (+ optionally Write) on the named repos, in
/// the SAME `repo:{repo}/memory` namespace `merge_bundle` checks against. Shared
/// with `transport.rs` tests.
pub(crate) fn grant_for(repos: &[&str], write: bool) -> CapabilityGrant {
    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let mut g = CapabilityGrant {
        id: "test-grant".into(),
        issuer: "did:test".into(),
        recipient: "agent:peer".into(),
        allowed_resources: repos
            .iter()
            .map(|r| ResourceScope {
                resource_id: format!("repo:{r}/memory"),
                can_read: true,
                can_write: write,
            })
            .collect(),
        policy: if write { PolicyProfile::Maintainer } else { PolicyProfile::ReadOnly },
        expires_at_ms: u64::MAX,
        revoked: false,
        delegation_chain: vec![],
        issuer_pubkey: vec![],
        signature: vec![],
    };
    g.sign_with(&sk);
    g
}

/// The ed25519 public key that signs [`grant_for`] — the trusted grant root the
/// federation merge anchors against (MEM-B-004). A grant not issued by this key
/// (e.g. one a peer self-signs) is refused by `merge_bundle`. Shared with
/// `transport.rs` tests.
pub(crate) fn trust_root() -> Vec<u8> {
    SigningKey::from_bytes(&[42u8; 32]).verifying_key().to_bytes().to_vec()
}

/// Test shim: most existing CRDT tests merge a tenant's own export back (trusted
/// local replica semantics) and predate the authz gate — route them through the
/// explicit trusted-local grant so their CRDT assertions are unchanged.
fn merge_bundle(
    store: &MemoryDagStore<MemoryNode>,
    bundle: &SyncBundle,
) -> Result<MergeOutcome, SyncError> {
    merge_bundle_trusted(store, bundle)
}

fn node(repo: &str, content: &str, confidence: Vec<BelnapValue>) -> MemoryNode {
    MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind: NodeKind::Commit,
        repo: repo.into(),
        author: "ingest".into(),
        source_ref: SourceRef::DagNative { key: content.into() },
        content: content.as_bytes().to_vec(),
        valid_from: 1,
        valid_to: None,
        observed_at: 1,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding: None,
        confidence,
        anchors: vec![],
        status: Status::Active,
    }
}

fn supersedes(from: &MemoryNode, to: &MemoryNode, at: u64) -> Edge {
    Edge {
        from: from.compute_id(),
        to: to.compute_id(),
        kind: EdgeKind::Supersedes,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "ingest".into(), at, evidence: None },
        confidence: vec![],
        quarantined: false,
        signature: None,
    }
}

/// The canonical comparable state of a tenant on a store.
fn fingerprint(s: &MemoryDagStore<MemoryNode>, repo: &str) -> String {
    tenant_root(s, repo).unwrap().to_hex()
}

#[test]
fn export_import_roundtrip_and_idempotence() {
    let a = store();
    let n1 = node("r", "first", vec![BelnapValue::True]);
    let n2 = node("r", "second", vec![]);
    a.commit(&[n1.clone(), n2.clone()], &[supersedes(&n2, &n1, 7)]).unwrap();
    a.apply_supersession(&supersedes(&n2, &n1, 7)).unwrap();

    let bundle = export_tenant(&a, "r", 100).unwrap();
    let b = store();
    let out = merge_bundle(&b, &bundle).unwrap();
    assert_eq!(out.nodes_added, 2);
    // The bundle's node already carries Superseded status, so the transition
    // arrives via the node merge; the edge lands with nothing left to do.
    assert_eq!(out.edges_added, 1);
    assert_eq!(out.superseded, 0, "no double transition");
    assert_eq!(
        b.get_node(&n1.compute_id()).unwrap().unwrap().status,
        Status::Superseded,
        "correction carried by the node state"
    );
    assert_eq!(fingerprint(&a, "r"), fingerprint(&b, "r"), "replica B converged to A");

    // Idempotence: merging the same bundle again changes nothing.
    let again = merge_bundle(&b, &bundle).unwrap();
    assert_eq!(again, MergeOutcome::default(), "second merge is a no-op: {again:?}");

    // Self-merge is also a no-op.
    let self_merge = merge_bundle(&a, &export_tenant(&a, "r", 200).unwrap()).unwrap();
    assert_eq!(self_merge, MergeOutcome::default());
}

#[test]
fn replicas_converge_and_contradictions_surface() {
    // Both replicas share one node but disagree on its confidence; each also
    // holds a node the other lacks.
    let shared_true = node("r", "shared claim", vec![BelnapValue::True]);
    let mut shared_false = shared_true.clone();
    shared_false.confidence = vec![BelnapValue::False]; // same id — confidence is advisory

    let a = store();
    a.put_node(&shared_true).unwrap();
    a.put_node(&node("r", "only in a", vec![])).unwrap();
    let b = store();
    b.put_node(&shared_false).unwrap();
    b.put_node(&node("r", "only in b", vec![])).unwrap();

    let from_a = export_tenant(&a, "r", 1).unwrap();
    let from_b = export_tenant(&b, "r", 1).unwrap();
    let out_a = merge_bundle(&a, &from_b).unwrap();
    let out_b = merge_bundle(&b, &from_a).unwrap();

    assert_eq!(fingerprint(&a, "r"), fingerprint(&b, "r"), "cross-merge converges");
    assert_eq!(out_a.contradictions, 1, "True ⊔ False surfaced as Both");
    assert_eq!(out_b.contradictions, 1);
    let merged = a.get_node(&shared_true.compute_id()).unwrap().unwrap();
    assert_eq!(merged.confidence, vec![BelnapValue::Both], "evidence combined, no silent winner");
    assert_eq!(a.all_nodes().unwrap().len(), 3);
}

#[test]
fn status_and_quarantine_merge_monotonically() {
    // A and B both know n1, n2. B locally superseded n1 and confirmed a
    // proposed edge; after merging B's bundle, A agrees on both.
    let n1 = node("r", "old understanding", vec![]);
    let n2 = node("r", "new understanding", vec![]);
    let a = store();
    let b = store();
    for s in [&a, &b] {
        s.put_node(&n1).unwrap();
        s.put_node(&n2).unwrap();
    }
    // A holds a quarantined proposal; B holds the same edge confirmed.
    let mut proposal = supersedes(&n2, &n1, 9);
    proposal.kind = EdgeKind::References;
    proposal.quarantined = true;
    a.add_edge(&proposal).unwrap();
    let mut confirmed = proposal.clone();
    confirmed.quarantined = false;
    b.add_edge(&confirmed).unwrap();
    // B also superseded n1.
    b.apply_supersession(&supersedes(&n2, &n1, 9)).unwrap();

    let out = merge_bundle(&a, &export_tenant(&b, "r", 1).unwrap()).unwrap();
    // n1's Superseded status rides in on the node merge (so no edge-driven
    // transition is left to count); the supersedes edge itself is new to A.
    assert_eq!(out.superseded, 0);
    assert!(out.nodes_merged >= 1, "n1 adopted B's correction");
    assert_eq!(out.edges_merged, 1, "quarantined → confirmed is a merge");
    let n1_in_a = a.get_node(&n1.compute_id()).unwrap().unwrap();
    assert_eq!(n1_in_a.status, Status::Superseded, "correction survives the merge");
    assert_eq!(n1_in_a.valid_to, Some(9));
    let edge_in_a = a
        .out_edges(&n2.compute_id())
        .unwrap()
        .into_iter()
        .find(|e| e.kind == EdgeKind::References)
        .unwrap();
    assert!(!edge_in_a.quarantined, "confirmation is one-way: confirmed anywhere = everywhere");

    // The reverse merge must not resurrect the quarantine or the Active status.
    let out = merge_bundle(&b, &export_tenant(&a, "r", 2).unwrap()).unwrap();
    assert_eq!(b.get_node(&n1.compute_id()).unwrap().unwrap().status, Status::Superseded);
    assert_eq!(out.superseded, 0);
    assert_eq!(fingerprint(&a, "r"), fingerprint(&b, "r"));
}

#[test]
fn concurrent_contradictory_supersessions_converge_in_status() {
    // A asserts x⊃y while B asserts y⊃x — the documented hard case.
    let x = node("r", "view x", vec![]);
    let y = node("r", "view y", vec![]);
    let a = store();
    let b = store();
    for s in [&a, &b] {
        s.put_node(&x).unwrap();
        s.put_node(&y).unwrap();
    }
    a.apply_supersession(&supersedes(&x, &y, 1)).unwrap();
    b.apply_supersession(&supersedes(&y, &x, 2)).unwrap();

    let out_a = merge_bundle(&a, &export_tenant(&b, "r", 1).unwrap()).unwrap();
    let out_b = merge_bundle(&b, &export_tenant(&a, "r", 1).unwrap()).unwrap();

    // Each replica rejects the other's cycle-closing edge…
    assert_eq!(out_a.rejected_supersessions, 1);
    assert_eq!(out_b.rejected_supersessions, 1);
    // …but BOTH nodes end Superseded on BOTH replicas (both views corrected),
    // and each replica's supersession DAG stays acyclic.
    for s in [&a, &b] {
        assert_eq!(s.get_node(&x.compute_id()).unwrap().unwrap().status, Status::Superseded);
        assert_eq!(s.get_node(&y.compute_id()).unwrap().unwrap().status, Status::Superseded);
    }
}

#[test]
fn forged_asserted_content_is_rejected_per_item() {
    let signer = Asserter::new(SigningKey::from_bytes(&[5u8; 32]));
    let good = signer.assert_node("r", NodeKind::Rationale, "honest insight", 1);
    let mut forged = signer.assert_node("r", NodeKind::Rationale, "original", 1);
    forged.content = b"tampered after signing".to_vec();

    let bundle = SyncBundle { repo: "r".into(), exported_at_ms: 1, nodes: vec![good.clone(), forged], edges: vec![] };
    let dst = store();
    let out = merge_bundle(&dst, &bundle).unwrap();
    assert_eq!(out.nodes_added, 1, "honest node lands");
    assert_eq!(out.rejected_signatures, 1, "forged node rejected individually");
    assert!(dst.get_node(&good.compute_id()).unwrap().is_some());
}

#[test]
fn anchor_detects_advisory_state_tampering() {
    let s = store();
    let n1 = node("r", "anchored claim", vec![]);
    let n2 = node("r", "its corrector", vec![]);
    s.put_node(&n1).unwrap();
    s.put_node(&n2).unwrap();

    let first = anchor_tenant(&s, "r", 100).unwrap();
    assert_eq!(first.node_count, 2);
    assert!(first.prev.is_none());
    assert_eq!(verify_anchor(&s, "r").unwrap(), Some(true));

    // A status transition (advisory field — OUTSIDE the content id) must
    // change the root: "what is current" is what the anchor protects.
    s.apply_supersession(&supersedes(&n2, &n1, 5)).unwrap();
    assert_eq!(verify_anchor(&s, "r").unwrap(), Some(false), "state drifted from anchor");

    // Re-anchoring chains onto the previous record.
    let second = anchor_tenant(&s, "r", 200).unwrap();
    assert!(second.prev.is_some(), "anchors are hash-chained");
    assert_ne!(second.root, first.root);
    assert_eq!(verify_anchor(&s, "r").unwrap(), Some(true));

    // Identical replica state ⇒ identical root (replica-independent).
    let twin = store();
    merge_bundle(&twin, &export_tenant(&s, "r", 1).unwrap()).unwrap();
    assert_eq!(tenant_root(&twin, "r").unwrap().to_hex(), second.root);

    // Never-anchored tenant reads None.
    assert_eq!(verify_anchor(&s, "never").unwrap(), None);
}

// ===========================================================================
// FWA-C10 remediation — federation merge authorization (red→green).
//
// These tests call the REAL authorized `merge_bundle` (4-arg) via
// `crate::merge_bundle`, NOT the trusted-local test shim above, so they exercise
// the actual capability gate a federation peer must pass.
// ===========================================================================

/// FWA-C10-01 (was a passing PoC proving the hole; INVERTED to prove the fix).
/// A peer whose grant does NOT authorize the victim repo cannot land a forged
/// Derived-plane node into it — the whole bundle is refused, fail-closed.
#[test]
fn fwa_c10_01_forged_derived_node_rejected_without_grant() {
    let victim = store();
    victim
        .commit(&[node("victim-tenant", "legit decision", vec![BelnapValue::True])], &[])
        .unwrap();
    let before = victim.node_count().unwrap();

    let mut forged = node(
        "victim-tenant",
        "FORGED: the audit concluded the bridge is safe to ship",
        vec![BelnapValue::True],
    );
    forged.plane = Plane::Derived;
    forged.trust_tier = TrustTier::DerivedDeterministic; // top tier
    forged.author = "attacker-with-no-grant".into();

    let bundle = SyncBundle {
        repo: "attacker-tenant".into(),
        exported_at_ms: 1,
        nodes: vec![forged.clone()],
        edges: vec![],
    };

    // Attacker holds a grant for its OWN repo only — not the victim's.
    let attacker_grant = grant_for(&["attacker-tenant"], true);
    let res = crate::merge_bundle(&victim, &bundle, &attacker_grant, &trust_root(), 100);

    // FIX: the merge is denied wholesale; nothing landed.
    assert!(
        matches!(res, Err(SyncError::Denied(_))),
        "forged Derived node into an unauthorized repo must be denied, got {res:?}"
    );
    assert_eq!(victim.node_count().unwrap(), before, "no node was written");
    assert!(
        victim.get_node(&forged.compute_id()).unwrap().is_none(),
        "the forged node is absent from the victim store"
    );
}

/// FWA-C10-02 (INVERTED): an unsigned Derived `Supersedes` edge from a peer that
/// is not authorized for the victim repo cannot flip a victim's Active decision.
#[test]
fn fwa_c10_02_unsigned_derived_supersedes_rejected_without_grant() {
    let victim = store();
    let target = node("victim-tenant", "ship v2 on schedule", vec![BelnapValue::True]);
    let target_id = target.compute_id();
    let usurper = node("victim-tenant", "do NOT ship; halt the release", vec![BelnapValue::True]);
    let usurper_id = usurper.compute_id();
    victim.commit(std::slice::from_ref(&target), &[]).unwrap();

    let edge = Edge {
        from: usurper_id,
        to: target_id,
        kind: EdgeKind::Supersedes,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance { method: EdgeMethod::Manual, asserter: "attacker".into(), at: 9, evidence: None },
        confidence: vec![BelnapValue::True],
        quarantined: false,
        signature: None,
    };
    let bundle = SyncBundle {
        repo: "anything".into(),
        exported_at_ms: 1,
        nodes: vec![usurper.clone()],
        edges: vec![edge],
    };

    let attacker_grant = grant_for(&["attacker-tenant"], true);
    let res = crate::merge_bundle(&victim, &bundle, &attacker_grant, &trust_root(), 100);

    assert!(
        matches!(res, Err(SyncError::Denied(_))),
        "unsigned Derived supersession into an unauthorized repo must be denied, got {res:?}"
    );
    let after = victim.get_node(&target_id).unwrap().unwrap();
    assert_eq!(
        after.status,
        Status::Active,
        "the victim's load-bearing decision is untouched"
    );
}

/// Positive control: a peer WITH a Write grant for the repo merges Derived
/// content normally (the gate authorizes, it does not block legitimate sync).
#[test]
fn fwa_c10_authorized_peer_merges_normally() {
    let dst = store();
    let n = node("citrate-chain", "ghostdag tip-selection", vec![BelnapValue::True]);
    let bundle = SyncBundle {
        repo: "citrate-chain".into(),
        exported_at_ms: 1,
        nodes: vec![n.clone()],
        edges: vec![],
    };
    let grant = grant_for(&["citrate-chain"], true);
    let out = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), 100).expect("authorized merge ok");
    assert_eq!(out.nodes_added, 1);
    assert!(dst.get_node(&n.compute_id()).unwrap().is_some());
}

/// VARIANT: a Read-only grant (can_read but not can_write) must NOT permit a
/// merge — merge is a write. Catches a grant whose scope exists but lacks Write.
#[test]
fn fwa_c10_read_only_grant_cannot_merge() {
    let dst = store();
    let bundle = SyncBundle {
        repo: "citrate-chain".into(),
        exported_at_ms: 1,
        nodes: vec![node("citrate-chain", "x", vec![BelnapValue::True])],
        edges: vec![],
    };
    let read_only = grant_for(&["citrate-chain"], false);
    let res = crate::merge_bundle(&dst, &bundle, &read_only, &trust_root(), 100);
    assert!(
        matches!(res, Err(SyncError::Denied(AuthzError::OperationDenied(Op::Write)))),
        "read-only grant must be denied Write on merge, got {res:?}"
    );
    assert_eq!(dst.node_count().unwrap(), 0);
}

/// VARIANT: an expired/revoked grant fails closed even for its own repo.
#[test]
fn fwa_c10_expired_grant_fails_closed() {
    let dst = store();
    let bundle = SyncBundle {
        repo: "citrate-chain".into(),
        exported_at_ms: 1,
        nodes: vec![node("citrate-chain", "x", vec![BelnapValue::True])],
        edges: vec![],
    };
    // grant_for sets expires_at_ms = u64::MAX, so force expiry by querying at MAX.
    let grant = grant_for(&["citrate-chain"], true);
    let res = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), u64::MAX);
    assert!(matches!(res, Err(SyncError::Denied(AuthzError::Expired))), "got {res:?}");
    assert_eq!(dst.node_count().unwrap(), 0);
}

/// VARIANT (mixed-repo bundle): a bundle that touches a repo the grant covers
/// AND one it does not is refused WHOLESALE — no partial, no-poison. Even the
/// authorized node must not land.
#[test]
fn fwa_c10_mixed_repo_bundle_refused_wholesale() {
    let dst = store();
    let ok = node("citrate-chain", "authorized", vec![BelnapValue::True]);
    let sneaky = node("victim-tenant", "unauthorized rider", vec![BelnapValue::True]);
    let bundle = SyncBundle {
        repo: "citrate-chain".into(),
        exported_at_ms: 1,
        nodes: vec![ok.clone(), sneaky.clone()],
        edges: vec![],
    };
    let grant = grant_for(&["citrate-chain"], true); // NOT victim-tenant
    let res = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), 100);
    assert!(matches!(res, Err(SyncError::Denied(_))), "got {res:?}");
    assert_eq!(dst.node_count().unwrap(), 0, "no partial application — even the authorized node is held back");
}

/// VARIANT (edge-repo authz, mirrors the FUA-MEMORIES-03 fix on merge_diff): an
/// edge whose far endpoint lives in a repo the grant does NOT cover is refused,
/// even when the near endpoint's repo IS covered. Edge-endpoint repos are
/// authorized, not just node repos.
#[test]
fn fwa_c10_edge_endpoint_repo_is_authorized() {
    let dst = store();
    // Far endpoint already in the store, owned by an unauthorized repo.
    let far = node("victim-tenant", "victim node", vec![BelnapValue::True]);
    dst.commit(std::slice::from_ref(&far), &[]).unwrap();

    let near = node("citrate-chain", "attacker node", vec![BelnapValue::True]);
    let edge = Edge {
        from: near.compute_id(),
        to: far.compute_id(), // crosses into victim-tenant
        kind: EdgeKind::References,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "x".into(), at: 1, evidence: None },
        confidence: vec![],
        quarantined: false,
        signature: None,
    };
    let bundle = SyncBundle {
        repo: "citrate-chain".into(),
        exported_at_ms: 1,
        nodes: vec![near],
        edges: vec![edge],
    };
    let grant = grant_for(&["citrate-chain"], true); // covers the node repo, not victim-tenant
    let res = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), 100);
    assert!(
        matches!(res, Err(SyncError::Denied(_))),
        "an edge crossing into an unauthorized repo must be denied, got {res:?}"
    );
}

/// VARIANT (unresolvable endpoint): an edge referencing an endpoint unknown to
/// both the bundle and the store cannot have its repo resolved → fail closed.
#[test]
fn fwa_c10_unresolvable_edge_endpoint_fails_closed() {
    let dst = store();
    let known = node("citrate-chain", "known", vec![BelnapValue::True]);
    let ghost = node("citrate-chain", "ghost-not-in-bundle-or-store", vec![BelnapValue::True]);
    let edge = Edge {
        from: known.compute_id(),
        to: ghost.compute_id(), // never added anywhere
        kind: EdgeKind::References,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "x".into(), at: 1, evidence: None },
        confidence: vec![],
        quarantined: false,
        signature: None,
    };
    let bundle = SyncBundle {
        repo: "citrate-chain".into(),
        exported_at_ms: 1,
        nodes: vec![known],
        edges: vec![edge],
    };
    let grant = grant_for(&["citrate-chain"], true);
    let res = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), 100);
    assert!(
        matches!(res, Err(SyncError::UnresolvableEndpoint(_))),
        "an edge with an unresolvable endpoint must fail closed, got {res:?}"
    );
    assert_eq!(dst.node_count().unwrap(), 0, "fail-closed before any write — nothing landed");
}

/// VARIANT: the still-correct per-item Asserted signature check survives the new
/// gate — an authorized peer pushing a tampered Asserted node has THAT node
/// rejected individually (the honest one lands), exactly as before.
#[test]
fn fwa_c10_asserted_signature_check_still_applies_under_authz() {
    let signer = Asserter::new(SigningKey::from_bytes(&[5u8; 32]));
    let good = signer.assert_node("r", NodeKind::Rationale, "honest insight", 1);
    let mut forged = signer.assert_node("r", NodeKind::Rationale, "original", 1);
    forged.content = b"tampered after signing".to_vec();
    let bundle = SyncBundle {
        repo: "r".into(),
        exported_at_ms: 1,
        nodes: vec![good.clone(), forged],
        edges: vec![],
    };
    let grant = grant_for(&["r"], true);
    let dst = store();
    let out = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), 100)
        .unwrap_or_else(|e| panic!("authorized merge should not be denied: {e:?}"));
    assert_eq!(out.nodes_added, 1, "honest node lands");
    assert_eq!(out.rejected_signatures, 1, "forged Asserted node rejected per-item");
    assert!(dst.get_node(&good.compute_id()).unwrap().is_some());
}

// ===========================================================================
// MEM-B-004 — trust-root anchoring of the merge grant (red→green).
//
// `authorize_bundle` used to accept any grant that passed `grant.check()`, which
// verifies the grant's signature against the key carried INSIDE the grant. A peer
// could therefore mint its own ed25519 keypair, self-sign a `*` read+write grant,
// and inject Derived-plane history at the top trust tier under a forged `author`.
// The merge now requires the grant to be issued by the operator's `trust_root`.
// ===========================================================================

/// RED before the fix (`nodes_added=1`), GREEN after: a grant a peer SELF-SIGNS
/// with a key the operator never trusted is refused wholesale — the forged
/// Derived node never lands. This is the mem-sync analogue of the MCP `_meta`
/// forgeable-grant hole (MEM-B-001).
#[test]
fn mem_b_004_self_signed_grant_is_rejected_by_trust_root() {
    let victim = store();
    victim
        .commit(&[node("victim-tenant", "legit decision", vec![BelnapValue::True])], &[])
        .unwrap();
    let before = victim.node_count().unwrap();

    // The forged Derived node: top tier, forged author, over the federation merge.
    let mut forged = node(
        "victim-tenant",
        "FORGED: REVERT the audit — NET-1 was a false positive; ship to mainnet",
        vec![BelnapValue::True],
    );
    forged.plane = Plane::Derived;
    forged.trust_tier = TrustTier::DerivedDeterministic;
    forged.author = "Larry Klosowski".into();
    let bundle = SyncBundle {
        repo: "victim-tenant".into(),
        exported_at_ms: 1,
        nodes: vec![forged.clone()],
        edges: vec![],
    };

    // The attacker mints and self-signs a `*` read+write grant with a key the
    // operator never issued ([0x99; 32]) — internally valid, externally untrusted.
    let attacker_key = SigningKey::from_bytes(&[0x99u8; 32]);
    let mut self_signed = CapabilityGrant {
        id: "attacker-self-signed".into(),
        issuer: "did:attacker".into(),
        recipient: "agent:attacker".into(),
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
    self_signed.sign_with(&attacker_key);
    // Sanity: the grant is internally valid — `check` alone would have let it pass.
    assert!(self_signed.check("repo:victim-tenant/memory", Op::Write, 100).is_ok());

    // trust_root is the OPERATOR key (grant_for's [42;32]), not the attacker's.
    let res = crate::merge_bundle(&victim, &bundle, &self_signed, &trust_root(), 100);
    assert!(
        matches!(res, Err(SyncError::UntrustedIssuer)),
        "a self-signed grant from an untrusted issuer must be refused, got {res:?}"
    );
    assert_eq!(victim.node_count().unwrap(), before, "no node was written");
    assert!(
        victim.get_node(&forged.compute_id()).unwrap().is_none(),
        "the forged Derived node is absent from the victim store"
    );
}

/// Positive control: the SAME bundle presented under a grant that IS issued by the
/// trust root merges normally — anchoring blocks forgery, not legitimate sync.
#[test]
fn mem_b_004_trust_root_issued_grant_merges() {
    let dst = store();
    let n = node("victim-tenant", "a legitimately synced derived node", vec![BelnapValue::True]);
    let bundle = SyncBundle {
        repo: "victim-tenant".into(),
        exported_at_ms: 1,
        nodes: vec![n.clone()],
        edges: vec![],
    };
    // grant_for signs with the operator key [42;32]; trust_root() is its pubkey.
    let grant = grant_for(&["victim-tenant"], true);
    let out = crate::merge_bundle(&dst, &bundle, &grant, &trust_root(), 100)
        .expect("trust-root-issued grant must merge");
    assert_eq!(out.nodes_added, 1);
    assert!(dst.get_node(&n.compute_id()).unwrap().is_some());
}

// ===========================================================================
// MEM-B-002 — the Belnap-CRDT merge is a join-semilattice (red→green).
//
// Before the fix, `merge_node` kept the LOCAL `embedding`/`trust_tier`/`anchors`
// and the edge merge kept the LOCAL `trust_tier`/`plane`/`provenance`/`signature`
// (first-writer-wins on the receiving replica), so two replicas that exchanged
// bundles in different orders never converged. This deals the SAME updates to two
// replicas in OPPOSITE orders, gossips to a fixpoint, and asserts byte-identical
// state. It FAILS on the pre-fix code (divergent embedding + edge advisory group)
// and PASSES once every non-identity field is folded order-independently.
// ===========================================================================

/// Two nodes that share a content id but differ on every advisory field, and two
/// edges that share `(from,to,kind)` but differ on tier/plane/provenance —
/// dealt A-then-B on one replica and B-then-A on the other.
#[test]
fn mem_b_002_crdt_converges_regardless_of_merge_order() {
    // Shared node id (plane+kind+repo+author+source+content are identical); the
    // two variants disagree on embedding, trust_tier, confidence, anchors.
    let base = node("r", "a shared claim", vec![BelnapValue::True]);
    let mut a_node = base.clone();
    a_node.embedding = Some(VersionedVector { model: "bge@q8".into(), data: vec![9.0, 9.0, 9.0] });
    a_node.trust_tier = TrustTier::DerivedDeterministic;
    a_node.anchors = vec![CodeAnchor {
        repo: "r".into(),
        path: "src/a.rs".into(),
        symbol: Some("f".into()),
        line_start: 1,
        line_end: 2,
    }];
    let mut b_node = base.clone();
    b_node.embedding = Some(VersionedVector { model: "bge@q8".into(), data: vec![1.0, 1.0, 1.0] });
    b_node.trust_tier = TrustTier::InferredAdvisory;
    b_node.confidence = vec![BelnapValue::False];
    b_node.anchors = vec![CodeAnchor {
        repo: "r".into(),
        path: "src/b.rs".into(),
        symbol: None,
        line_start: 3,
        line_end: 4,
    }];
    assert_eq!(a_node.compute_id(), b_node.compute_id(), "same content id, differing advisory state");

    // Two endpoints for a shared-key edge.
    let x = node("r", "endpoint x", vec![]);
    let y = node("r", "endpoint y", vec![]);

    // Same (from,to,kind) edge, differing advisory group.
    let edge_a = Edge {
        from: x.compute_id(),
        to: y.compute_id(),
        kind: EdgeKind::References,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "alice".into(), at: 1, evidence: None },
        confidence: vec![BelnapValue::True],
        quarantined: false,
        signature: None,
    };
    // Both edges are Derived (an unsigned Asserted edge would be rejected by the
    // per-item signature check); they disagree on the advisory group the security
    // edge of MEM-B-002/003 is about — trust_tier and provenance.
    let mut edge_b = edge_a.clone();
    edge_b.trust_tier = TrustTier::InferredAdvisory;
    edge_b.provenance = EdgeProvenance { method: EdgeMethod::Nlp, asserter: "mallory".into(), at: 2, evidence: Some("guess".into()) };
    edge_b.confidence = vec![BelnapValue::False];

    // Seed each endpoint node on both replicas (edges need their endpoints).
    let ra = store();
    let rb = store();
    for s in [&ra, &rb] {
        s.put_node(&x).unwrap();
        s.put_node(&y).unwrap();
    }

    // Replica A applies A-variants first, then B's bundle; replica B the reverse.
    ra.put_node(&a_node).unwrap();
    ra.add_edge(&edge_a).unwrap();
    rb.put_node(&b_node).unwrap();
    rb.add_edge(&edge_b).unwrap();

    let bundle_from_a = export_tenant(&ra, "r", 1).unwrap();
    let bundle_from_b = export_tenant(&rb, "r", 1).unwrap();

    // Gossip to a fixpoint in opposite orders.
    for _ in 0..3 {
        merge_bundle(&ra, &bundle_from_b).unwrap();
        merge_bundle(&rb, &bundle_from_a).unwrap();
    }

    // Byte-identical convergence over the FULL records (not just the anchor root).
    let mut a_nodes = ra.all_nodes().unwrap();
    let mut b_nodes = rb.all_nodes().unwrap();
    a_nodes.sort_by_key(|n| n.compute_id());
    b_nodes.sort_by_key(|n| n.compute_id());
    assert_eq!(a_nodes, b_nodes, "replicas did not converge on node state (MEM-B-002)");

    let mut a_edges = ra.all_edges().unwrap();
    let mut b_edges = rb.all_edges().unwrap();
    a_edges.sort_by_key(|e| e.key());
    b_edges.sort_by_key(|e| e.key());
    assert_eq!(a_edges, b_edges, "replicas did not converge on edge state (MEM-B-002)");

    // And the merged advisory values are the conservative (least-trusted) ones.
    let merged = a_nodes.iter().find(|n| n.compute_id() == a_node.compute_id()).unwrap();
    assert_eq!(merged.trust_tier, TrustTier::InferredAdvisory, "least-trusted wins");
    let merged_edge = a_edges.iter().find(|e| e.kind == EdgeKind::References).unwrap();
    assert_eq!(merged_edge.trust_tier, TrustTier::InferredAdvisory, "least-trusted edge group wins");
}

// ---------------------------------------------------------------------------
// TRIPWIRE (Class-A "trust-what-is-signed"): a permanent source-level assertion
// that the only entry into the federation node/edge store is the authorized
// `merge_bundle`. If a future edit removes the `authorize_bundle(...)` call from
// `merge_bundle`, OR adds a store write ANYWHERE else in the crate's production
// source, this fails.
//
// FWA-BV-MEM-02 (blind 2nd-model follow-up): the ORIGINAL backstop was
// NAME-scoped — it only inspected functions named `pub fn merge*`. A
// differently-named sibling write helper (e.g. `pub fn ingest_bundle_unguarded`
// that calls `store.put_node` directly) bypassed authz entirely AND the backstop
// still PASSED. This version is LOCATION-scoped: it scans EVERY production `.rs`
// file on the sync write path (lib.rs, transport.rs, chain.rs — test modules
// stripped) and FAILS if any node/edge write primitive
// (`put_node`/`add_edge`/`apply_supersession`/`commit`) appears OUTSIDE the body
// of the single authorized write entry, `merge_bundle`. Name no longer matters;
// only location does.
//
// A semgrep rule (`.agentile/tripwires/mem-sync-merge-authz.yml`,
// `no-unguarded-store-write-helper-in-sync`, severity ERROR) enforces the same
// invariant in CI; this in-tree test is the always-on backstop so the invariant
// cannot silently regress even without semgrep wired.
// ---------------------------------------------------------------------------

/// The node/edge write primitives on the store API. Any of these reaching the
/// store outside `merge_bundle`'s authorized body is an unguarded write path.
/// (`put_meta` is intentionally excluded — it writes anchor metadata, not
/// graph nodes/edges, and `anchor_tenant` legitimately uses it.)
const STORE_WRITE_PRIMITIVES: [&str; 4] =
    [".put_node(", ".add_edge(", ".apply_supersession(", ".commit("];

/// Strip `#[cfg(test)]`-gated modules from a source string so the location scan
/// only sees PRODUCTION code. Tests (here and in transport.rs) legitimately call
/// `store.commit(...)` / `peer.put_node(...)` to build fixtures; those are not
/// federation write paths. We remove from each `#[cfg(test)]` marker to the end
/// of its following balanced `mod { ... }` block (or to EOF if it is the trailing
/// `mod tests;` / inline module, which is the common shape).
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(pos) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        // Find the first `{` after the cfg marker (the test module's body open).
        match after.find('{') {
            None => {
                // `#[cfg(test)] mod tests;` (declaration, no inline body) — nothing
                // more to strip in THIS file; drop to EOF and stop.
                rest = "";
                break;
            }
            Some(brace_rel) => {
                // Walk braces to the matching close, then continue after it.
                let body = &after[brace_rel..];
                let mut depth = 0usize;
                let mut end = None;
                for (i, c) in body.char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(i + 1);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                match end {
                    Some(e) => rest = &body[e..],
                    None => {
                        rest = "";
                        break;
                    }
                }
            }
        }
    }
    out.push_str(rest);
    out
}

/// Return the byte span `[start, end)` of `merge_bundle`'s body (the authorized
/// write region) within `src`, by brace-matching from the function's opening `{`.
fn merge_bundle_body_span(src: &str) -> (usize, usize) {
    let sig = src.find("pub fn merge_bundle(").expect("merge_bundle exists");
    let open_rel = src[sig..].find('{').expect("merge_bundle has a body");
    let open = sig + open_rel;
    let bytes = &src[open..];
    let mut depth = 0usize;
    for (i, c) in bytes.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (open, open + i + 1);
                }
            }
            _ => {}
        }
    }
    panic!("merge_bundle body did not close — unbalanced braces in lib.rs");
}

#[test]
fn tripwire_merge_bundle_is_authorized_and_is_the_sole_write_entry() {
    let lib = include_str!("lib.rs");

    // 1) `merge_bundle` must call the authz gate before writing.
    let (mb_start, mb_end) = merge_bundle_body_span(lib);
    let mb = &lib[mb_start..mb_end];
    assert!(
        mb.contains("authorize_bundle("),
        "merge_bundle must call authorize_bundle() before any store write (FWA-C10-01/02)"
    );
    assert!(
        lib.contains("grant: &CapabilityGrant"),
        "merge_bundle must take a CapabilityGrant — no unauthenticated merge path"
    );

    // 2) authorize_bundle must check Op::Write per repo.
    let ab_start = lib.find("fn authorize_bundle(").expect("authorize_bundle exists");
    let ab = &lib[ab_start..];
    assert!(
        ab.contains("grant.check(") && ab.contains("Op::Write"),
        "authorize_bundle must grant.check(.., Op::Write, ..) every touched repo"
    );

    // 3) LOCATION-scoped sole-write-entry check (FWA-BV-MEM-02). Across EVERY
    //    production source file on the sync write path, every store WRITE
    //    primitive must live inside merge_bundle's authorized body — regardless of
    //    the enclosing function's NAME. This is what catches a differently-named
    //    sibling helper (e.g. `pub fn ingest_bundle_unguarded`) that the original
    //    `pub fn merge*` name-scan walked straight past.
    //
    //    Each (file, source) pair: lib.rs holds the only sanctioned writes (inside
    //    merge_bundle); transport.rs / chain.rs must hold NONE in production code.
    let sources: [(&str, &str); 3] = [
        ("lib.rs", lib),
        ("transport.rs", include_str!("transport.rs")),
        ("chain.rs", include_str!("chain.rs")),
    ];

    for (file, raw) in sources {
        let prod = strip_test_modules(raw);
        // The authorized window only exists in lib.rs (where merge_bundle lives).
        let authorized: Option<(usize, usize)> = if file == "lib.rs" {
            Some(merge_bundle_body_span(&prod))
        } else {
            None
        };

        for prim in STORE_WRITE_PRIMITIVES {
            let mut from = 0usize;
            while let Some(rel) = prod[from..].find(prim) {
                let at = from + rel;
                let inside_authorized = authorized
                    .map(|(s, e)| at >= s && at < e)
                    .unwrap_or(false);
                assert!(
                    inside_authorized,
                    "FWA-BV-MEM-02: unguarded store write `{prim}` found in {file} at byte \
                     offset {at}, OUTSIDE the authorized merge_bundle body. Every \
                     node/edge write must route through merge_bundle (post \
                     authorize_bundle). A sibling write helper bypasses federation \
                     authz — route it through merge_bundle / merge_bundle_trusted."
                );
                from = at + prim.len();
            }
        }
    }

    // 4) The sanctioned trusted wrapper must funnel into merge_bundle, never the
    //    store directly (it is the one delegated path, still authz-gated).
    let mbt_start = lib.find("pub fn merge_bundle_trusted(").expect("exists");
    let mbt = &lib[mbt_start..mbt_start + 400.min(lib.len() - mbt_start)];
    assert!(
        mbt.contains("merge_bundle(store"),
        "merge_bundle_trusted must delegate to the authorized merge_bundle"
    );
}

#[test]
fn tripwire_trusted_local_grant_key_is_not_hardcoded() {
    // MEM-B-018: the trusted-ingest signing key must be seeded from the OS CSPRNG,
    // not a published constant like `[0xA1u8; 32]`. A hardcoded key is a known,
    // publishable signing key sitting beside the one no-authz merge entry point.
    let lib = include_str!("lib.rs");
    let start = lib.find("pub fn trusted_local_grant(").expect("trusted_local_grant exists");
    let open = start + lib[start..].find('{').expect("body");
    let body = &lib[open..];
    let mut depth = 0usize;
    let mut end = body.len();
    for (i, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &body[..end];
    assert!(
        body.contains("getrandom::getrandom("),
        "trusted_local_grant must seed its key from the OS CSPRNG (MEM-B-018)"
    );
    assert!(
        !body.contains("from_bytes(&[0x") && !body.contains("from_bytes(&[0xA1"),
        "trusted_local_grant must NOT sign with a hardcoded constant key (MEM-B-018)"
    );
}

/// PBA-L6b-019 tripwire (remediation plan): the peer-to-peer HTTP transport stays
/// OFF in every shipped binary until mTLS / peer identity lands. Fails if any
/// workspace crate enables mem-sync's `http` feature (the only ones allowed are
/// mem-sync itself, whose manifest declares it).
#[test]
fn tripwire_no_binary_enables_the_sync_http_feature() {
    let crates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut checked = 0;
    for entry in std::fs::read_dir(&crates_dir).expect("crates dir") {
        let dir = entry.expect("entry").path();
        let manifest = dir.join("Cargo.toml");
        if dir.file_name().and_then(|n| n.to_str()) == Some("mem-sync") || !manifest.exists() {
            continue;
        }
        let toml = std::fs::read_to_string(&manifest).expect("read manifest");
        for line in toml.lines().filter(|l| l.contains("mem-sync")) {
            assert!(
                !line.contains("\"http\""),
                "PBA-L6b-019: {} enables mem-sync's `http` transport: {line}",
                manifest.display()
            );
        }
        // `mem-sync/http` feature forwarding, e.g. `foo = ["mem-sync/http"]`.
        assert!(!toml.contains("mem-sync/http"), "PBA-L6b-019: {} forwards mem-sync/http", manifest.display());
        checked += 1;
    }
    assert!(checked >= 9, "scanned {checked} crate manifests");
}
