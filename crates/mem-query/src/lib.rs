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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mem_core::{ContentHash, MemoryNode, NodeKind, Plane, SourceRef, Status, Timestamp, TrustTier};
use mem_index::{BruteForceIndex, Embedder, HashingEmbedder, HnswIndex, VectorIndex};
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
    /// Lifecycle status (WP-1.4): callers must be able to see that a node is
    /// Superseded so they never act on a stale memory unknowingly.
    pub status: Status,
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
            status: n.status,
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
    /// `true` for a proposed (quarantined) edge — advisory, not load-bearing,
    /// until confirmed (MEM-S4 WP-4.1). Callers must surface this.
    pub quarantined: bool,
    /// The connected node, if it resolves (a dangling edge yields `None`).
    pub node: Option<RecallItem>,
}

/// One cross-tenant analogy candidate (MEM-S4 WP-4.3, decision #7).
#[derive(Debug, Clone)]
pub struct AnalogyCandidate {
    pub item: RecallItem,
    /// Coarse signal: embedding cosine vs the anchor.
    pub cosine: f32,
    /// Fine signal: structural edge-kind-signature overlap (multiset Jaccard
    /// over (direction, edge-kind) of load-bearing edges).
    pub structural: f32,
    /// Combined ranking score.
    pub score: f32,
}

/// v1 ranking weights for the coarse-to-fine combination. The embedding does
/// the finding; the structure ("shape of prior actions") does the vetting.
const ANALOGY_COSINE_WEIGHT: f32 = 0.7;
const ANALOGY_STRUCTURAL_WEIGHT: f32 = 0.3;
/// Shortlist factor: how many coarse candidates survive to the structural pass.
const ANALOGY_SHORTLIST_FACTOR: usize = 4;

/// A minted placeholder reference node (trailer/block target not yet resolved).
fn is_reference(n: &MemoryNode) -> bool {
    matches!(&n.source_ref, SourceRef::DagNative { key } if key.starts_with("ref:"))
}

/// The embedding model id a tenant's nodes were built with (the first embedded
/// node wins). Lets a caller pick a matching query embedder so the index's
/// model-version guard lines up instead of silently rejecting every query.
pub fn detect_embedding_model(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
) -> Result<Option<String>, StoreError> {
    Ok(store
        .all_nodes()?
        .into_iter()
        .find(|n| n.repo == repo && n.embedding.is_some())
        .and_then(|n| n.embedding.map(|v| v.model)))
}

/// Store-wide variant of [`detect_embedding_model`]: the embedding model of the
/// first embedded node in any tenant. A federation backfill embeds every tenant
/// with one model, so this is enough for a server to pick its query embedder once
/// at startup.
pub fn detect_store_embedding_model(
    store: &MemoryDagStore<MemoryNode>,
) -> Result<Option<String>, StoreError> {
    Ok(store
        .all_nodes()?
        .into_iter()
        .find_map(|n| n.embedding.map(|v| v.model)))
}

/// One cached per-tenant HNSW index plus the store state it was built from.
struct CachedTenantIndex {
    watermark: Option<Watermark>,
    /// Global node count at build time — catches Asserted-plane writes, which
    /// land without bumping the ingest watermark.
    node_count: usize,
    /// Tenant size at build (incl. nodes without embeddings), for `total_in_tenant`.
    tenant_total: usize,
    index: Arc<HnswIndex>,
}

/// Cross-query (and, in the daemon, cross-session) cache of per-tenant
/// [`HnswIndex`]es — what makes search sub-linear in practice. Building any
/// ANN index is itself a full scan, so it only pays off amortised: build once
/// per (tenant, store-state), then every later query skips both the tenant
/// scan and the brute-force pass. An entry is invalidated when the repo's
/// ingest watermark or the global node count changes.
#[derive(Default)]
pub struct TenantIndexCache {
    entries: Mutex<HashMap<String, CachedTenantIndex>>,
    builds: AtomicUsize,
    hits: AtomicUsize,
}

impl TenantIndexCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many indexes have been (re)built — observable for tests and ops.
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::Relaxed)
    }

    /// How many lookups were served from cache.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    /// Return the cached index for `repo` if it still matches the store state,
    /// else build, cache, and return a fresh one. The lock is held across the
    /// build so concurrent first-queries do one build, not N.
    fn get_or_build<F>(
        &self,
        repo: &str,
        watermark: &Option<Watermark>,
        node_count: usize,
        build: F,
    ) -> Result<(Arc<HnswIndex>, usize), StoreError>
    where
        F: FnOnce() -> Result<(HnswIndex, usize), StoreError>,
    {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| StoreError::Backend("tenant index cache poisoned".into()))?;
        if let Some(e) = entries.get(repo) {
            if e.watermark == *watermark && e.node_count == node_count {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok((Arc::clone(&e.index), e.tenant_total));
            }
        }
        let (index, tenant_total) = build()?;
        self.builds.fetch_add(1, Ordering::Relaxed);
        let index = Arc::new(index);
        entries.insert(
            repo.to_string(),
            CachedTenantIndex {
                watermark: watermark.clone(),
                node_count,
                tenant_total,
                index: Arc::clone(&index),
            },
        );
        Ok((index, tenant_total))
    }
}

pub struct Recall<'a> {
    store: &'a MemoryDagStore<MemoryNode>,
    embedder: Box<dyn Embedder>,
    index_cache: Option<Arc<TenantIndexCache>>,
}

impl<'a> Recall<'a> {
    /// Default recall: the dependency-free hashing embedder. `search` only works if
    /// the tenant's nodes were embedded with the same model (the index guard
    /// rejects a mismatch), so use [`with_embedder`](Recall::with_embedder) to match
    /// a transformer-embedded graph.
    pub fn new(store: &'a MemoryDagStore<MemoryNode>) -> Self {
        Self {
            store,
            embedder: Box::new(HashingEmbedder::new(EMBED_DIM)),
            index_cache: None,
        }
    }

    /// Recall whose query embedder is explicit — must match the model the tenant's
    /// nodes were embedded with, or `search`'s vector-space guard rejects the query.
    pub fn with_embedder(store: &'a MemoryDagStore<MemoryNode>, embedder: Box<dyn Embedder>) -> Self {
        Self { store, embedder, index_cache: None }
    }

    /// Serve `search` from a shared [`TenantIndexCache`] (HNSW, built once per
    /// tenant per store-state) instead of scanning + brute-forcing per query.
    pub fn with_index_cache(mut self, cache: Arc<TenantIndexCache>) -> Self {
        self.index_cache = Some(cache);
        self
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
        if let Some(cache) = &self.index_cache {
            return self.search_cached(Arc::clone(cache), repo, query, budget);
        }
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

    /// `search` served from the shared HNSW cache: reuse the tenant's index when
    /// the store hasn't changed, else rebuild it (one full scan, amortised over
    /// every following query). Hits resolve to nodes by id — `budget` point reads,
    /// not a tenant scan.
    fn search_cached(
        &self,
        cache: Arc<TenantIndexCache>,
        repo: &str,
        query: &str,
        budget: usize,
    ) -> Result<RecallResult, StoreError> {
        let watermark = self.watermark(repo);
        let node_count = self.store.node_count()?;
        let (index, total) = cache.get_or_build(repo, &watermark, node_count, || {
            let nodes = self.tenant_nodes(repo)?;
            let mut index = HnswIndex::new();
            for n in &nodes {
                if let Some(v) = &n.embedding {
                    // Skip vectors from a mismatched model, like the uncached path.
                    let _ = index.add(n.compute_id(), v);
                }
            }
            Ok((index, nodes.len()))
        })?;

        let q = self.embedder.embed(query).map_err(|e| StoreError::Backend(e.to_string()))?;
        let hits = index.search(&q, budget).unwrap_or_default();
        let mut items = Vec::with_capacity(hits.len());
        for nb in hits {
            if let Some(n) = self.store.get_node(&nb.id)? {
                items.push(RecallItem::from_node(&n, Some(nb.score)));
            }
        }
        Ok(RecallResult {
            repo: repo.to_string(),
            watermark,
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
            out.push(NeighborItem { edge_kind: e.kind, direction: Direction::Out, quarantined: e.quarantined, node });
        }
        for e in self.store.in_edges(id)? {
            if out.len() >= budget {
                break;
            }
            let node = self.store.get_node(&e.from)?.map(|n| RecallItem::from_node(&n, None));
            out.push(NeighborItem { edge_kind: e.kind, direction: Direction::In, quarantined: e.quarantined, node });
        }
        Ok(out)
    }

    /// Structural edge-kind signature of a node: a multiset of
    /// (direction, edge-kind) over its **load-bearing** edges. Quarantined
    /// proposals are excluded — an unconfirmed edge must not influence
    /// analogy ranking (it could launder itself into confirmations).
    fn edge_signature(&self, id: &ContentHash) -> Result<HashMap<(u8, u8), usize>, StoreError> {
        let mut sig: HashMap<(u8, u8), usize> = HashMap::new();
        for e in self.store.out_edges(id)? {
            if !e.quarantined {
                *sig.entry((0, e.kind.tag())).or_default() += 1;
            }
        }
        for e in self.store.in_edges(id)? {
            if !e.quarantined {
                *sig.entry((1, e.kind.tag())).or_default() += 1;
            }
        }
        Ok(sig)
    }

    /// Multiset Jaccard overlap of two signatures, in [0, 1]. Two edgeless
    /// nodes score 0 (no structural evidence, not perfect agreement).
    fn signature_overlap(a: &HashMap<(u8, u8), usize>, b: &HashMap<(u8, u8), usize>) -> f32 {
        let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
        let (mut inter, mut union) = (0usize, 0usize);
        for k in keys {
            let (x, y) = (*a.get(k).unwrap_or(&0), *b.get(k).unwrap_or(&0));
            inter += x.min(y);
            union += x.max(y);
        }
        if union == 0 {
            0.0
        } else {
            inter as f32 / union as f32
        }
    }

    /// Cross-tenant analogy (MEM-S4 WP-4.3, decision #7 coarse-to-fine):
    /// shortlist by embedding cosine against the anchor's stored vector across
    /// `candidate_repos` (the caller passes only tenants the session may read —
    /// the R3 grant intersection lives at the authz boundary), then verify by
    /// structural edge-kind-signature overlap and rank by the combined score.
    /// An anchor without an embedding has no coarse signal: empty result.
    pub fn analogies(
        &self,
        anchor_id: &ContentHash,
        candidate_repos: &[String],
        budget: usize,
    ) -> Result<Vec<AnalogyCandidate>, StoreError> {
        let Some(anchor) = self.store.get_node(anchor_id)? else {
            return Err(StoreError::Backend("analogy anchor not in store".into()));
        };
        let Some(anchor_vec) = &anchor.embedding else {
            return Ok(Vec::new());
        };
        let anchor_norm: f32 = anchor_vec.data.iter().map(|x| x * x).sum::<f32>().sqrt();
        if anchor_norm == 0.0 {
            return Ok(Vec::new());
        }

        // Coarse: cosine shortlist across the readable candidate tenants.
        let mut shortlist: Vec<(f32, MemoryNode)> = Vec::new();
        for n in self.store.all_nodes()? {
            if n.repo == anchor.repo || !candidate_repos.contains(&n.repo) || is_reference(&n) {
                continue;
            }
            let Some(v) = &n.embedding else { continue };
            // Never compare across vector spaces (the index guard's rule).
            if v.model != anchor_vec.model {
                continue;
            }
            let dot: f32 = v.data.iter().zip(&anchor_vec.data).map(|(x, y)| x * y).sum();
            let norm: f32 = v.data.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm == 0.0 {
                continue;
            }
            shortlist.push((dot / (norm * anchor_norm), n));
        }
        shortlist.sort_by(|a, b| {
            b.0.total_cmp(&a.0).then_with(|| a.1.compute_id().cmp(&b.1.compute_id()))
        });
        shortlist.truncate((budget * ANALOGY_SHORTLIST_FACTOR).max(budget));

        // Fine: structural verify + combined rank.
        let anchor_sig = self.edge_signature(anchor_id)?;
        let mut out: Vec<AnalogyCandidate> = Vec::with_capacity(shortlist.len());
        for (cosine, n) in shortlist {
            let structural = Self::signature_overlap(&anchor_sig, &self.edge_signature(&n.compute_id())?);
            out.push(AnalogyCandidate {
                item: RecallItem::from_node(&n, None),
                cosine,
                structural,
                score: ANALOGY_COSINE_WEIGHT * cosine + ANALOGY_STRUCTURAL_WEIGHT * structural,
            });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.item.id.cmp(&b.item.id)));
        out.truncate(budget);
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
    fn cached_search_matches_uncached_and_reuses_index() {
        // Graded relevance (2 / 1 / 0 query tokens shared) so every rank is
        // determined — ties among zero-overlap items are float noise, not ranking.
        let s = store_with(&[
            node("a", "rocksdb storage durability backend", 1),
            node("a", "storage compaction write amplification", 2),
            node("a", "belnap four valued logic lattice", 3),
            node("b", "other tenant noise", 4),
        ]);
        let cache = Arc::new(TenantIndexCache::new());
        let plain = Recall::new(&s).search("a", "rocksdb storage", 2).unwrap();
        let cached_recall = Recall::new(&s).with_index_cache(Arc::clone(&cache));

        let first = cached_recall.search("a", "rocksdb storage", 2).unwrap();
        let second = cached_recall.search("a", "rocksdb storage", 2).unwrap();

        for r in [&first, &second] {
            assert_eq!(r.total_in_tenant, plain.total_in_tenant);
            let plain_titles: Vec<_> = plain.items.iter().map(|i| i.title.as_str()).collect();
            let titles: Vec<_> = r.items.iter().map(|i| i.title.as_str()).collect();
            assert_eq!(titles, plain_titles, "cached search must rank like the exact path");
        }
        assert_eq!(cache.builds(), 1, "one build serves both queries");
        assert_eq!(cache.hits(), 1, "second query came from cache");
    }

    #[test]
    fn cache_rebuilds_when_store_changes() {
        let s = store_with(&[node("a", "rocksdb storage durability backend", 1)]);
        let cache = Arc::new(TenantIndexCache::new());
        let recall = Recall::new(&s).with_index_cache(Arc::clone(&cache));

        let r = recall.search("a", "consensus tip selection ghostdag", 5).unwrap();
        assert_eq!(r.total_in_tenant, 1);
        assert_eq!(cache.builds(), 1);

        // An Asserted-plane-style write lands without bumping the watermark; the
        // node-count check must still invalidate the entry.
        s.put_node(&node("a", "ghostdag tip selection consensus", 9)).unwrap();
        let r = recall.search("a", "consensus tip selection ghostdag", 5).unwrap();
        assert_eq!(cache.builds(), 2, "store change must rebuild the index");
        assert_eq!(r.total_in_tenant, 2);
        assert!(
            r.items[0].title.contains("ghostdag"),
            "the new node must be searchable immediately"
        );
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
    fn recall_surfaces_superseded_status() {
        let old = node("a", "old decision", 1);
        let new = node("a", "new decision", 2);
        let s = store_with(&[old.clone(), new.clone()]);
        let edge = Edge {
            from: new.compute_id(),
            to: old.compute_id(),
            kind: EdgeKind::Supersedes,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance { method: EdgeMethod::Trailer, asserter: "t".into(), at: 7, evidence: None },
            confidence: vec![],
            quarantined: false,
            signature: None,
        };
        s.apply_supersession(&edge).unwrap();

        let r = Recall::new(&s).storyline("a", 10).unwrap();
        let by_title = |t: &str| r.items.iter().find(|i| i.title == t).map(|i| i.status);
        assert_eq!(by_title("old decision"), Some(Status::Superseded), "stale memory is visibly marked");
        assert_eq!(by_title("new decision"), Some(Status::Active));
    }

    fn plain_edge(from: &MemoryNode, to: &MemoryNode, kind: EdgeKind, quarantined: bool) -> Edge {
        Edge {
            from: from.compute_id(),
            to: to.compute_id(),
            kind,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance { method: EdgeMethod::Trailer, asserter: "t".into(), at: 0, evidence: None },
            confidence: vec![],
            quarantined,
            signature: None,
        }
    }

    #[test]
    fn analogies_rank_structural_matches_above_bare_cosine_twins() {
        // Anchor in tenant a, with one load-bearing Implements out-edge.
        let anchor = node("a", "rocksdb storage engine compaction", 1);
        let impl_target = node("a", "storage work package", 2);
        // Two candidates in tenant b with IDENTICAL text (same cosine):
        // one mirrors the anchor's edge shape, one only has a QUARANTINED edge
        // (which must not count as structure).
        // Same token multiset → identical hashing vector (same cosine), but
        // different byte order → distinct content-addressed ids.
        let twin_structured = node("b", "rocksdb storage engine compaction notes", 3);
        let twin_bare = node("b", "compaction notes rocksdb storage engine", 4);
        let b_target = node("b", "storage work package b", 5);
        // A tenant outside the candidate list must never appear.
        let outsider = node("c", "rocksdb storage engine compaction notes", 6);

        let s = store_with(&[
            anchor.clone(),
            impl_target.clone(),
            twin_structured.clone(),
            twin_bare.clone(),
            b_target.clone(),
            outsider,
        ]);
        s.add_edge(&plain_edge(&anchor, &impl_target, EdgeKind::Implements, false)).unwrap();
        s.add_edge(&plain_edge(&twin_structured, &b_target, EdgeKind::Implements, false)).unwrap();
        s.add_edge(&plain_edge(&twin_bare, &b_target, EdgeKind::Implements, true)).unwrap(); // proposal only

        let r = Recall::new(&s);
        let hits = r.analogies(&anchor.compute_id(), &["b".to_string()], 5).unwrap();
        assert_eq!(hits.len(), 3, "only tenant b candidates (c not in the readable list)");
        assert_eq!(hits[0].item.id, twin_structured.compute_id(), "matching edge shape wins the tie");
        assert!(hits[0].structural > 0.0);
        let bare = hits.iter().find(|h| h.item.id == twin_bare.compute_id()).unwrap();
        assert_eq!(bare.structural, 0.0, "quarantined edges contribute no structure");
        assert!(hits[0].score > bare.score);
    }

    #[test]
    fn analogies_never_cross_vector_spaces_or_leave_candidate_repos() {
        let anchor = node("a", "gossip networking peers", 1);
        let mut alien = node("b", "gossip networking peers", 2);
        // Same text but a different embedding model: must be skipped, not compared.
        alien.embedding = Some(mem_index::HashingEmbedder::new(64).embed("gossip networking peers").unwrap());
        let s = store_with(&[anchor.clone(), alien]);
        let hits = Recall::new(&s).analogies(&anchor.compute_id(), &["b".to_string()], 5).unwrap();
        assert!(hits.is_empty(), "cross-model vectors are never cosine-compared");
        // And with no candidate repos at all, nothing comes back.
        let hits = Recall::new(&s).analogies(&anchor.compute_id(), &[], 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn neighbors_marks_proposed_edges() {
        let a = node("a", "anchor node", 1);
        let b = node("a", "proposed peer", 2);
        let s = store_with(&[a.clone(), b.clone()]);
        s.add_edge(&plain_edge(&a, &b, EdgeKind::References, true)).unwrap();
        let out = Recall::new(&s).neighbors(&a.compute_id(), 10).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].quarantined, "proposal visibly marked in the read path");
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
