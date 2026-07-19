//! `mem-ingest` — Derived-plane ingestion (MEM-S1 / WP-1.2).
//!
//! Turns a repo's git history into memory nodes and edges, deterministically:
//! commits become `Commit` nodes (identity = repo + sha + subject), parent links
//! become `TemporalNext`/`MergeParent` edges, and `Agentile-*` commit trailers
//! become load-bearing relationship edges. A freshness watermark is stored so
//! every later recall can report how far behind HEAD the index is.
//!
//! Everything here is the Derived plane: a pure function of git state, so two
//! machines ingest to byte-identical node ids (the core invariant). Nothing here
//! ever calls a model or writes a non-deterministic edge.

pub mod chain;
pub mod docs;
pub mod federation;
pub mod frontmatter;
pub mod git;
pub mod trailers;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use mem_core::{
    BelnapValue, ContentHash, Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind,
    Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION,
};
use mem_index::{EmbedError, Embedder, HashingEmbedder};
use mem_store::{MemoryDagStore, StoreError, SupersessionError};

use git::CommitRecord;

/// Embedding dimension used for all Derived-plane nodes. Public so the query
/// layer embeds queries in the same space (the index rejects model mismatches).
pub const EMBED_DIM: usize = 256;

/// Freshness watermark is keyed per-repo: one store holds many tenant repos.
fn watermark_key(repo: &str) -> Vec<u8> {
    format!("derived_watermark:{repo}").into_bytes()
}

/// Per-branch watermark key (ADR-09 B.2): last-ingested tip of one non-default
/// branch, so branch ingest is incremental and drift is a cheap HEAD compare.
fn branch_watermark_key(repo: &str, branch: &str) -> Vec<u8> {
    format!("derived_branch_watermark:{repo}:{branch}").into_bytes()
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("git error: {0}")]
    Git(String),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("serialization error: {0}")]
    Serde(String),
    #[error("embedding error: {0}")]
    Embed(String),
    /// Chain-state ingest (E-4): malformed catalog/block data or a failed RPC
    /// round-trip. Always fail-closed — nothing is committed on this error.
    #[error("chain ingest error: {0}")]
    Chain(String),
}

impl From<EmbedError> for IngestError {
    fn from(e: EmbedError) -> Self {
        IngestError::Embed(e.to_string())
    }
}

/// Records how current the Derived index is for a repo. Stamped on every ingest;
/// read back to compute "N commits behind HEAD" on recall.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watermark {
    pub repo: String,
    pub head: Option<String>,
    pub head_count: usize,
    pub ingested_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct IngestReport {
    pub commits: usize,
    pub docs: usize,
    pub nodes_in_store: usize,
    pub edges_in_store: usize,
    /// Supersessions applied this run (Active → Superseded transitions, WP-1.4).
    pub superseded: usize,
    /// Supersedes edges rejected by the guards (cycle/self/missing target) —
    /// deterministic outcome of bad source directives, counted, never fatal.
    pub supersessions_rejected: usize,
    pub watermark: Watermark,
}

/// Outcome of an in-flight branch ingest pass (ADR-09 B.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchIngestReport {
    /// Branches whose tip changed and were (re)ingested this pass.
    pub branches_ingested: usize,
    /// In-flight commit nodes added across those branches.
    pub in_flight_commits: usize,
    /// Branches skipped because their tip matched the stored per-branch watermark.
    pub unchanged: usize,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn commit_node(
    repo: &str,
    rec: &CommitRecord,
    now_ms: u64,
    embedder: &dyn Embedder,
) -> Result<MemoryNode, IngestError> {
    let embed_text = format!("{}\n{}", rec.subject, rec.body);
    Ok(MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind: NodeKind::Commit,
        repo: repo.to_string(),
        author: rec.author.clone(),
        // sha lives in source_ref → identity is stable; subject is the canonical content.
        source_ref: SourceRef::GitCommit {
            repo: repo.to_string(),
            sha: rec.sha.clone(),
        },
        content: rec.subject.as_bytes().to_vec(),
        valid_from: (rec.time_secs.max(0) as u64).saturating_mul(1000),
        valid_to: None,
        observed_at: now_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding: Some(embedder.embed(&embed_text)?),
        confidence: vec![BelnapValue::True], // a commit's existence is a known-true fact
        anchors: vec![],
        status: Status::Active,
    })
}

/// Best-effort kind for an unresolved trailer target (resolution/unification with
/// the real node happens in a later WP).
fn ref_kind(slug: &str) -> NodeKind {
    if slug.to_ascii_lowercase().starts_with("adr") {
        NodeKind::Adr
    } else {
        NodeKind::WorkPackage
    }
}

fn ref_node(repo: &str, slug: &str, now_ms: u64) -> MemoryNode {
    MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind: ref_kind(slug),
        repo: repo.to_string(),
        author: "ingest".to_string(),
        source_ref: SourceRef::DagNative {
            key: format!("ref:{slug}"),
        },
        content: slug.as_bytes().to_vec(),
        valid_from: now_ms,
        valid_to: None,
        observed_at: now_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding: None,
        confidence: vec![],
        anchors: vec![],
        status: Status::Active,
    }
}

fn make_edge(
    from: ContentHash,
    to: ContentHash,
    kind: EdgeKind,
    method: EdgeMethod,
    now_ms: u64,
    evidence: Option<String>,
) -> Edge {
    Edge {
        from,
        to,
        kind,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance {
            method,
            asserter: "ingest".to_string(),
            at: now_ms,
            evidence,
        },
        confidence: vec![BelnapValue::True],
        quarantined: false, // deterministic, load-bearing
        signature: None,
    }
}

/// FUA-MEMORIES-06: edge kinds that RETRACT or CONTRADICT existing memory. A git
/// commit (or doc) trailer is not, by itself, sufficient authority to assert one
/// — that's an authorial act that must be confirmed on the Asserted plane (cf.
/// mem-assert). Structural kinds (Implements/Decides/DependsOn/References/…) are
/// not gated.
fn is_authority_bearing(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Supersedes | EdgeKind::Refutes)
}

/// Build a trailer-derived edge, attributed to the trailer's AUTHOR (not the
/// generic "ingest" identity). Authority-bearing kinds are LOAD-BEARING only when
/// `author` is in `authoritative`; otherwise the edge is quarantined — recorded
/// and attributed, but never auto-applied (e.g. a quarantined `Supersedes` does
/// not transition its target). This is the trailer authorial gate (FUA-MEMORIES-06).
fn make_trailer_edge(
    from: ContentHash,
    to: ContentHash,
    kind: EdgeKind,
    now_ms: u64,
    evidence: Option<String>,
    author: &str,
    authoritative: &HashSet<String>,
) -> Edge {
    let quarantined = is_authority_bearing(kind) && !authoritative.contains(author);
    Edge {
        from,
        to,
        kind,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance {
            method: EdgeMethod::Trailer,
            asserter: author.to_string(),
            at: now_ms,
            evidence,
        },
        confidence: vec![BelnapValue::True],
        quarantined,
        signature: None,
    }
}

/// Pure core: commits -> (nodes, edges). Deterministic given the same inputs
/// (modulo `now_ms`, which only lands on non-identity fields). Exposed so it can
/// be tested without a git repo.
///
/// Fail-closed: no author is treated as authoritative, so authority-bearing
/// trailers (Supersedes/Refutes) are quarantined. Use [`build_graph_gated`] to
/// pass the authoritative-author allowlist (FUA-MEMORIES-06).
pub fn build_graph(
    repo: &str,
    recs: &[CommitRecord],
    now_ms: u64,
    embedder: &dyn Embedder,
) -> Result<(Vec<MemoryNode>, Vec<Edge>), IngestError> {
    build_graph_gated(repo, recs, now_ms, embedder, &HashSet::new())
}

/// As [`build_graph`], but `authoritative` lists the authors whose
/// authority-bearing trailers (Supersedes/Refutes) are load-bearing rather than
/// quarantined (FUA-MEMORIES-06).
pub fn build_graph_gated(
    repo: &str,
    recs: &[CommitRecord],
    now_ms: u64,
    embedder: &dyn Embedder,
    authoritative: &HashSet<String>,
) -> Result<(Vec<MemoryNode>, Vec<Edge>), IngestError> {
    let mut sha_to_id: HashMap<String, ContentHash> = HashMap::new();
    let mut ref_ids: HashMap<String, ContentHash> = HashMap::new();
    let mut nodes: Vec<MemoryNode> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();

    for rec in recs {
        let node = commit_node(repo, rec, now_ms, embedder)?;
        let id = node.compute_id();
        sha_to_id.insert(rec.sha.clone(), id);
        nodes.push(node);

        // Parent links: first parent is the spine (TemporalNext), the rest are merges.
        for (i, parent) in rec.parents.iter().enumerate() {
            if let Some(pid) = sha_to_id.get(parent).copied() {
                let kind = if i == 0 {
                    EdgeKind::TemporalNext
                } else {
                    EdgeKind::MergeParent
                };
                edges.push(make_edge(id, pid, kind, EdgeMethod::Ingest, now_ms, Some(parent.clone())));
            }
        }

        // Trailer edges, attributed to the commit author and authorially gated.
        for t in trailers::parse_trailers(&rec.body) {
            if let Some(ek) = trailers::map_trailer(&t.key) {
                let rid = *ref_ids.entry(t.value.clone()).or_insert_with(|| {
                    let rn = ref_node(repo, &t.value, now_ms);
                    let rid = rn.compute_id();
                    nodes.push(rn);
                    rid
                });
                edges.push(make_trailer_edge(
                    id,
                    rid,
                    ek,
                    now_ms,
                    Some(t.key.clone()),
                    &rec.author,
                    authoritative,
                ));
            }
        }
    }

    Ok((nodes, edges))
}

/// A `Branch` meta-node (ADR-09 B.2): content carries the tip, so a moving branch
/// mints a new node per observed tip (like a chain checkpoint) and re-ingesting the
/// same tip is idempotent. Not embedded (it is a marker, not searchable content).
fn branch_node(repo: &str, name: &str, tip: &str, now_ms: u64) -> MemoryNode {
    MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind: NodeKind::Branch,
        repo: repo.to_string(),
        author: "ingest".to_string(),
        source_ref: SourceRef::DagNative { key: format!("branch:{name}") },
        content: format!("{name}@{tip}").into_bytes(),
        valid_from: now_ms,
        valid_to: None,
        observed_at: now_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding: None,
        confidence: vec![BelnapValue::True],
        anchors: vec![],
        status: Status::Active,
    }
}

/// Build the in-flight subgraph for one branch (ADR-09 B.2): commit nodes for the
/// commits UNIQUE to the branch (`default..branch`), their intra-branch spine/merge
/// edges, a `Branch` meta-node, and a `BranchContains` edge from it to each unique
/// commit. Deliberately does NOT parse trailers: unmerged work must not retract or
/// contradict canonical memory — that authorial act waits for merge.
fn build_branch_graph(
    repo: &str,
    branch_name: &str,
    tip: &str,
    recs: &[CommitRecord],
    now_ms: u64,
    embedder: &dyn Embedder,
) -> Result<(Vec<MemoryNode>, Vec<Edge>), IngestError> {
    let mut sha_to_id: HashMap<String, ContentHash> = HashMap::new();
    let mut nodes: Vec<MemoryNode> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();

    let branch = branch_node(repo, branch_name, tip, now_ms);
    let bid = branch.compute_id();
    nodes.push(branch);

    for rec in recs {
        let node = commit_node(repo, rec, now_ms, embedder)?;
        let id = node.compute_id();
        sha_to_id.insert(rec.sha.clone(), id);
        nodes.push(node);

        // Intra-branch spine (first parent) + merges — only where the parent is
        // itself in this unique set (the base is canonical and already ingested).
        for (i, parent) in rec.parents.iter().enumerate() {
            if let Some(pid) = sha_to_id.get(parent).copied() {
                let kind = if i == 0 { EdgeKind::TemporalNext } else { EdgeKind::MergeParent };
                edges.push(make_edge(id, pid, kind, EdgeMethod::Ingest, now_ms, Some(parent.clone())));
            }
        }

        // The Branch node "contains" every commit unique to it (the in-flight marker
        // canonical recall keys off).
        edges.push(make_edge(bid, id, EdgeKind::BranchContains, EdgeMethod::Ingest, now_ms, Some(branch_name.to_string())));
    }

    Ok((nodes, edges))
}

#[allow(clippy::too_many_arguments)]
fn doc_node(
    repo: &str,
    rel_path: &str,
    title: &str,
    file_len: usize,
    fm: &BTreeMap<String, String>,
    kind: NodeKind,
    body: &str,
    now_ms: u64,
    embedder: &dyn Embedder,
) -> Result<MemoryNode, IngestError> {
    let preview: String = body.chars().take(512).collect();
    let embed_text = format!("{title}\n{preview}");
    // Bitemporal `valid_from` = the doc's authored date (frontmatter `created:`),
    // so it lands in the storyline at its real time rather than clustering at
    // ingest time (finding F-3). Falls back to ingest time when no date is present.
    let valid_from = frontmatter::created_ms(fm).unwrap_or(now_ms);
    Ok(MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind,
        repo: repo.to_string(),
        author: fm.get("author").cloned().unwrap_or_else(|| "ingest".to_string()),
        // path identifies the file; title is the canonical content. No HEAD sha
        // here, so a doc's identity does not churn on unrelated commits.
        source_ref: SourceRef::Artifact {
            repo: repo.to_string(),
            path: rel_path.to_string(),
            git_sha: String::new(),
            byte_start: 0,
            byte_end: file_len as u64,
        },
        content: title.as_bytes().to_vec(),
        valid_from,
        valid_to: None,
        observed_at: now_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding: Some(embedder.embed(&embed_text)?),
        confidence: vec![BelnapValue::True],
        anchors: vec![],
        status: docs::status_from_fm(fm),
    })
}

/// Ingest every tracked markdown doc under `repo_root` into nodes/edges. Returns
/// `(nodes, edges, doc_count)` where `doc_count` excludes minted reference nodes.
fn build_doc_graph(
    repo: &str,
    repo_root: &Path,
    now_ms: u64,
    embedder: &dyn Embedder,
    authoritative: &HashSet<String>,
) -> Result<(Vec<MemoryNode>, Vec<Edge>, usize), IngestError> {
    let paths = docs::list_md_files(repo_root)?;
    let mut nodes: Vec<MemoryNode> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut ref_ids: HashMap<String, ContentHash> = HashMap::new();
    let mut doc_count = 0usize;

    for rel in &paths {
        let bytes = match std::fs::read(repo_root.join(rel)) {
            Ok(b) => b,
            Err(_) => continue, // tracked but unreadable (e.g. deleted) — skip
        };
        let text = String::from_utf8_lossy(&bytes);
        let (fm, body) = frontmatter::parse(&text);
        let kind = docs::classify(rel, &fm, body);
        let title = docs::doc_title(rel, &fm, body);
        // The doc's frontmatter author owns its agentile-block trailers.
        let doc_author = fm.get("author").cloned().unwrap_or_else(|| "ingest".to_string());
        let node = doc_node(repo, rel, &title, bytes.len(), &fm, kind, body, now_ms, embedder)?;
        let id = node.compute_id();
        nodes.push(node);
        doc_count += 1;

        for t in trailers::parse_agentile_blocks(&text) {
            if let Some(ek) = docs::map_block_directive(&t.key) {
                let rid = *ref_ids.entry(t.value.clone()).or_insert_with(|| {
                    let rn = ref_node(repo, &t.value, now_ms);
                    let rid = rn.compute_id();
                    nodes.push(rn);
                    rid
                });
                edges.push(make_trailer_edge(
                    id,
                    rid,
                    ek,
                    now_ms,
                    Some(t.key.clone()),
                    &doc_author,
                    authoritative,
                ));
            }
        }
    }
    Ok((nodes, edges, doc_count))
}

/// Drives Derived-plane ingestion for one repo.
///
/// The embedder is pluggable: [`new`](Ingestor::new) uses the dependency-free
/// [`HashingEmbedder`] baseline, while [`with_embedder`](Ingestor::with_embedder)
/// accepts any [`Embedder`] — e.g. `mem-index`'s transformer embedder (WP-0.4b).
/// Because every vector is model-tagged, the index never mixes spaces, so the
/// choice of embedder is safe to vary per backfill.
pub struct Ingestor {
    repo_name: String,
    embedder: Box<dyn Embedder>,
    /// Authors whose authority-bearing trailers (Supersedes/Refutes) are
    /// load-bearing rather than quarantined (FUA-MEMORIES-06). Empty = fail-closed.
    authoritative_authors: HashSet<String>,
}

impl Ingestor {
    pub fn new(repo_name: impl Into<String>) -> Self {
        Self {
            repo_name: repo_name.into(),
            embedder: Box::new(HashingEmbedder::new(EMBED_DIM)),
            authoritative_authors: HashSet::new(),
        }
    }

    /// Build an ingestor backed by a specific embedder (e.g. a transformer model).
    pub fn with_embedder(repo_name: impl Into<String>, embedder: Box<dyn Embedder>) -> Self {
        Self {
            repo_name: repo_name.into(),
            embedder,
            authoritative_authors: HashSet::new(),
        }
    }

    /// Set the authors whose authority-bearing trailers (Supersedes/Refutes) are
    /// honored as load-bearing. Anyone else's are quarantined (FUA-MEMORIES-06).
    pub fn with_authoritative_authors(
        mut self,
        authors: impl IntoIterator<Item = String>,
    ) -> Self {
        self.authoritative_authors = authors.into_iter().collect();
        self
    }

    /// Ingest the full git history at `repo_path` into `store`. Idempotent:
    /// content-addressed nodes/edges dedupe, so re-running is a no-op on counts.
    pub fn ingest(
        &self,
        repo_path: &Path,
        store: &MemoryDagStore<MemoryNode>,
    ) -> Result<IngestReport, IngestError> {
        let root = git::repo_root(repo_path)?;
        let now = now_millis();

        // Derived plane = commits + markdown docs, committed atomically.
        let recs = git::read_commits(&root)?;
        let (mut nodes, mut edges) = build_graph_gated(
            &self.repo_name,
            &recs,
            now,
            self.embedder.as_ref(),
            &self.authoritative_authors,
        )?;
        let (doc_nodes, doc_edges, doc_count) = build_doc_graph(
            &self.repo_name,
            &root,
            now,
            self.embedder.as_ref(),
            &self.authoritative_authors,
        )?;
        nodes.extend(doc_nodes);
        edges.extend(doc_edges);

        // Supersedes edges are *applied* (status transition, cycle-guarded), not
        // just stored (WP-1.4). Commit everything else first so targets exist.
        let (supersessions, plain): (Vec<Edge>, Vec<Edge>) =
            edges.into_iter().partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);
        store.commit(&nodes, &plain)?;
        let (superseded, supersessions_rejected) = apply_supersessions(store, &supersessions)?;

        let watermark = Watermark {
            repo: self.repo_name.clone(),
            head: recs.last().map(|r| r.sha.clone()),
            head_count: recs.len(),
            ingested_at_ms: now,
        };
        let bytes = serde_json::to_vec(&watermark).map_err(|e| IngestError::Serde(e.to_string()))?;
        store.put_meta(&self.repo_name, &watermark_key(&self.repo_name), &bytes)?;

        Ok(IngestReport {
            commits: recs.len(),
            docs: doc_count,
            nodes_in_store: store.node_count()?,
            edges_in_store: store.edge_count()?,
            superseded,
            supersessions_rejected,
            watermark,
        })
    }

    /// Read back the freshness watermark (None if never ingested).
    pub fn read_watermark(
        &self,
        store: &MemoryDagStore<MemoryNode>,
    ) -> Result<Option<Watermark>, IngestError> {
        match store.get_meta(&watermark_key(&self.repo_name))? {
            None => Ok(None),
            Some(bytes) => {
                let wm = serde_json::from_slice(&bytes).map_err(|e| IngestError::Serde(e.to_string()))?;
                Ok(Some(wm))
            }
        }
    }

    /// Incremental ingest (MEM-S7 WP-7.1): embed only the commits added since the
    /// stored watermark, then refresh the docs. This is the per-push update path —
    /// O(new commits) instead of O(full history) — so the change-feed can keep the
    /// graph current without re-embedding everything on each push.
    ///
    /// Falls back to a full {@link ingest} when there is no prior watermark, when
    /// the previous head is no longer reachable from HEAD (force-push / history
    /// rewrite), or when the watermark has no head. Idempotent: with HEAD already
    /// at the watermark, no new commit nodes are produced.
    ///
    /// Docs are always rebuilt against the current tree (they are few and
    /// content-addressed, so re-running dedupes); commit embedding — the expensive
    /// part — is what we make incremental.
    pub fn ingest_incremental(
        &self,
        repo_path: &Path,
        store: &MemoryDagStore<MemoryNode>,
    ) -> Result<IngestReport, IngestError> {
        let root = git::repo_root(repo_path)?;

        // Decide range: only when the prior head is still an ancestor of HEAD.
        let prior = self.read_watermark(store)?;
        let since: Option<String> = match prior.as_ref().and_then(|w| w.head.clone()) {
            Some(head) if git::is_ancestor(&root, &head)? => Some(head),
            // No watermark, no head, or rewritten history → re-derive fully.
            _ => None,
        };
        if since.is_none() {
            return self.ingest(repo_path, store);
        }
        let since = since.expect("checked is_some");

        let now = now_millis();
        let recs = git::read_commits_range(&root, Some(&since))?;

        // Commit nodes/edges for ONLY the new commits.
        let (mut nodes, mut edges) = build_graph_gated(
            &self.repo_name,
            &recs,
            now,
            self.embedder.as_ref(),
            &self.authoritative_authors,
        )?;
        // Refresh docs against the current tree (idempotent on unchanged docs).
        let (doc_nodes, doc_edges, doc_count) = build_doc_graph(
            &self.repo_name,
            &root,
            now,
            self.embedder.as_ref(),
            &self.authoritative_authors,
        )?;
        nodes.extend(doc_nodes);
        edges.extend(doc_edges);

        let (supersessions, plain): (Vec<Edge>, Vec<Edge>) =
            edges.into_iter().partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);
        store.commit(&nodes, &plain)?;
        let (superseded, supersessions_rejected) = apply_supersessions(store, &supersessions)?;

        // Stamp the watermark from git directly so head/head_count stay exact
        // regardless of merges (additive counting would drift).
        let watermark = Watermark {
            repo: self.repo_name.clone(),
            head: Some(git::head_sha(&root)?),
            head_count: git::commit_count(&root)?,
            ingested_at_ms: now,
        };
        let bytes = serde_json::to_vec(&watermark).map_err(|e| IngestError::Serde(e.to_string()))?;
        store.put_meta(&self.repo_name, &watermark_key(&self.repo_name), &bytes)?;

        Ok(IngestReport {
            commits: recs.len(),
            docs: doc_count,
            nodes_in_store: store.node_count()?,
            edges_in_store: store.edge_count()?,
            superseded,
            supersessions_rejected,
            watermark,
        })
    }

    /// Ingest the in-flight branch layer (ADR-09 B.2): for every non-default branch,
    /// emit its unique commits (`default..branch`) plus a `Branch` node and
    /// `BranchContains` edges, tracked by a per-branch watermark so it is incremental
    /// and idempotent. Canonical (default-branch) recall excludes these until they
    /// merge; `include_in_flight` (B.4) surfaces them, clearly labeled.
    ///
    /// Requires a mirror with remote-tracking refs fetched (`refs/remotes/origin/*`),
    /// which the ingest worker's `ensure_mirror` provides.
    pub fn ingest_branches(
        &self,
        repo_path: &Path,
        store: &MemoryDagStore<MemoryNode>,
    ) -> Result<BranchIngestReport, IngestError> {
        let root = git::repo_root(repo_path)?;
        let default_ref = git::default_branch_ref(&root)?;
        let branches = git::list_branches(&root, &default_ref)?;
        let now = now_millis();
        let mut report = BranchIngestReport::default();

        for br in branches {
            // Idempotent skip: tip unchanged since the last branch ingest.
            let wkey = branch_watermark_key(&self.repo_name, &br.name);
            if let Some(bytes) = store.get_meta(&wkey)? {
                if let Ok(wm) = serde_json::from_slice::<Watermark>(&bytes) {
                    if wm.head.as_deref() == Some(br.tip.as_str()) {
                        report.unchanged += 1;
                        continue;
                    }
                }
            }

            let recs = git::read_commits_between(&root, &default_ref, &br.tip)?;
            let (nodes, edges) = build_branch_graph(
                &self.repo_name,
                &br.name,
                &br.tip,
                &recs,
                now,
                self.embedder.as_ref(),
            )?;
            // No trailers in the branch subgraph → no Supersedes edges → a plain commit.
            store.commit(&nodes, &edges)?;

            let wm = Watermark {
                repo: self.repo_name.clone(),
                head: Some(br.tip.clone()),
                head_count: recs.len(),
                ingested_at_ms: now,
            };
            let bytes = serde_json::to_vec(&wm).map_err(|e| IngestError::Serde(e.to_string()))?;
            store.put_meta(&self.repo_name, &wkey, &bytes)?;

            report.branches_ingested += 1;
            report.in_flight_commits += recs.len();
        }
        Ok(report)
    }
}

/// Apply a batch of ingest-derived supersedes edges (targets must already be
/// committed). Returns `(applied, rejected)`: a backend failure is fatal, but a
/// semantically bad directive (cycle/self-supersede/missing target) is a
/// deterministic property of the source — counted, never fatal, so one bad
/// trailer can't take down a repo's ingest.
fn apply_supersessions(
    store: &MemoryDagStore<MemoryNode>,
    edges: &[Edge],
) -> Result<(usize, usize), IngestError> {
    let (mut applied, mut rejected) = (0, 0);
    for e in edges {
        match store.apply_supersession(e) {
            Ok(r) => applied += usize::from(r.transitioned),
            Err(SupersessionError::Store(e)) => return Err(IngestError::Store(e)),
            Err(_) => rejected += 1,
        }
    }
    Ok((applied, rejected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_store::kv::InMemoryKv;

    fn rec(sha: &str, parents: &[&str], subject: &str, body: &str) -> CommitRecord {
        CommitRecord {
            sha: sha.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: "tester".into(),
            time_secs: 1_000,
            subject: subject.into(),
            body: body.into(),
        }
    }

    fn embedder() -> HashingEmbedder {
        HashingEmbedder::new(EMBED_DIM)
    }

    #[test]
    fn build_branch_graph_marks_in_flight_and_ignores_trailers() {
        // Two commits unique to a branch; the second carries a Supersedes trailer
        // that MUST NOT become an edge (unmerged work can't retract canonical memory).
        let recs = vec![
            rec("f1", &["base"], "wip one", ""),
            rec("f2", &["f1"], "wip two", "Supersedes: ADR-01"),
        ];
        let (nodes, edges) = build_branch_graph("r", "feat/x", "f2", &recs, 1, &embedder()).unwrap();

        assert_eq!(nodes.iter().filter(|n| n.kind == NodeKind::Branch).count(), 1, "one branch node");
        assert_eq!(nodes.iter().filter(|n| n.kind == NodeKind::Commit).count(), 2, "two in-flight commits");
        assert_eq!(
            edges.iter().filter(|e| e.kind == EdgeKind::BranchContains).count(),
            2,
            "a BranchContains edge per unique commit"
        );
        assert!(!edges.iter().any(|e| e.kind == EdgeKind::Supersedes), "no trailer-derived edges from in-flight work");
        // f2's parent f1 is in the set → one intra-branch spine edge; f1's parent
        // `base` is canonical (not in set) → no edge.
        assert_eq!(edges.iter().filter(|e| e.kind == EdgeKind::TemporalNext).count(), 1);
    }

    #[test]
    fn ingest_branches_end_to_end_and_idempotent() {
        use std::process::Command;
        fn git(args: &[&str]) {
            assert!(Command::new("git").args(args).status().unwrap().success(), "git {args:?}");
        }
        let root = std::env::temp_dir().join(format!("mem-branch-it-{}", std::process::id()));
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
        git(&["-C", &s, "commit", "-q", "-m", "base on main"]);
        // A feature branch with one extra commit.
        git(&["-C", &s, "checkout", "-q", "-b", "feat/x"]);
        std::fs::write(src.join("b.txt"), "2").unwrap();
        git(&["-C", &s, "add", "."]);
        git(&["-C", &s, "commit", "-q", "-m", "wip on feat/x"]);
        git(&["-C", &s, "checkout", "-q", "main"]);

        // Clone → mirror with refs/remotes/origin/* and origin/HEAD (like ensure_mirror).
        let mirror = root.join("mirror");
        git(&["clone", "-q", &s, &mirror.to_string_lossy()]);

        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let ing = Ingestor::new("repo");
        ing.ingest(&mirror, &store).expect("canonical ingest");

        let rep = ing.ingest_branches(&mirror, &store).expect("branch ingest");
        assert_eq!(rep.branches_ingested, 1, "feat/x ingested");
        assert_eq!(rep.in_flight_commits, 1, "one commit unique to feat/x");

        // Idempotent: same tip → unchanged, nothing new.
        let rep2 = ing.ingest_branches(&mirror, &store).expect("re-ingest");
        assert_eq!(rep2.branches_ingested, 0);
        assert_eq!(rep2.unchanged, 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn linear_history_builds_temporal_spine() {
        let recs = vec![
            rec("aaa", &[], "first", ""),
            rec("bbb", &["aaa"], "second", ""),
            rec("ccc", &["bbb"], "third", ""),
        ];
        let (nodes, edges) = build_graph("r", &recs, 1, &embedder()).unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|e| e.kind == EdgeKind::TemporalNext));
    }

    #[test]
    fn merge_commit_yields_one_spine_and_one_merge_edge() {
        let recs = vec![
            rec("aaa", &[], "root", ""),
            rec("bbb", &["aaa"], "branch", ""),
            rec("ccc", &["aaa", "bbb"], "merge", ""), // two parents
        ];
        let (_, edges) = build_graph("r", &recs, 1, &embedder()).unwrap();
        let temporal = edges.iter().filter(|e| e.kind == EdgeKind::TemporalNext).count();
        let merge = edges.iter().filter(|e| e.kind == EdgeKind::MergeParent).count();
        // bbb->aaa and ccc->aaa (first parents); aaa is a root with no parent.
        assert_eq!(temporal, 2, "each non-root child links to its first parent");
        assert_eq!(merge, 1, "the second parent of the merge is a MergeParent edge");
    }

    #[test]
    fn trailer_becomes_load_bearing_edge() {
        let body = "details\n\nAgentile-Implements: SELL-S2#step-3";
        let recs = vec![rec("aaa", &[], "do step 3", body)];
        let (nodes, edges) = build_graph("r", &recs, 1, &embedder()).unwrap();
        // commit node + one ref node
        assert_eq!(nodes.len(), 2);
        let impl_edges: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Implements).collect();
        assert_eq!(impl_edges.len(), 1);
        assert!(!impl_edges[0].quarantined, "trailer edges are load-bearing");
        assert_eq!(impl_edges[0].trust_tier, TrustTier::DerivedDeterministic);
    }

    fn authoritative() -> HashSet<String> {
        // `rec` authors its commits as "tester".
        HashSet::from(["tester".to_string()])
    }

    #[test]
    fn supersedes_trailer_from_unauthoritative_author_is_quarantined() {
        // FUA-MEMORIES-06: a Supersedes trailer from an author NOT on the
        // authoritative list is recorded + attributed but quarantined, so it is
        // never auto-applied (the ingest pipeline only applies !quarantined).
        let body = "details\n\nAgentile-Supersedes: adr:001";
        let recs = vec![rec("aaa", &[], "revisit storage decision", body)];
        // build_graph = fail-closed (empty allowlist) → "tester" is not authoritative.
        let (_, edges) = build_graph("r", &recs, 1, &embedder()).unwrap();
        let sup: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Supersedes).collect();
        assert_eq!(sup.len(), 1, "the edge is still recorded");
        assert!(sup[0].quarantined, "an unauthoritative supersedes is quarantined");
        assert_eq!(sup[0].provenance.asserter, "tester", "attributed to the real author");
        // The pipeline's application filter excludes it.
        assert_eq!(
            edges.iter().filter(|e| e.kind == EdgeKind::Supersedes && !e.quarantined).count(),
            0,
        );
    }

    #[test]
    fn supersedes_trailer_is_applied_with_status_transition() {
        // The full ingest pipeline shape: build the graph, commit the plain
        // edges, apply the supersessions (WP-1.4). The author is authoritative,
        // so the trailer is load-bearing (FUA-MEMORIES-06).
        let body = "details\n\nAgentile-Supersedes: adr:001";
        let recs = vec![rec("aaa", &[], "revisit storage decision", body)];
        let (nodes, edges) =
            build_graph_gated("r", &recs, 1, &embedder(), &authoritative()).unwrap();

        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let (supersessions, plain): (Vec<Edge>, Vec<Edge>) =
            edges.into_iter().partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);
        assert_eq!(supersessions.len(), 1, "trailer minted a supersedes edge");
        store.commit(&nodes, &plain).unwrap();

        let (applied, rejected) = apply_supersessions(&store, &supersessions).unwrap();
        assert_eq!((applied, rejected), (1, 0));
        let target = store.get_node(&supersessions[0].to).unwrap().unwrap();
        assert_eq!(target.status, Status::Superseded, "ref-node target transitioned");
        assert_eq!(target.valid_to, Some(supersessions[0].provenance.at));

        // Re-ingest of the same history: idempotent, no second transition.
        let (applied, rejected) = apply_supersessions(&store, &supersessions).unwrap();
        assert_eq!((applied, rejected), (0, 0));
    }

    #[test]
    fn bad_supersession_is_counted_not_fatal() {
        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        // A supersedes edge whose target was never committed (dangling directive).
        // Author is authoritative so the edge is load-bearing (FUA-MEMORIES-06).
        let body = "x\n\nAgentile-Supersedes: adr:404";
        let recs = vec![rec("aaa", &[], "subject", body)];
        let (nodes, edges) =
            build_graph_gated("r", &recs, 1, &embedder(), &authoritative()).unwrap();
        let (supersessions, _): (Vec<Edge>, Vec<Edge>) =
            edges.into_iter().partition(|e| e.kind == EdgeKind::Supersedes && !e.quarantined);
        // Commit only the commit/ref nodes minus the target: simulate by NOT
        // committing anything — both endpoints missing.
        let (applied, rejected) = apply_supersessions(&store, &supersessions).unwrap();
        assert_eq!((applied, rejected), (0, 1), "dangling directive counted, ingest survives");
        let _ = nodes;
    }

    #[test]
    fn build_graph_is_deterministic() {
        let recs = vec![rec("aaa", &[], "first", "body"), rec("bbb", &["aaa"], "second", "")];
        let (n1, _) = build_graph("r", &recs, 1, &embedder()).unwrap();
        let (n2, _) = build_graph("r", &recs, 999, &embedder()).unwrap(); // different now_ms
        let ids1: Vec<_> = n1.iter().map(|n| n.compute_id()).collect();
        let ids2: Vec<_> = n2.iter().map(|n| n.compute_id()).collect();
        assert_eq!(ids1, ids2, "node ids do not depend on observed_at");
    }

    #[test]
    fn ingest_is_idempotent_on_counts() {
        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let recs = vec![rec("aaa", &[], "first", ""), rec("bbb", &["aaa"], "second", "")];
        let now = 42;
        let (nodes, edges) = build_graph("r", &recs, now, &embedder()).unwrap();
        store.commit(&nodes, &edges).unwrap();
        let n1 = store.node_count().unwrap();
        let e1 = store.edge_count().unwrap();
        // commit again
        store.commit(&nodes, &edges).unwrap();
        assert_eq!(store.node_count().unwrap(), n1, "re-commit must not grow node count");
        assert_eq!(store.edge_count().unwrap(), e1, "re-commit must not grow edge count");
    }

    #[test]
    fn watermark_roundtrips() {
        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let ing = Ingestor::new("r");
        assert!(ing.read_watermark(&store).unwrap().is_none());
        // simulate an ingest's watermark write via the store directly
        let wm = Watermark { repo: "r".into(), head: Some("aaa".into()), head_count: 1, ingested_at_ms: 7 };
        store.put_meta("r", &watermark_key("r"), &serde_json::to_vec(&wm).unwrap()).unwrap();
        assert_eq!(ing.read_watermark(&store).unwrap(), Some(wm));
    }

    // ── MEM-S7 WP-7.1: incremental ingest over a real temp git repo ──────────
    use std::process::Command as Cmd;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp_repo() -> std::path::PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mem-s7-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let ok = Cmd::new("git").arg("-C").arg(&dir).args(args).status().unwrap().success();
            assert!(ok, "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t.t"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "commit.gpgsign", "false"]);
        dir
    }

    fn commit(dir: &std::path::Path, file: &str, contents: &str, msg: &str) {
        std::fs::write(dir.join(file), contents).unwrap();
        let git = |args: &[&str]| {
            assert!(Cmd::new("git").arg("-C").arg(dir).args(args).status().unwrap().success());
        };
        git(&["add", "."]);
        git(&["commit", "-q", "-m", msg]);
    }

    #[test]
    fn ingest_incremental_only_embeds_new_commits() {
        let dir = tmp_repo();
        commit(&dir, "a.txt", "1", "first");
        commit(&dir, "a.txt", "2", "second");

        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let ing = Ingestor::new("r");

        // No prior watermark → falls back to a full ingest (both commits).
        let r1 = ing.ingest_incremental(&dir, &store).unwrap();
        assert_eq!(r1.commits, 2, "first run ingests full history");
        let n1 = store.node_count().unwrap();
        assert_eq!(ing.read_watermark(&store).unwrap().unwrap().head_count, 2);

        // Re-run with no new commits → idempotent: 0 commits, no node growth.
        let r2 = ing.ingest_incremental(&dir, &store).unwrap();
        assert_eq!(r2.commits, 0, "no new commits to ingest");
        assert_eq!(store.node_count().unwrap(), n1, "idempotent: store does not grow");

        // One new commit → only that commit is embedded.
        commit(&dir, "a.txt", "3", "third");
        let r3 = ing.ingest_incremental(&dir, &store).unwrap();
        assert_eq!(r3.commits, 1, "only the single new commit is ingested");
        assert_eq!(store.node_count().unwrap(), n1 + 1, "exactly one new commit node");
        assert_eq!(ing.read_watermark(&store).unwrap().unwrap().head_count, 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ingest_incremental_full_rederive_on_history_rewrite() {
        let dir = tmp_repo();
        commit(&dir, "a.txt", "1", "first");
        commit(&dir, "a.txt", "2", "second");
        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let ing = Ingestor::new("r");
        ing.ingest_incremental(&dir, &store).unwrap();

        // Rewrite history so the watermark head is no longer reachable.
        assert!(Cmd::new("git").arg("-C").arg(&dir)
            .args(["reset", "--hard", "HEAD~1"]).status().unwrap().success());
        commit(&dir, "a.txt", "2-rewritten", "second-prime");

        // The prior head is not an ancestor → full re-derive, no panic.
        let r = ing.ingest_incremental(&dir, &store).unwrap();
        assert_eq!(r.commits, 2, "rewrite triggers a full re-derive");
        assert_eq!(ing.read_watermark(&store).unwrap().unwrap().head_count, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
