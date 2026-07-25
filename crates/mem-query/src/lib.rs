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

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mem_core::{ContentHash, MemoryNode, NodeKind, Plane, SourceRef, Status, Timestamp, TrustTier};
use mem_index::{BruteForceIndex, Embedder, HashingEmbedder, HnswIndex, VectorIndex};
use mem_ingest::{Ingestor, Watermark};
use mem_store::{MemoryDagStore, StoreError};

/// The default hashing-embedder dimension `Recall::new` queries in. Re-exported
/// so a writer can embed into the same space the reader will search, without
/// taking a dependency on mem-ingest just for the constant.
pub use mem_ingest::EMBED_DIM;

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
    /// ADR-09: when this item is in-flight (reachable only from a feature branch,
    /// not yet merged), the branch it lives on. `None` = canonical (merged) truth.
    /// Only ever populated when the caller opts in via `with_in_flight(true)`.
    pub in_flight_branch: Option<String>,
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
            in_flight_branch: None,
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

/// Signature posture of a node, as reported by [`Recall::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// Derived plane: trusted by construction (deterministic projection of a
    /// canonical artifact), so a signature is not applicable.
    NotApplicableDerived,
    /// Asserted plane: the ed25519 signature over the content id holds.
    Valid,
    /// Asserted plane: claims to be signed but the signature does not verify.
    Invalid(String),
    /// Asserted plane: no signature present (should never land via the write
    /// boundary, but reported honestly if encountered).
    Missing,
}

/// The provenance answer for a single node (WP-2.4 `verify`): everything a caller
/// needs to decide whether to trust and act on a memory.
#[derive(Debug, Clone)]
pub struct Verification {
    pub item: RecallItem,
    pub signature: SignatureStatus,
    /// Non-quarantined `Supersedes` edges pointing at this node (newer nodes that
    /// override it). Non-empty ⇒ this memory is stale.
    pub superseded_by: Vec<ContentHash>,
    /// Non-quarantined `Refutes`/`Contradicts` edges pointing at this node.
    pub refuted_by: Vec<ContentHash>,
    /// The node's Belnap confidence records a contradiction (`Both`).
    pub contradicted: bool,
}

impl Verification {
    /// A node is "safe to act on" iff its signature posture is acceptable AND it
    /// is current (not superseded), not refuted, and not contradicted.
    pub fn is_trustworthy(&self) -> bool {
        matches!(self.signature, SignatureStatus::NotApplicableDerived | SignatureStatus::Valid)
            && self.superseded_by.is_empty()
            && self.refuted_by.is_empty()
            && !self.contradicted
            && self.item.status == Status::Active
    }
}

/// One completeness gap the self-critic ([`Recall::critique`]) found in a recall
/// result — something an agent acting on the result might be missing.
#[derive(Debug, Clone, PartialEq)]
pub enum Gap {
    /// The budget hid part of the tenant (`shown` of `total`).
    TruncatedCoverage { shown: usize, total: usize },
    /// Superseded node(s) are present in the result — acting on stale memory.
    SupersededInResult(Vec<ContentHash>),
    /// A returned node carries a Belnap `Both` contradiction.
    Contradicted(ContentHash),
    /// A returned node has an adjacent quarantined (proposed, unconfirmed) edge
    /// the result didn't surface.
    AdjacentProposal(ContentHash),
}

/// The self-critic's verdict over a recall result (WP-3.5 / D3.5).
#[derive(Debug, Clone)]
pub struct Critique {
    pub gaps: Vec<Gap>,
    /// Milliseconds between the result's freshness watermark and `now`; `None`
    /// if the tenant has no watermark.
    pub watermark_age_ms: Option<Timestamp>,
    /// 1.0 = no gaps; decreases as gaps accumulate. Advisory shaping signal.
    pub completeness: f32,
}

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
    /// ADR-09 B.4: surface the in-flight branch layer in `storyline`/`search`.
    /// Off by default — canonical (default-branch) truth only.
    include_in_flight: bool,
    /// Widen `as_of` to the Asserted plane. Off by default — an as-of snapshot is
    /// a deterministic replay, and signed claims are not a deterministic
    /// projection. See [`with_asserted`](Recall::with_asserted).
    include_asserted: bool,
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
            include_in_flight: false,
            include_asserted: false,
        }
    }

    /// Recall whose query embedder is explicit — must match the model the tenant's
    /// nodes were embedded with, or `search`'s vector-space guard rejects the query.
    pub fn with_embedder(store: &'a MemoryDagStore<MemoryNode>, embedder: Box<dyn Embedder>) -> Self {
        Self { store, embedder, index_cache: None, include_in_flight: false, include_asserted: false }
    }

    /// Serve `search` from a shared [`TenantIndexCache`] (HNSW, built once per
    /// tenant per store-state) instead of scanning + brute-forcing per query.
    pub fn with_index_cache(mut self, cache: Arc<TenantIndexCache>) -> Self {
        self.index_cache = Some(cache);
        self
    }

    /// Surface the in-flight branch layer (ADR-09 B.4) in `storyline`/`search`:
    /// commits reachable only from a feature branch are included and each carries
    /// its `in_flight_branch`. Off by default — canonical (default-branch) truth
    /// only, so the trust guarantee holds unless a caller explicitly opts in.
    pub fn with_in_flight(mut self, yes: bool) -> Self {
        self.include_in_flight = yes;
        self
    }

    /// Widen [`as_of`](Recall::as_of) to include the Asserted plane, changing the
    /// question it answers from *"what did the deterministic record show at T"* to
    /// *"what was claimed, and still stood, at T"*.
    ///
    /// Off by default, and deliberately so: the default snapshot is a
    /// **deterministic replay**, reproducible because the Derived plane is a
    /// projection of git and markdown. Signed claims are not reproducible in that
    /// sense, so opting in forfeits replay-stability in exchange for being able to
    /// time-filter claims at all.
    ///
    /// Without this there is no way to ask whether a signed claim is still current,
    /// which leaves `valid_from` on an Asserted node write-only: correctly stored
    /// and read by nothing. The bitemporal filter itself is identical either way,
    /// and `apply_supersession` stamps `valid_to`, so a superseded claim drops out
    /// of the snapshot exactly as a retired Derived node does.
    pub fn with_asserted(mut self, yes: bool) -> Self {
        self.include_asserted = yes;
        self
    }

    /// Non-reference tenant nodes plus the in-flight commit→branch label map. By
    /// default (`include_in_flight = false`) the in-flight branch layer (ADR-09) is
    /// excluded — `Branch` meta-nodes and any commit held by an Active branch's
    /// `BranchContains` edge — so canonical recall is pure. When opted in, in-flight
    /// commits are kept (and the map lets the caller label them); `Branch` meta-nodes
    /// are always excluded from results (they are the mechanism, not content).
    fn tenant_scan(
        &self,
        repo: &str,
    ) -> Result<(Vec<MemoryNode>, HashMap<ContentHash, String>), StoreError> {
        let all = self.store.all_nodes()?;
        let in_flight = self.in_flight_map(repo, &all)?;
        let nodes = all
            .into_iter()
            .filter(|n| {
                n.repo == repo
                    && !is_reference(n)
                    && n.kind != NodeKind::Branch
                    && (self.include_in_flight || !in_flight.contains_key(&n.compute_id()))
            })
            .collect();
        Ok((nodes, in_flight))
    }

    fn tenant_nodes(&self, repo: &str) -> Result<Vec<MemoryNode>, StoreError> {
        Ok(self.tenant_scan(repo)?.0)
    }

    /// Map of in-flight commit id → the branch that contains it (ADR-09). In-flight is
    /// defined by **reachability**: a commit that a `BranchContains` edge points at but
    /// that is NOT reachable from the default watermark head (the canonical spine). This
    /// is robust to every trigger order and to force-push orphans — only genuinely
    /// merged commits (reachable from the default head) ever count as canonical, so a
    /// branch node's status is irrelevant to purity (reap is pure housekeeping).
    fn in_flight_map(
        &self,
        repo: &str,
        all: &[MemoryNode],
    ) -> Result<HashMap<ContentHash, String>, StoreError> {
        let reachable = self.canonical_reachable(repo, all)?;
        let mut map = HashMap::new();
        // Active branches first so a live branch's name wins the label over an archived
        // one when both point at the same still-unmerged commit.
        for want_active in [true, false] {
            for n in all {
                if n.repo == repo && n.kind == NodeKind::Branch && (n.status == Status::Active) == want_active {
                    let branch = String::from_utf8_lossy(&n.content)
                        .rsplit_once('@')
                        .map(|(name, _)| name.to_string())
                        .unwrap_or_default();
                    for e in self.store.out_edges(&n.compute_id())? {
                        if e.kind == mem_core::EdgeKind::BranchContains && !reachable.contains(&e.to) {
                            map.entry(e.to).or_insert_with(|| branch.clone());
                        }
                    }
                }
            }
        }
        Ok(map)
    }

    /// The set of commit ids reachable from the tenant's default watermark head via
    /// the commit spine (`TemporalNext`/`MergeParent`) — i.e. the canonical (merged)
    /// history. Empty when the tenant has no watermark yet.
    fn canonical_reachable(
        &self,
        repo: &str,
        all: &[MemoryNode],
    ) -> Result<HashSet<ContentHash>, StoreError> {
        let head_sha = match self.watermark(repo).and_then(|w| w.head) {
            Some(h) => h,
            None => return Ok(HashSet::new()),
        };
        let mut sha_to_id: HashMap<&str, ContentHash> = HashMap::new();
        for n in all {
            if n.repo == repo && n.kind == NodeKind::Commit {
                if let SourceRef::GitCommit { sha, .. } = &n.source_ref {
                    sha_to_id.insert(sha.as_str(), n.compute_id());
                }
            }
        }
        let head_id = match sha_to_id.get(head_sha.as_str()) {
            Some(id) => *id,
            None => return Ok(HashSet::new()),
        };
        let mut parents: HashMap<ContentHash, Vec<ContentHash>> = HashMap::new();
        for e in self.store.all_edges()? {
            if matches!(e.kind, mem_core::EdgeKind::TemporalNext | mem_core::EdgeKind::MergeParent) {
                parents.entry(e.from).or_default().push(e.to);
            }
        }
        let mut reachable = HashSet::new();
        let mut stack = vec![head_id];
        while let Some(id) = stack.pop() {
            if reachable.insert(id) {
                if let Some(ps) = parents.get(&id) {
                    stack.extend(ps.iter().copied());
                }
            }
        }
        Ok(reachable)
    }

    fn watermark(&self, repo: &str) -> Option<Watermark> {
        Ingestor::new(repo).read_watermark(self.store).ok().flatten()
    }

    /// Recency-ordered storyline for a tenant. `budget` caps the item count.
    pub fn storyline(&self, repo: &str, budget: usize) -> Result<RecallResult, StoreError> {
        let (mut nodes, in_flight) = self.tenant_scan(repo)?;
        let total = nodes.len();
        // Newest first; ties broken by id for determinism.
        nodes.sort_by(|a, b| {
            b.valid_from
                .cmp(&a.valid_from)
                .then_with(|| a.compute_id().cmp(&b.compute_id()))
        });
        let items = nodes
            .iter()
            .take(budget)
            .map(|n| {
                let mut item = RecallItem::from_node(n, None);
                item.in_flight_branch = in_flight.get(&n.compute_id()).cloned();
                item
            })
            .collect();
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
        // The HNSW cache holds the canonical set; an in-flight query takes the brute
        // path (which labels hits) so the cache never mixes the two node sets.
        if !self.include_in_flight {
            if let Some(cache) = &self.index_cache {
                return self.search_cached(Arc::clone(cache), repo, query, budget);
            }
        }
        let (nodes, in_flight) = self.tenant_scan(repo)?;
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
            .filter_map(|nb| {
                by_id.get(&nb.id).map(|n| {
                    let mut item = RecallItem::from_node(n, Some(nb.score));
                    item.in_flight_branch = in_flight.get(&nb.id).cloned();
                    item
                })
            })
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

    /// As-of / decision-replay (WP-3.4, D3.4): reconstruct what the **Derived**
    /// plane knew about a tenant at a point in time `as_of_ms`.
    ///
    /// A node is "known and current as of T" iff it was already valid
    /// (`valid_from <= T`) and had not yet been retired (`valid_to` is `None` or
    /// `> T`). This is a pure bitemporal filter — the Derived plane is a
    /// deterministic projection of git/markdown, so replaying it at T is
    /// well-defined and reproducible (the core invariant). The Asserted plane is
    /// append-only *signed* claims, not a deterministic projection, so
    /// decision-replay is scoped to Derived only: an "as-of" snapshot answers
    /// "what did the deterministic record show," not "what had anyone claimed."
    ///
    /// `valid_from` is excluded from `compute_id`, but for dated Derived nodes it
    /// is itself deterministic (frontmatter `created:` / commit date — see
    /// `mem-ingest` F-3), so the snapshot is stable across rebuilds.
    ///
    /// [`with_asserted`](Recall::with_asserted) opts into the Asserted plane for
    /// callers that need to ask the *other* question, "what was claimed and still
    /// stood at T". That is a different question, not a better answer to this one,
    /// which is why it is a separate opt-in rather than a widening of the default.
    pub fn as_of(&self, repo: &str, as_of_ms: Timestamp, budget: usize) -> Result<RecallResult, StoreError> {
        let nodes = self.tenant_nodes(repo)?;
        let total = nodes.len();
        let mut current: Vec<&MemoryNode> = nodes
            .iter()
            .filter(|n| self.include_asserted || n.plane == Plane::Derived)
            .filter(|n| n.valid_from <= as_of_ms)
            .filter(|n| n.valid_to.map(|vt| vt > as_of_ms).unwrap_or(true))
            .collect();
        // Newest-as-of-T first; ties broken by id for determinism.
        current.sort_by(|a, b| {
            b.valid_from
                .cmp(&a.valid_from)
                .then_with(|| a.compute_id().cmp(&b.compute_id()))
        });
        let items = current.iter().take(budget).map(|n| RecallItem::from_node(n, None)).collect();
        Ok(RecallResult {
            repo: repo.to_string(),
            watermark: self.watermark(repo),
            total_in_tenant: total,
            items,
        })
    }

    /// Provenance verification (WP-2.4 `verify`): answer "can I trust this node,
    /// and is it still current?" for a single node by id.
    ///
    /// Gathers, in one pass, everything a caller needs to decide whether to act on
    /// a memory: its plane/tier/status, whether its Asserted-plane signature
    /// actually holds (re-checked here, not assumed), whether it has been
    /// superseded or refuted (and by what), and whether its Belnap confidence
    /// records a contradiction. A `Derived` node verifies by construction (it
    /// points at a canonical artifact and is rebuildable); an `Asserted` node
    /// verifies iff its signature over the content id holds.
    pub fn verify(&self, id: &ContentHash) -> Result<Option<Verification>, StoreError> {
        let node = match self.store.get_node(id)? {
            Some(n) => n,
            None => return Ok(None),
        };

        // Signature check. Derived nodes are trusted by construction (deterministic
        // projection of a canonical artifact); Asserted nodes must carry a valid
        // ed25519 signature over their content id (mem-assert is the authority).
        let signature = match node.plane {
            Plane::Derived => SignatureStatus::NotApplicableDerived,
            Plane::Asserted => match mem_assert::verify_node(&node) {
                Ok(()) => SignatureStatus::Valid,
                Err(mem_assert::AssertError::Unsigned) => SignatureStatus::Missing,
                Err(e) => SignatureStatus::Invalid(e.to_string()),
            },
        };

        // Supersession / refutation: an incoming Supersedes/Refutes edge means
        // something newer overrides or disputes this node. (Edge is from→to where
        // `from` supersedes/refutes `to`, so we want this node's in-edges.)
        let mut superseded_by = Vec::new();
        let mut refuted_by = Vec::new();
        for e in self.store.in_edges(id)? {
            if e.quarantined {
                continue; // a proposal is advisory, never load-bearing
            }
            match e.kind {
                mem_core::EdgeKind::Supersedes => superseded_by.push(e.from),
                mem_core::EdgeKind::Refutes | mem_core::EdgeKind::Contradicts => refuted_by.push(e.from),
                _ => {}
            }
        }

        let contradicted = node.confidence.iter().any(|c| matches!(c, mem_core::BelnapValue::Both));

        Ok(Some(Verification {
            item: RecallItem::from_node(&node, None),
            signature,
            superseded_by,
            refuted_by,
            contradicted,
        }))
    }

    /// Self-critic completeness pass (WP-3.5, D3.5): an agentic verification loop
    /// that reads a recall/search result and reports what an agent acting on it
    /// might be *missing*, so it never treats a partial or stale answer as
    /// complete.
    ///
    /// v1 is a **deterministic** critic — it does not call an LLM. That is a
    /// deliberate plane-invariant choice: an LLM critique is nondeterministic and
    /// would be an *assertion*, not a derivation, so it cannot be a trusted
    /// completeness gate. The deterministic critic instead surfaces the concrete,
    /// checkable gaps that recall's budget-shaping and the two-plane model can
    /// hide: truncated coverage, superseded/contradicted items still in the set,
    /// adjacent quarantined (unconfirmed) edges, and a stale freshness watermark.
    /// An optional LLM elaboration over these gaps is a v2 follow-up (it would
    /// land as a quarantined assertion, never load-bearing).
    pub fn critique(&self, result: &RecallResult, now_ms: Timestamp) -> Result<Critique, StoreError> {
        let mut gaps = Vec::new();

        // 1. Truncated coverage: the budget hid part of the tenant.
        if result.total_in_tenant > result.items.len() {
            gaps.push(Gap::TruncatedCoverage {
                shown: result.items.len(),
                total: result.total_in_tenant,
            });
        }

        // 2. Acting on stale memory: superseded/archived items are in the answer.
        let superseded = result
            .items
            .iter()
            .filter(|i| i.status == Status::Superseded)
            .map(|i| i.id)
            .collect::<Vec<_>>();
        if !superseded.is_empty() {
            gaps.push(Gap::SupersededInResult(superseded));
        }

        // 3. Unresolved contradictions: Belnap `Both` recorded on a returned node.
        for i in &result.items {
            if let Some(n) = self.store.get_node(&i.id)? {
                if n.confidence.iter().any(|c| matches!(c, mem_core::BelnapValue::Both)) {
                    gaps.push(Gap::Contradicted(i.id));
                }
                // 4. Adjacent unconfirmed knowledge: a returned node has a
                //    quarantined (proposed, advisory) edge the answer didn't show.
                let has_quarantined = self
                    .store
                    .out_edges(&i.id)?
                    .into_iter()
                    .chain(self.store.in_edges(&i.id)?)
                    .any(|e| e.quarantined);
                if has_quarantined {
                    gaps.push(Gap::AdjacentProposal(i.id));
                }
            }
        }

        // 5. Stale freshness: the Derived index may be behind the repo.
        let watermark_age_ms = result.watermark.as_ref().map(|w| now_ms.saturating_sub(w.ingested_at_ms));

        let completeness = if gaps.is_empty() { 1.0 } else { 1.0 / (1.0 + gaps.len() as f32) };
        Ok(Critique { gaps, watermark_age_ms, completeness })
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

    /// A Derived node with an explicit retirement instant (`valid_to`).
    fn node_retired(repo: &str, subject: &str, valid_from: Timestamp, valid_to: Timestamp) -> MemoryNode {
        let mut n = node(repo, subject, valid_from);
        n.valid_to = Some(valid_to);
        n
    }

    fn supersedes_edge(from: &ContentHash, to: &ContentHash) -> Edge {
        Edge {
            from: *from,
            to: *to,
            kind: EdgeKind::Supersedes,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "t".into(), at: 0, evidence: None },
            confidence: vec![],
            quarantined: false,
            signature: None,
        }
    }

    // ---- ADR-09 B.4: include_in_flight read path ----

    #[test]
    fn include_in_flight_surfaces_and_labels_branch() {
        let canonical = node("r", "merged work", 100);
        let inflight = node("r", "wip on a branch", 200);
        let mut branch = node("r", "feat/x@deadbeef", 150);
        branch.kind = NodeKind::Branch;
        branch.source_ref = SourceRef::DagNative { key: "branch:feat/x".into() };
        branch.embedding = None;
        let bc = Edge {
            from: branch.compute_id(),
            to: inflight.compute_id(),
            kind: EdgeKind::BranchContains,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "ingest".into(), at: 0, evidence: None },
            confidence: vec![],
            quarantined: false,
            signature: None,
        };
        let s = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        s.commit(&[canonical, inflight, branch], &[bc]).unwrap();

        let r = Recall::new(&s).with_in_flight(true).storyline("r", 10).unwrap();
        let wip = r.items.iter().find(|i| i.title == "wip on a branch").expect("in-flight commit surfaced when opted in");
        assert_eq!(wip.in_flight_branch.as_deref(), Some("feat/x"), "in-flight item labeled with its branch");
        let merged = r.items.iter().find(|i| i.title == "merged work").unwrap();
        assert_eq!(merged.in_flight_branch, None, "canonical item carries no branch label");
        assert!(!r.items.iter().any(|i| i.kind == NodeKind::Branch), "Branch meta-nodes are never items");
    }

    // ---- ADR-09 in-flight branch layer: canonical purity ----

    #[test]
    fn canonical_recall_excludes_in_flight_branch_layer() {
        // A canonical (merged) commit, a Branch meta-node, and an in-flight commit
        // the branch "contains". Canonical recall must show ONLY the merged commit.
        let canonical = node("r", "merged work", 100);
        let inflight = node("r", "wip on a branch", 200);
        let mut branch = node("r", "feat/x@tip", 150);
        branch.kind = NodeKind::Branch;
        branch.source_ref = SourceRef::DagNative { key: "branch:feat/x".into() };
        branch.embedding = None;

        let bc = Edge {
            from: branch.compute_id(),
            to: inflight.compute_id(),
            kind: EdgeKind::BranchContains,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance { method: EdgeMethod::Ingest, asserter: "ingest".into(), at: 0, evidence: None },
            confidence: vec![],
            quarantined: false,
            signature: None,
        };

        let s = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        s.commit(&[canonical, inflight, branch], &[bc]).unwrap();

        let titles: Vec<_> = Recall::new(&s)
            .storyline("r", 10)
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(
            titles,
            vec!["merged work".to_string()],
            "canonical recall must exclude both the Branch node and its in-flight commit"
        );
    }

    #[test]
    fn merged_branch_promotes_from_in_flight_to_canonical() {
        use std::process::Command;
        fn git(args: &[&str]) {
            assert!(Command::new("git").args(args).status().unwrap().success(), "git {args:?}");
        }
        let root = std::env::temp_dir().join(format!("mem-promote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let s = src.to_string_lossy().to_string();
        git(&["-C", &s, "init", "-q", "-b", "main"]);
        git(&["-C", &s, "config", "user.email", "t@t.t"]);
        git(&["-C", &s, "config", "user.name", "t"]);
        git(&["-C", &s, "config", "commit.gpgsign", "false"]);
        std::fs::write(src.join("a.txt"), "1").unwrap();
        git(&["-C", &s, "add", "."]);
        git(&["-C", &s, "commit", "-q", "-m", "base"]);
        git(&["-C", &s, "checkout", "-q", "-b", "feat/x"]);
        std::fs::write(src.join("b.txt"), "2").unwrap();
        git(&["-C", &s, "add", "."]);
        git(&["-C", &s, "commit", "-q", "-m", "feature work xyz"]);
        git(&["-C", &s, "checkout", "-q", "main"]);

        let mirror = root.join("mirror");
        let m = mirror.to_string_lossy().to_string();
        git(&["clone", "-q", &s, &m]);

        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let ing = Ingestor::new("repo");
        ing.ingest(&mirror, &store).unwrap();
        ing.ingest_branches(&mirror, &store).unwrap();

        let titles = |store: &MemoryDagStore<MemoryNode>| -> Vec<String> {
            Recall::new(store).storyline("repo", 20).unwrap().items.into_iter().map(|i| i.title).collect()
        };

        // Pre-merge: the feature work is in-flight, excluded from canonical recall.
        assert!(
            !titles(&store).iter().any(|t| t.contains("feature work")),
            "in-flight work must be excluded from canonical recall before merge"
        );

        // Merge feat/x into main on the source, refresh the mirror (as ensure_mirror does).
        git(&["-C", &s, "merge", "-q", "--no-ff", "feat/x", "-m", "merge feat/x"]);
        git(&["-C", &m, "fetch", "origin", "--prune", "-q"]);
        git(&["-C", &m, "reset", "--hard", "-q", "origin/HEAD"]);

        // Drain: canonical incremental + reap the merged branch.
        ing.ingest_incremental(&mirror, &store).unwrap();
        assert!(ing.reap_branches(&mirror, &store).unwrap().merged >= 1, "feat/x reaped as merged");

        // Post-merge: the same commit is now canonical (promotion, no rewrite).
        assert!(
            titles(&store).iter().any(|t| t.contains("feature work")),
            "merged work must be promoted into canonical recall"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- WP-3.4 as_of / decision-replay ----

    #[test]
    fn as_of_excludes_nodes_not_yet_valid() {
        let s = store_with(&[node("a", "early", 100), node("a", "later", 300)]);
        // At T=200 only the first node existed.
        let r = Recall::new(&s).as_of("a", 200, 10).unwrap();
        let titles: Vec<_> = r.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["early"], "a node valid_from=300 must not appear in an as-of T=200 snapshot");
    }

    #[test]
    fn as_of_excludes_retired_nodes() {
        // valid in [100,200); queried at 250 → already retired.
        let s = store_with(&[node_retired("a", "retired", 100, 200), node("a", "live", 100)]);
        let r = Recall::new(&s).as_of("a", 250, 10).unwrap();
        let titles: Vec<_> = r.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["live"], "a node with valid_to<=T must be excluded from the as-of snapshot");
        // ...but it IS present mid-validity.
        let mid = Recall::new(&s).as_of("a", 150, 10).unwrap();
        assert_eq!(mid.items.len(), 2, "both nodes valid at T=150");
    }

    #[test]
    fn as_of_is_derived_plane_only() {
        let mut asserted = node("a", "an assertion", 100);
        asserted.plane = Plane::Asserted;
        asserted.trust_tier = TrustTier::AgentAsserted;
        let s = store_with(&[node("a", "derived", 100), asserted]);
        let r = Recall::new(&s).as_of("a", 150, 10).unwrap();
        let titles: Vec<_> = r.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["derived"], "decision-replay is over the deterministic Derived projection only");
    }

    /// Opt-in: "what was CLAIMED and still stood at T", a different question from
    /// the default "what did the deterministic record show at T".
    ///
    /// Without this, a signed claim can never be time-filtered at all, so a
    /// consumer of the Asserted plane has no way to ask whether a claim is still
    /// current as of now. `valid_from` on an Asserted node is write-only until
    /// something reads it.
    #[test]
    fn as_of_with_asserted_includes_signed_claims_and_still_honours_retirement() {
        let mut live = node("a", "a claim that still stands", 100);
        live.plane = Plane::Asserted;
        live.trust_tier = TrustTier::AgentAsserted;

        // Superseded at T=120: `apply_supersession` stamps `valid_to` exactly so.
        let mut retired = node("a", "a claim that was corrected", 100);
        retired.plane = Plane::Asserted;
        retired.trust_tier = TrustTier::AgentAsserted;
        retired.valid_to = Some(120);
        retired.status = Status::Superseded;

        let s = store_with(&[node("a", "derived", 100), live, retired]);

        // Default is unchanged: deterministic replay, Derived only.
        let default = Recall::new(&s).as_of("a", 150, 10).unwrap();
        assert_eq!(
            default.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            vec!["derived"],
            "the default must stay a pure deterministic-replay snapshot"
        );

        // Opt in: Derived plus claims still standing at T. The retired one is gone,
        // which is the whole point — this is the staleness gate.
        let widened = Recall::new(&s).with_asserted(true).as_of("a", 150, 10).unwrap();
        let mut titles: Vec<&str> = widened.items.iter().map(|i| i.title.as_str()).collect();
        titles.sort();
        assert_eq!(
            titles,
            vec!["a claim that still stands", "derived"],
            "opt-in adds live claims and still excludes retired ones"
        );

        // Before the claim was made, it is not yet visible either.
        let early = Recall::new(&s).with_asserted(true).as_of("a", 50, 10).unwrap();
        assert!(early.items.is_empty(), "nothing is valid before it began");

        // And at a T before the correction, the retired claim WAS still standing.
        let before_retirement = Recall::new(&s).with_asserted(true).as_of("a", 110, 10).unwrap();
        assert_eq!(before_retirement.items.len(), 3, "history stays walkable");
    }

    // ---- WP-2.4 verify ----

    fn signed_assertion(repo: &str, content: &str) -> MemoryNode {
        use ed25519_dalek::SigningKey;
        let a = mem_assert::Asserter::new(SigningKey::from_bytes(&[7u8; 32]));
        a.assert_node(repo, NodeKind::Rationale, content, 1_000)
    }

    #[test]
    fn verify_derived_node_is_trustworthy_without_a_signature() {
        let s = store_with(&[node("a", "deterministic", 100)]);
        let id = node("a", "deterministic", 100).compute_id();
        let v = Recall::new(&s).verify(&id).unwrap().expect("node exists");
        assert_eq!(v.signature, SignatureStatus::NotApplicableDerived);
        assert!(v.is_trustworthy());
    }

    #[test]
    fn verify_validly_signed_assertion_passes() {
        let n = signed_assertion("a", "tried X, it failed");
        let s = store_with(std::slice::from_ref(&n));
        let v = Recall::new(&s).verify(&n.compute_id()).unwrap().expect("node exists");
        assert_eq!(v.signature, SignatureStatus::Valid);
        assert!(v.is_trustworthy());
    }

    #[test]
    fn verify_tampered_signature_is_rejected() {
        let mut n = signed_assertion("a", "load-bearing claim");
        // Flip a signature byte — the content id still resolves, but the sig breaks.
        if let Some(sig) = n.signature.as_mut() {
            sig[0] ^= 0xff;
        }
        let s = store_with(&[n.clone()]);
        let v = Recall::new(&s).verify(&n.compute_id()).unwrap().expect("node exists");
        assert!(matches!(v.signature, SignatureStatus::Invalid(_)), "a tampered signature must not verify");
        assert!(!v.is_trustworthy());
    }

    #[test]
    fn verify_surfaces_supersession() {
        let old = node("a", "old decision", 100);
        let new = node("a", "new decision", 200);
        let s = store_with(&[old.clone(), new.clone()]);
        s.add_edge(&supersedes_edge(&new.compute_id(), &old.compute_id())).unwrap();
        let v = Recall::new(&s).verify(&old.compute_id()).unwrap().expect("node exists");
        assert_eq!(v.superseded_by, vec![new.compute_id()]);
        assert!(!v.is_trustworthy(), "a superseded node is not safe to act on");
    }

    #[test]
    fn verify_quarantined_supersedes_does_not_count() {
        let old = node("a", "old", 100);
        let new = node("a", "new", 200);
        let s = store_with(&[old.clone(), new.clone()]);
        let mut e = supersedes_edge(&new.compute_id(), &old.compute_id());
        e.quarantined = true; // a proposal — advisory only
        s.add_edge(&e).unwrap();
        let v = Recall::new(&s).verify(&old.compute_id()).unwrap().expect("node exists");
        assert!(v.superseded_by.is_empty(), "a quarantined proposal must not mark the node stale");
        assert!(v.is_trustworthy());
    }

    #[test]
    fn verify_flags_contradiction() {
        let mut n = node("a", "contested", 100);
        n.confidence = vec![mem_core::BelnapValue::Both];
        let s = store_with(&[n.clone()]);
        let v = Recall::new(&s).verify(&n.compute_id()).unwrap().expect("node exists");
        assert!(v.contradicted);
        assert!(!v.is_trustworthy());
    }

    #[test]
    fn verify_unknown_id_is_none() {
        let s = store_with(&[node("a", "x", 1)]);
        let missing = node("a", "not in store", 9).compute_id();
        assert!(Recall::new(&s).verify(&missing).unwrap().is_none());
    }

    // ---- WP-3.5 self-critic ----

    #[test]
    fn critique_flags_truncated_coverage() {
        let s = store_with(&[node("a", "n1", 1), node("a", "n2", 2), node("a", "n3", 3)]);
        let r = Recall::new(&s).storyline("a", 2).unwrap();
        let c = Recall::new(&s).critique(&r, 10).unwrap();
        assert!(c.gaps.iter().any(|g| matches!(g, Gap::TruncatedCoverage { shown: 2, total: 3 })));
        assert!(c.completeness < 1.0);
    }

    #[test]
    fn critique_flags_superseded_in_result() {
        let mut stale = node("a", "stale", 100);
        stale.status = Status::Superseded;
        let s = store_with(&[stale]);
        let r = Recall::new(&s).storyline("a", 10).unwrap();
        let c = Recall::new(&s).critique(&r, 10).unwrap();
        assert!(c.gaps.iter().any(|g| matches!(g, Gap::SupersededInResult(ids) if ids.len() == 1)));
    }

    #[test]
    fn critique_clean_result_has_no_gaps() {
        let s = store_with(&[node("a", "only", 1)]);
        let r = Recall::new(&s).storyline("a", 10).unwrap();
        let c = Recall::new(&s).critique(&r, 10).unwrap();
        assert!(c.gaps.is_empty(), "a complete, current, single-item result has no completeness gaps");
        assert_eq!(c.completeness, 1.0);
    }

    #[test]
    fn critique_flags_adjacent_proposal() {
        let n = node("a", "anchor", 100);
        let other = node("a", "other", 50);
        let s = store_with(&[n.clone(), other.clone()]);
        let mut e = supersedes_edge(&n.compute_id(), &other.compute_id());
        e.quarantined = true; // an unconfirmed proposal adjacent to a returned node
        s.add_edge(&e).unwrap();
        let r = Recall::new(&s).storyline("a", 10).unwrap();
        let c = Recall::new(&s).critique(&r, 10).unwrap();
        assert!(c.gaps.iter().any(|g| matches!(g, Gap::AdjacentProposal(_))));
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

    #[test]
    fn recall_paths_work_over_a_sealed_store_and_forget_after_shred() {
        // ENCRYPT-S1 WP-6: the hot query paths (search / storyline / neighbors)
        // must round-trip unchanged over a store whose nodes AND edges are
        // sealed at rest — and a crypto-shredded tenant must read as forgotten,
        // never as an error.
        let a = node("a", "ghostdag tip selection", 1);
        let b = node("a", "tip selection follow-up", 2);
        let s = MemoryDagStore::new_encrypted(Box::new(InMemoryKv::new()));
        s.commit(
            &[a.clone(), b.clone()],
            &[plain_edge(&b, &a, EdgeKind::Implements, false)],
        )
        .unwrap();

        let r = Recall::new(&s);
        let hits = r.search("a", "ghostdag tip selection", 5).unwrap();
        assert_eq!(hits.items[0].title, "ghostdag tip selection");
        assert_eq!(r.storyline("a", 5).unwrap().items.len(), 2);
        let out = r.neighbors(&b.compute_id(), 10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node.as_ref().map(|n| n.title.clone()), Some("ghostdag tip selection".into()));

        s.shred_tenant("a").unwrap();
        assert!(r.search("a", "ghostdag tip selection", 5).unwrap().items.is_empty());
        assert!(r.storyline("a", 5).unwrap().items.is_empty());
        assert!(r.neighbors(&b.compute_id(), 10).unwrap().is_empty(), "sealed edges die with their tenant");
    }
}
