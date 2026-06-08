//! `mem-store` — a generic, content-addressed DAG over a [`kv::KvStore`] backend
//! (WP-0.2).
//!
//! This is the v1 replacement for `citrate_consensus::dag_store::DagStore`, which
//! is hardcoded to GHOSTDAG `Block`s and could not be reused (verified
//! 2026-06-05). `MemoryDagStore<N>` stores arbitrary content-addressed nodes plus
//! typed edges, with atomic batch commits and prefix-scanned neighbour queries.
//!
//! Grow-only + content-addressed = the substrate for the Belnap-CRDT federation
//! merge (a later WP): re-adding the same node/edge is idempotent.

pub mod kv;
#[cfg(feature = "rocksdb")]
pub mod rocks;

use std::collections::{HashSet, VecDeque};

use serde::de::DeserializeOwned;
use serde::Serialize;

use mem_core::{ContentHash, Edge, EdgeKind, MemoryNode};

use kv::{KvOp, KvStore};

pub mod cf {
    pub const NODES: &str = "mem_nodes";
    pub const EDGES_OUT: &str = "mem_edges_out"; // key: from ‖ to ‖ kind
    pub const EDGES_IN: &str = "mem_edges_in"; // key: to ‖ from ‖ kind
}

/// Every column family a [`MemoryDagStore`] uses. Pass to `RocksKv::open`.
pub const ALL_CFS: &[&str] = &[cf::NODES, cf::EDGES_OUT, cf::EDGES_IN];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("serialization error: {0}")]
    Serde(String),
}

/// Anything the DAG can store must know its own content-addressed id.
pub trait Identified {
    fn id(&self) -> ContentHash;
}

impl Identified for MemoryNode {
    fn id(&self) -> ContentHash {
        self.compute_id()
    }
}

/// A content-addressed DAG of nodes `N` and [`Edge`]s, backed by any [`KvStore`].
pub struct MemoryDagStore<N> {
    kv: Box<dyn KvStore>,
    _node: std::marker::PhantomData<N>,
}

impl<N> MemoryDagStore<N>
where
    N: Identified + Serialize + DeserializeOwned + Clone,
{
    pub fn new(kv: Box<dyn KvStore>) -> Self {
        Self {
            kv,
            _node: std::marker::PhantomData,
        }
    }

    /// Open a durable RocksDB-backed store at `path`, wiring all required column
    /// families. Requires the `rocksdb` feature.
    #[cfg(feature = "rocksdb")]
    pub fn open_rocksdb<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StoreError> {
        let kv = crate::rocks::RocksKv::open(path, ALL_CFS).map_err(StoreError::Backend)?;
        Ok(Self::new(Box::new(kv)))
    }

    // ---- nodes ----

    /// Insert (or overwrite-in-place) a node. Content-addressing means an
    /// overwrite with the same id carries the same identity-bearing content;
    /// only advisory fields (embedding, confidence, status) can differ.
    pub fn put_node(&self, node: &N) -> Result<ContentHash, StoreError> {
        let id = node.id();
        let bytes = serde_json::to_vec(node).map_err(|e| StoreError::Serde(e.to_string()))?;
        self.kv
            .kv_put(cf::NODES, id.as_bytes(), &bytes)
            .map_err(StoreError::Backend)?;
        Ok(id)
    }

    pub fn get_node(&self, id: &ContentHash) -> Result<Option<N>, StoreError> {
        match self.kv.kv_get(cf::NODES, id.as_bytes()).map_err(StoreError::Backend)? {
            None => Ok(None),
            Some(bytes) => {
                let node = serde_json::from_slice(&bytes).map_err(|e| StoreError::Serde(e.to_string()))?;
                Ok(Some(node))
            }
        }
    }

    pub fn has_node(&self, id: &ContentHash) -> Result<bool, StoreError> {
        self.kv.kv_exists(cf::NODES, id.as_bytes()).map_err(StoreError::Backend)
    }

    pub fn node_count(&self) -> Result<usize, StoreError> {
        Ok(self.kv.kv_iter_cf(cf::NODES).map_err(StoreError::Backend)?.len())
    }

    // ---- edges ----

    fn in_key(edge: &Edge) -> Vec<u8> {
        let mut k = Vec::with_capacity(65);
        k.extend_from_slice(edge.to.as_bytes());
        k.extend_from_slice(edge.from.as_bytes());
        k.push(edge.kind.tag());
        k
    }

    /// Add an edge. Written to both the out- and in-adjacency CFs in one atomic
    /// batch so a half-written edge can never be observed. Idempotent by
    /// (from, to, kind).
    pub fn add_edge(&self, edge: &Edge) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(edge).map_err(|e| StoreError::Serde(e.to_string()))?;
        let ops = vec![
            KvOp::Put {
                cf: cf::EDGES_OUT.into(),
                key: edge.key(),
                value: bytes.clone(),
            },
            KvOp::Put {
                cf: cf::EDGES_IN.into(),
                key: Self::in_key(edge),
                value: bytes,
            },
        ];
        self.kv.kv_write_batch(&ops).map_err(StoreError::Backend)
    }

    /// Atomically commit a batch of nodes and edges (the unit a memory-diff
    /// merge or an ingest tick uses).
    pub fn commit(&self, nodes: &[N], edges: &[Edge]) -> Result<(), StoreError> {
        let mut ops = Vec::with_capacity(nodes.len() + edges.len() * 2);
        for n in nodes {
            let bytes = serde_json::to_vec(n).map_err(|e| StoreError::Serde(e.to_string()))?;
            ops.push(KvOp::Put {
                cf: cf::NODES.into(),
                key: n.id().as_bytes().to_vec(),
                value: bytes,
            });
        }
        for e in edges {
            let bytes = serde_json::to_vec(e).map_err(|e| StoreError::Serde(e.to_string()))?;
            ops.push(KvOp::Put {
                cf: cf::EDGES_OUT.into(),
                key: e.key(),
                value: bytes.clone(),
            });
            ops.push(KvOp::Put {
                cf: cf::EDGES_IN.into(),
                key: Self::in_key(e),
                value: bytes,
            });
        }
        self.kv.kv_write_batch(&ops).map_err(StoreError::Backend)
    }

    fn scan_prefix(&self, cf: &str, prefix: &[u8]) -> Result<Vec<Edge>, StoreError> {
        let mut out = Vec::new();
        for (k, v) in self.kv.kv_iter_cf(cf).map_err(StoreError::Backend)? {
            if k.starts_with(prefix) {
                let e: Edge = serde_json::from_slice(&v).map_err(|e| StoreError::Serde(e.to_string()))?;
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Edges leaving `from`.
    pub fn out_edges(&self, from: &ContentHash) -> Result<Vec<Edge>, StoreError> {
        self.scan_prefix(cf::EDGES_OUT, from.as_bytes())
    }

    /// Edges entering `to`.
    pub fn in_edges(&self, to: &ContentHash) -> Result<Vec<Edge>, StoreError> {
        self.scan_prefix(cf::EDGES_IN, to.as_bytes())
    }

    /// BFS over out-edges of a single kind. Returns reachable node ids (excluding
    /// the start). Cycle-safe via a visited set — important because supersession
    /// is *supposed* to be acyclic but we must never loop even if a bad edge
    /// slips in.
    pub fn reachable_via(&self, start: &ContentHash, kind: EdgeKind) -> Result<Vec<ContentHash>, StoreError> {
        let mut seen: HashSet<ContentHash> = HashSet::new();
        let mut queue: VecDeque<ContentHash> = VecDeque::new();
        queue.push_back(*start);
        seen.insert(*start);
        let mut result = Vec::new();
        while let Some(cur) = queue.pop_front() {
            for e in self.out_edges(&cur)? {
                if e.kind == kind && seen.insert(e.to) {
                    result.push(e.to);
                    queue.push_back(e.to);
                }
            }
        }
        Ok(result)
    }

    /// Would adding `from -supersedes-> to` create a cycle? (i.e. is `from`
    /// already reachable from `to` via Supersedes?) Enforces the TLA+
    /// `SupersededDag.Acyclic` invariant at the write boundary.
    pub fn would_cycle_supersedes(&self, from: &ContentHash, to: &ContentHash) -> Result<bool, StoreError> {
        Ok(self.reachable_via(to, EdgeKind::Supersedes)?.contains(from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kv::InMemoryKv;
    use mem_core::{
        Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind, Plane, SourceRef, Status,
        TrustTier, SCHEMA_VERSION,
    };

    fn node(content: &str) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Commit,
            repo: "r".into(),
            author: "ingest".into(),
            source_ref: SourceRef::DagNative { key: content.into() },
            content: content.as_bytes().to_vec(),
            valid_from: 0,
            valid_to: None,
            observed_at: 0,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: None,
            confidence: vec![],
            anchors: vec![],
            status: Status::Active,
        }
    }

    fn edge(from: &MemoryNode, to: &MemoryNode, kind: EdgeKind) -> Edge {
        Edge {
            from: from.compute_id(),
            to: to.compute_id(),
            kind,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance {
                method: EdgeMethod::Ingest,
                asserter: "ingest".into(),
                at: 0,
                evidence: None,
            },
            confidence: vec![],
            quarantined: false,
            signature: None,
        }
    }

    fn store() -> MemoryDagStore<MemoryNode> {
        MemoryDagStore::new(Box::new(InMemoryKv::new()))
    }

    #[test]
    fn put_then_get_roundtrips() {
        let s = store();
        let n = node("a");
        let id = s.put_node(&n).unwrap();
        assert_eq!(id, n.compute_id());
        assert_eq!(s.get_node(&id).unwrap(), Some(n));
        assert!(s.has_node(&id).unwrap());
        assert_eq!(s.node_count().unwrap(), 1);
    }

    #[test]
    fn put_is_idempotent_by_id() {
        let s = store();
        let n = node("a");
        s.put_node(&n).unwrap();
        s.put_node(&n).unwrap();
        assert_eq!(s.node_count().unwrap(), 1, "same content -> same id -> one node");
    }

    #[test]
    fn edges_are_queryable_both_directions() {
        let s = store();
        let (a, b) = (node("a"), node("b"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        s.add_edge(&edge(&a, &b, EdgeKind::Implements)).unwrap();

        let out = s.out_edges(&a.compute_id()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, b.compute_id());

        let inc = s.in_edges(&b.compute_id()).unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].from, a.compute_id());

        // No phantom edges the other way.
        assert!(s.out_edges(&b.compute_id()).unwrap().is_empty());
    }

    #[test]
    fn commit_is_atomic_batch() {
        let s = store();
        let (a, b, c) = (node("a"), node("b"), node("c"));
        let edges = vec![edge(&a, &b, EdgeKind::TemporalNext), edge(&b, &c, EdgeKind::TemporalNext)];
        s.commit(&[a.clone(), b.clone(), c.clone()], &edges).unwrap();
        assert_eq!(s.node_count().unwrap(), 3);
        assert_eq!(s.out_edges(&a.compute_id()).unwrap().len(), 1);
    }

    #[test]
    fn reachable_and_cycle_guard() {
        let s = store();
        let (a, b, c) = (node("a"), node("b"), node("c"));
        for n in [&a, &b, &c] {
            s.put_node(n).unwrap();
        }
        // a supersedes b, b supersedes c
        s.add_edge(&edge(&a, &b, EdgeKind::Supersedes)).unwrap();
        s.add_edge(&edge(&b, &c, EdgeKind::Supersedes)).unwrap();

        let reach = s.reachable_via(&a.compute_id(), EdgeKind::Supersedes).unwrap();
        assert!(reach.contains(&b.compute_id()));
        assert!(reach.contains(&c.compute_id()));

        // Adding c -supersedes-> a would close a cycle a->b->c->a.
        assert!(s
            .would_cycle_supersedes(&c.compute_id(), &a.compute_id())
            .unwrap());
        // Adding a fresh node superseding a does not.
        let d = node("d");
        s.put_node(&d).unwrap();
        assert!(!s
            .would_cycle_supersedes(&d.compute_id(), &a.compute_id())
            .unwrap());
    }
}
