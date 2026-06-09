//! `mem-query` — the read path over the memory DAG (first slice of MEM-S3).
//!
//! Three ways to ask the graph questions, all tenant-scoped and
//! provenance-carrying:
//! - [`Recall::storyline`] — recency-ordered budget-shaped storyline for a repo.
//! - [`Recall::search`] — semantic search (cosine over node embeddings).
//! - [`Recall::neighbors`] — blast-radius around a node (connected via edges).
//!
//! Every result carries the freshness [`Watermark`] so a caller knows how current
//! the Derived index is, and every item carries its trust tier + source pointer so
//! the caller can judge how much to trust it (provenance-carrying recall).
//!
//! v1 scans the tenant on each call (linear over `all_nodes`). Fast enough at the
//! current scale (~8k nodes); a per-tenant index/CF is a later refinement.

use std::collections::HashMap;

use mem_core::{ContentHash, MemoryNode, NodeKind, Plane, SourceRef, Timestamp, TrustTier};
use mem_index::{BruteForceIndex, Embedder, HashingEmbedder, VectorIndex};
use mem_ingest::{Ingestor, Watermark, EMBED_DIM};
use mem_store::{MemoryDagStore, StoreError};

/// One node in a recall result, flattened for display + decision-making.
#[derive(Debug, Clone)]
pub struct RecallItem {
    pub id: ContentHash,
    pub kind: NodeKind,
    pub repo: String,
    /// Human-readable title (commit subject / doc title / reference slug).
    pub title: String,
    pub valid_from: Timestamp,
    pub plane: Plane,
    pub trust_tier: TrustTier,
    pub source: SourceRef,
    /// Similarity score for `search`; `None` for `storyline`/`neighbors`.
    pub score: Option<f32>,
}

impl RecallItem {
    fn from_node(n: &MemoryNode, score: Option<f32>) -> Self {
        RecallItem {
            id: n.compute_id(),
            kind: n.kind.clone(),
            repo: n.repo.clone(),
            title: String::from_utf8_lossy(&n.content).to_string(),
            valid_from: n.valid_from,
            plane: n.plane,
            trust_tier: n.trust_tier,
            source: n.source_ref.clone(),
            score,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecallResult {
    pub repo: String,
    pub watermark: Option<Watermark>,
    pub total_in_tenant: usize,
    pub items: Vec<RecallItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Out,
    In,
}

#[derive(Debug, Clone)]
pub struct NeighborItem {
    pub edge_kind: mem_core::EdgeKind,
    pub direction: Direction,
    /// The connected node, if it resolves (a dangling edge yields `None`).
    pub node: Option<RecallItem>,
}

/// A minted placeholder reference node (trailer/block target not yet resolved).
fn is_reference(n: &MemoryNode) -> bool {
    matches!(&n.source_ref, SourceRef::DagNative { key } if key.starts_with("ref:"))
}

pub struct Recall<'a> {
    store: &'a MemoryDagStore<MemoryNode>,
    embedder: HashingEmbedder,
}

impl<'a> Recall<'a> {
    pub fn new(store: &'a MemoryDagStore<MemoryNode>) -> Self {
        Self {
            store,
            embedder: HashingEmbedder::new(EMBED_DIM),
        }
    }

    /// All non-reference nodes for a tenant repo.
    fn tenant_nodes(&self, repo: &str) -> Result<Vec<MemoryNode>, StoreError> {
        Ok(self
            .store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.repo == repo && !is_reference(n))
            .collect())
    }

    fn watermark(&self, repo: &str) -> Option<Watermark> {
        Ingestor::new(repo).read_watermark(self.store).ok().flatten()
    }

    /// Recency-ordered storyline for a tenant. `budget` caps the item count.
    pub fn storyline(&self, repo: &str, budget: usize) -> Result<RecallResult, StoreError> {
        let mut nodes = self.tenant_nodes(repo)?;
        let total = nodes.len();
        // Newest first; ties broken by id for determinism.
        nodes.sort_by(|a, b| {
            b.valid_from
                .cmp(&a.valid_from)
                .then_with(|| a.compute_id().cmp(&b.compute_id()))
        });
        let items = nodes.iter().take(budget).map(|n| RecallItem::from_node(n, None)).collect();
        Ok(RecallResult {
            repo: repo.to_string(),
            watermark: self.watermark(repo),
            total_in_tenant: total,
            items,
        })
    }

    /// Semantic search within a tenant: cosine over node embeddings. The query is
    /// embedded in the same space as ingest, so the index's model-version guard
    /// lines up.
    pub fn search(&self, repo: &str, query: &str, budget: usize) -> Result<RecallResult, StoreError> {
        let nodes = self.tenant_nodes(repo)?;
        let total = nodes.len();

        let mut index = BruteForceIndex::new();
        let mut by_id: HashMap<ContentHash, &MemoryNode> = HashMap::new();
        for n in &nodes {
            if let Some(v) = &n.embedding {
                let id = n.compute_id();
                if index.add(id, v).is_ok() {
                    by_id.insert(id, n);
                }
            }
        }

        let q = self.embedder.embed(query).map_err(|e| StoreError::Backend(e.to_string()))?;
        let hits = index.search(&q, budget).unwrap_or_default();
        let items = hits
            .into_iter()
            .filter_map(|nb| by_id.get(&nb.id).map(|n| RecallItem::from_node(n, Some(nb.score))))
            .collect();
        Ok(RecallResult {
            repo: repo.to_string(),
            watermark: self.watermark(repo),
            total_in_tenant: total,
            items,
        })
    }

    /// Blast-radius around a node: nodes reachable by one edge, in or out.
    pub fn neighbors(&self, id: &ContentHash, budget: usize) -> Result<Vec<NeighborItem>, StoreError> {
        let mut out = Vec::new();
        for e in self.store.out_edges(id)? {
            if out.len() >= budget {
                return Ok(out);
            }
            let node = self.store.get_node(&e.to)?.map(|n| RecallItem::from_node(&n, None));
            out.push(NeighborItem { edge_kind: e.kind, direction: Direction::Out, node });
        }
        for e in self.store.in_edges(id)? {
            if out.len() >= budget {
                break;
            }
            let node = self.store.get_node(&e.from)?.map(|n| RecallItem::from_node(&n, None));
            out.push(NeighborItem { edge_kind: e.kind, direction: Direction::In, node });
        }
        Ok(out)
    }

    /// Resolve a (possibly short) hex id prefix to a full node id. Returns `None`
    /// if zero or more than one node matches (ambiguous).
    pub fn resolve_prefix(&self, prefix: &str) -> Result<Option<ContentHash>, StoreError> {
        let mut found: Option<ContentHash> = None;
        for n in self.store.all_nodes()? {
            let id = n.compute_id();
            if id.to_hex().starts_with(prefix) {
                if found.is_some() {
                    return Ok(None); // ambiguous
                }
                found = Some(id);
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_core::{
        Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind, SourceRef, Status,
        SCHEMA_VERSION,
    };
    use mem_store::kv::InMemoryKv;

    fn node(repo: &str, subject: &str, valid_from: Timestamp) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Commit,
            repo: repo.into(),
            author: "t".into(),
            source_ref: SourceRef::GitCommit { repo: repo.into(), sha: subject.into() },
            content: subject.as_bytes().to_vec(),
            valid_from,
            valid_to: None,
            observed_at: valid_from,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: Some(HashingEmbedder::new(EMBED_DIM).embed(subject).unwrap()),
            confidence: vec![],
            anchors: vec![],
            status: Status::Active,
        }
    }

    fn store_with(nodes: &[MemoryNode]) -> MemoryDagStore<MemoryNode> {
        let s = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        s.commit(nodes, &[]).unwrap();
        s
    }

    #[test]
    fn storyline_is_recency_ordered_and_tenant_scoped() {
        let s = store_with(&[
            node("a", "old", 100),
            node("a", "new", 300),
            node("a", "mid", 200),
            node("b", "other-tenant", 999),
        ]);
        let r = Recall::new(&s).storyline("a", 10).unwrap();
        assert_eq!(r.total_in_tenant, 3, "tenant b excluded");
        let titles: Vec<_> = r.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["new", "mid", "old"]);
    }

    #[test]
    fn storyline_respects_budget() {
        let s = store_with(&[node("a", "n1", 1), node("a", "n2", 2), node("a", "n3", 3)]);
        let r = Recall::new(&s).storyline("a", 2).unwrap();
        assert_eq!(r.items.len(), 2);
    }

    #[test]
    fn search_ranks_relevant_node_first() {
        let s = store_with(&[
            node("a", "rocksdb storage durability backend", 1),
            node("a", "gossip peer discovery quic networking", 2),
            node("a", "belnap four valued logic lattice", 3),
        ]);
        let r = Recall::new(&s).search("a", "rocksdb storage", 2).unwrap();
        assert!(!r.items.is_empty());
        assert!(r.items[0].title.contains("rocksdb"), "best match should be the storage node");
        assert!(r.items[0].score.is_some());
    }

    #[test]
    fn neighbors_returns_connected_nodes() {
        let a = node("a", "sprint", 1);
        let b = node("a", "commit", 2);
        let s = store_with(&[a.clone(), b.clone()]);
        let edge = Edge {
            from: b.compute_id(),
            to: a.compute_id(),
            kind: EdgeKind::Implements,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance { method: EdgeMethod::Trailer, asserter: "t".into(), at: 0, evidence: None },
            confidence: vec![],
            quarantined: false,
            signature: None,
        };
        s.add_edge(&edge).unwrap();

        let out = Recall::new(&s).neighbors(&b.compute_id(), 10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].edge_kind, EdgeKind::Implements);
        assert_eq!(out[0].direction, Direction::Out);
        assert_eq!(out[0].node.as_ref().map(|n| n.title.clone()), Some("sprint".to_string()));
    }
}
