//! Memory nodes and the ontology (WP-1.1, started in WP-0.3).
//!
//! See `PLANSET/02_ARCHITECTURE.md §3` for the canonical schema.

use serde::{Deserialize, Serialize};

use crate::belnap::BelnapValue;
use crate::identity::{ContentHash, IdBuilder};

pub const SCHEMA_VERSION: u16 = 1;
const ID_DOMAIN: &str = "memory-node:v1";

pub type Timestamp = u64; // epoch milliseconds

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plane {
    Derived,
    Asserted,
}

impl Plane {
    fn tag(&self) -> &'static str {
        match self {
            Plane::Derived => "derived",
            Plane::Asserted => "asserted",
        }
    }
}

/// Trust tiers, highest to lowest. Recall filters on these; LoRA training and
/// "decisions of record" only ever consider the top two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TrustTier {
    /// Deterministically derived from canonical artifacts. Highest trust.
    DerivedDeterministic,
    /// A human explicitly confirmed it.
    HumanConfirmed,
    /// An authenticated agent asserted it.
    AgentAsserted,
    /// Inferred (LLM/analogy). Starts quarantined; never load-bearing until confirmed.
    InferredAdvisory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Active,
    Superseded,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SprintPhase {
    Kickoff,
    Daily,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NarrativeKind {
    Journal,
    Essay,
    CaseStudy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimStatus {
    Confirmed,
    Corrected,
    Refined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Commit,
    Pr,
    Sprint(SprintPhase),
    Adr,
    Audit,
    Finding,
    Handoff,
    Narrative(NarrativeKind),
    Claim(ClaimStatus),
    ManifestChange,
    PinBump,
    DriftEvent,
    Benchmark,
    AgentAction,
    Blocker,
    TechDebt,
    WorkPackage,
    Rationale,
    AnalogyHypothesis,
    /// Generic markdown document that doesn't match a more specific kind
    /// (e.g. a planset/design doc).
    Doc,
    /// Federation meta-graph: one node per tenant repo (role/tier summary),
    /// living in the reserved `federation` tenant (MEM-S4 WP-4.4).
    Tenant,
    /// Chain-state tenant (E-4, `PLANSET/08`): the network/genesis parameter
    /// summary for one chain — one node per chainId.
    ChainNetwork,
    /// Chain-state tenant: a deployed contract (or precompile/AA module) at a
    /// specific address. The address is identity-bearing, so a redeploy is a
    /// NEW node that supersedes the old one — never a mutation.
    ChainContract,
    /// Chain-state tenant: an observed on-chain event (a transaction touching
    /// a watched address). Identity = tx hash, so re-walking a range dedupes.
    ChainEvent,
    /// Chain-state tenant: a witnessed block (number + hash). A re-org mints a
    /// new checkpoint at the same height that supersedes the orphaned one.
    ChainCheckpoint,
    /// In-flight branch layer (ADR-09): one node per non-default branch of a repo,
    /// carrying the branch tip. Its `BranchContains` edges point at the commits
    /// unique to the branch (`git rev-list default..branch`), so recall can surface
    /// work-in-progress separately from canonical (default-branch) truth. Archived
    /// when the branch merges (its commits become reachable from the default tip).
    Branch,
}

impl NodeKind {
    /// Stable discriminant string fed into the content hash. Changing these
    /// strings is a schema migration.
    pub fn discriminant(&self) -> String {
        match self {
            NodeKind::Commit => "commit".into(),
            NodeKind::Pr => "pr".into(),
            NodeKind::Sprint(p) => format!("sprint:{p:?}").to_lowercase(),
            NodeKind::Adr => "adr".into(),
            NodeKind::Audit => "audit".into(),
            NodeKind::Finding => "finding".into(),
            NodeKind::Handoff => "handoff".into(),
            NodeKind::Narrative(k) => format!("narrative:{k:?}").to_lowercase(),
            NodeKind::Claim(s) => format!("claim:{s:?}").to_lowercase(),
            NodeKind::ManifestChange => "manifest_change".into(),
            NodeKind::PinBump => "pin_bump".into(),
            NodeKind::DriftEvent => "drift_event".into(),
            NodeKind::Benchmark => "benchmark".into(),
            NodeKind::AgentAction => "agent_action".into(),
            NodeKind::Blocker => "blocker".into(),
            NodeKind::TechDebt => "tech_debt".into(),
            NodeKind::WorkPackage => "work_package".into(),
            NodeKind::Rationale => "rationale".into(),
            NodeKind::AnalogyHypothesis => "analogy_hypothesis".into(),
            NodeKind::Doc => "doc".into(),
            NodeKind::Tenant => "tenant".into(),
            NodeKind::ChainNetwork => "chain_network".into(),
            NodeKind::ChainContract => "chain_contract".into(),
            NodeKind::ChainEvent => "chain_event".into(),
            NodeKind::ChainCheckpoint => "chain_checkpoint".into(),
            NodeKind::Branch => "branch".into(),
        }
    }
}

/// Where the canonical artifact lives. Rule 9 — we point, we do not copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceRef {
    /// Backed by a real artifact in a repo at a specific git revision.
    Artifact {
        repo: String,
        path: String,
        git_sha: String,
        byte_start: u64,
        byte_end: u64,
    },
    /// A git commit object itself (not a file span). The sha uniquely identifies
    /// the commit, so a commit node's identity is stable across rebuilds.
    GitCommit { repo: String, sha: String },
    /// Born in the graph (Asserted plane); lives nowhere else. `key` makes
    /// distinct dag-native records distinguishable in the identity hash.
    DagNative { key: String },
}

impl SourceRef {
    fn canonical_bytes(&self) -> Vec<u8> {
        match self {
            SourceRef::Artifact {
                repo,
                path,
                git_sha,
                byte_start,
                byte_end,
            } => format!("artifact|{repo}|{path}|{git_sha}|{byte_start}|{byte_end}").into_bytes(),
            SourceRef::GitCommit { repo, sha } => format!("git_commit|{repo}|{sha}").into_bytes(),
            SourceRef::DagNative { key } => format!("dag_native|{key}").into_bytes(),
        }
    }
}

/// Binds a node to a span of code (D3.2 code-anchored recall). Advisory
/// enrichment — NOT part of identity, so anchors can be added later without
/// re-hashing the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAnchor {
    pub repo: String,
    pub path: String,
    pub symbol: Option<String>,
    pub line_start: u32,
    pub line_end: u32,
}

/// An embedding tagged with the model that produced it. Versioned and advisory:
/// similarity queries refuse to mix model versions, and the vector is NEVER part
/// of node identity (so re-embedding is safe).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionedVector {
    pub model: String, // e.g. "bge-small-en-v1.5@q8"
    pub data: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryNode {
    pub schema_version: u16,
    pub plane: Plane,
    pub kind: NodeKind,
    pub repo: String,
    pub author: String,

    // --- identity-defining (see compute_id) ---
    pub source_ref: SourceRef,
    /// Normalised canonical content bytes (small — e.g. commit subject+trailers,
    /// or the assertion body). The full artifact stays at `source_ref`.
    pub content: Vec<u8>,

    // --- bitemporal (NOT in identity) ---
    pub valid_from: Timestamp,
    pub valid_to: Option<Timestamp>,
    pub observed_at: Timestamp,

    // --- advisory / mutable (NOT in identity) ---
    pub trust_tier: TrustTier,
    pub signature: Option<Vec<u8>>,
    pub embedding: Option<VersionedVector>,
    pub confidence: Vec<BelnapValue>,
    pub anchors: Vec<CodeAnchor>,
    pub status: Status,
}

impl MemoryNode {
    /// Compute the content-addressed id.
    ///
    /// IDENTITY SET: schema_version, plane, kind, repo, author, source_ref, content.
    /// DELIBERATELY EXCLUDED: all timestamps, trust_tier, signature, embedding,
    /// confidence, anchors, status. This is the core invariant — re-embedding,
    /// re-observing, supersession, and enrichment must never change a node's id.
    pub fn compute_id(&self) -> ContentHash {
        IdBuilder::new(ID_DOMAIN)
            .field_u16(1, self.schema_version)
            .field_str(2, self.plane.tag())
            .field_str(3, &self.kind.discriminant())
            .field_str(4, &self.repo)
            .field_str(5, &self.author)
            .field(6, &self.source_ref.canonical_bytes())
            .field(7, &self.content)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Commit,
            repo: "citrate-chain".into(),
            author: "ingest@42".into(),
            source_ref: SourceRef::Artifact {
                repo: "citrate-chain".into(),
                path: "core/learning/src/adapters.rs".into(),
                git_sha: "abc123".into(),
                byte_start: 0,
                byte_end: 100,
            },
            content: b"feat: compose_lora rank concat".to_vec(),
            valid_from: 1_000,
            valid_to: None,
            observed_at: 2_000,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: None,
            confidence: vec![],
            anchors: vec![],
            status: Status::Active,
        }
    }

    #[test]
    fn id_is_stable_across_recompute() {
        let n = sample();
        assert_eq!(n.compute_id(), n.compute_id());
    }

    #[test]
    fn embedding_does_not_affect_id() {
        let n = sample();
        let id0 = n.compute_id();
        let mut n2 = n.clone();
        n2.embedding = Some(VersionedVector {
            model: "bge@q8".into(),
            data: vec![0.1, 0.2, 0.3],
        });
        assert_eq!(id0, n2.compute_id(), "embedding must be excluded from identity");
    }

    #[test]
    fn timestamps_and_status_do_not_affect_id() {
        let n = sample();
        let id0 = n.compute_id();
        let mut n2 = n.clone();
        n2.observed_at = 999_999;
        n2.valid_to = Some(123_456);
        n2.status = Status::Superseded;
        n2.confidence = vec![BelnapValue::Both];
        n2.anchors = vec![CodeAnchor {
            repo: "x".into(),
            path: "y".into(),
            symbol: Some("f".into()),
            line_start: 1,
            line_end: 2,
        }];
        assert_eq!(
            id0,
            n2.compute_id(),
            "bitemporal/status/confidence/anchors must be excluded from identity"
        );
    }

    #[test]
    fn content_change_changes_id() {
        let n = sample();
        let id0 = n.compute_id();
        let mut n2 = n.clone();
        n2.content = b"different content".to_vec();
        assert_ne!(id0, n2.compute_id());
    }

    #[test]
    fn chain_kind_discriminants_are_stable() {
        // E-4: these strings are identity-bearing — changing them is a schema
        // migration, exactly like the kinds above them.
        assert_eq!(NodeKind::ChainNetwork.discriminant(), "chain_network");
        assert_eq!(NodeKind::ChainContract.discriminant(), "chain_contract");
        assert_eq!(NodeKind::ChainEvent.discriminant(), "chain_event");
        assert_eq!(NodeKind::ChainCheckpoint.discriminant(), "chain_checkpoint");
    }

    #[test]
    fn author_and_plane_are_identity_bearing() {
        let n = sample();
        let id0 = n.compute_id();
        let mut by_author = n.clone();
        by_author.author = "someone-else".into();
        assert_ne!(id0, by_author.compute_id());
        let mut by_plane = n.clone();
        by_plane.plane = Plane::Asserted;
        assert_ne!(id0, by_plane.compute_id());
    }
}
