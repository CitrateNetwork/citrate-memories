use super::*;

use ed25519_dalek::SigningKey;
use mem_assert::Asserter;
use mem_core::{
    BelnapValue, EdgeMethod, EdgeProvenance, NodeKind, SourceRef, TrustTier, SCHEMA_VERSION,
};
use mem_store::kv::InMemoryKv;

fn store() -> MemoryDagStore<MemoryNode> {
    MemoryDagStore::new(Box::new(InMemoryKv::new()))
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
