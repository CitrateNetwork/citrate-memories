//! Edges between memory nodes. See `PLANSET/02_ARCHITECTURE.md §3.2`.

use serde::{Deserialize, Serialize};

use crate::belnap::BelnapValue;
use crate::identity::ContentHash;
use crate::node::{Plane, TrustTier, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Chronological spine (the selected-parent analogue): `from` follows `to`.
    TemporalNext,
    /// Additional DAG parent (multi-parent merge).
    MergeParent,
    /// `from` supersedes `to` (append-only correction; must stay acyclic).
    Supersedes,
    /// Cross-repo dependency (from manifest `[[drift]]`).
    DependsOn,
    CausedBy,
    Motivates,
    /// Sprint step -> commit/PR that implements it.
    Implements,
    /// ADR -> sprint/decision it governs.
    Decides,
    Refutes,
    Contradicts,
    /// Lineage (LoRA-provenance style).
    DerivedFrom,
    /// Cross-DAG latent edge (authz-gated at traversal time).
    AnalogousTo,
    References,
    /// Node -> code span.
    AnchoredTo,
    /// Chain-state (E-4): checkpoint (block) -> event observed in it. Ingest-
    /// only, like `TemporalNext` — never proposable over MCP.
    Emits,
    /// Chain-state (E-4): event -> the contract node it involved. Ingest-only.
    Touches,
    /// In-flight branch layer (ADR-09): a `Branch` node -> a commit unique to that
    /// branch (not yet reachable from the default tip). Ingest-only, Derived; never
    /// proposable over MCP.
    BranchContains,
}

impl EdgeKind {
    /// Stable byte tag used in the edge storage key.
    pub fn tag(&self) -> u8 {
        match self {
            EdgeKind::TemporalNext => 1,
            EdgeKind::MergeParent => 2,
            EdgeKind::Supersedes => 3,
            EdgeKind::DependsOn => 4,
            EdgeKind::CausedBy => 5,
            EdgeKind::Motivates => 6,
            EdgeKind::Implements => 7,
            EdgeKind::Decides => 8,
            EdgeKind::Refutes => 9,
            EdgeKind::Contradicts => 10,
            EdgeKind::DerivedFrom => 11,
            EdgeKind::AnalogousTo => 12,
            EdgeKind::References => 13,
            EdgeKind::AnchoredTo => 14,
            EdgeKind::Emits => 15,
            EdgeKind::Touches => 16,
            EdgeKind::BranchContains => 17,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeMethod {
    /// Derived deterministically from a git trailer or fenced `agentile` block.
    Trailer,
    /// Derived from ingestion structure (e.g. parent commit).
    Ingest,
    /// Proposed by an LLM (advisory; starts quarantined).
    Nlp,
    /// Proposed by the analogy engine (advisory; starts quarantined).
    Analogy,
    /// Asserted manually by a human/agent.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeProvenance {
    pub method: EdgeMethod,
    pub asserter: String,
    pub at: Timestamp,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: ContentHash,
    pub to: ContentHash,
    pub kind: EdgeKind,
    pub plane: Plane,
    pub trust_tier: TrustTier,
    pub provenance: EdgeProvenance,
    pub confidence: Vec<BelnapValue>,
    /// Inferred edges start quarantined and are never load-bearing until a
    /// `confirm` promotes them (anti-poisoning, R1).
    pub quarantined: bool,
    pub signature: Option<Vec<u8>>,
}

impl Edge {
    /// Storage key: `from ‖ to ‖ kind`. An edge is identified by its endpoints
    /// and kind; re-asserting the same edge is idempotent (set semantics),
    /// which is what the grow-only CRDT merge needs.
    pub fn key(&self) -> Vec<u8> {
        let mut k = Vec::with_capacity(65);
        k.extend_from_slice(self.from.as_bytes());
        k.extend_from_slice(self.to.as_bytes());
        k.push(self.kind.tag());
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdBuilder;

    fn h(s: &str) -> ContentHash {
        IdBuilder::new("test").field_str(1, s).finish()
    }

    fn edge(from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge {
            from: h(from),
            to: h(to),
            kind,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance {
                method: EdgeMethod::Trailer,
                asserter: "ingest".into(),
                at: 0,
                evidence: None,
            },
            confidence: vec![],
            quarantined: false,
            signature: None,
        }
    }

    #[test]
    fn key_depends_on_endpoints_and_kind() {
        let e1 = edge("a", "b", EdgeKind::Implements);
        let e2 = edge("a", "b", EdgeKind::Decides);
        let e3 = edge("b", "a", EdgeKind::Implements);
        assert_ne!(e1.key(), e2.key(), "kind distinguishes edges");
        assert_ne!(e1.key(), e3.key(), "direction distinguishes edges");
        assert_eq!(e1.key(), edge("a", "b", EdgeKind::Implements).key(), "stable");
    }

    #[test]
    fn key_length_is_fixed() {
        assert_eq!(edge("a", "b", EdgeKind::TemporalNext).key().len(), 65);
    }

    #[test]
    fn chain_edge_tags_are_stable_and_distinct() {
        // E-4: storage-key tags are append-only — 15/16 are claimed forever.
        assert_eq!(EdgeKind::Emits.tag(), 15);
        assert_eq!(EdgeKind::Touches.tag(), 16);
        assert_ne!(
            edge("a", "b", EdgeKind::Emits).key(),
            edge("a", "b", EdgeKind::Touches).key()
        );
    }

    #[test]
    fn branch_contains_tag_is_stable_and_distinct() {
        // ADR-09: append-only tag 17, distinct from every prior edge kind.
        assert_eq!(EdgeKind::BranchContains.tag(), 17);
        assert_ne!(
            edge("a", "b", EdgeKind::BranchContains).key(),
            edge("a", "b", EdgeKind::Touches).key()
        );
    }
}
