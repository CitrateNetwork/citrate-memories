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

pub mod docs;
pub mod federation;
pub mod frontmatter;
pub mod git;
pub mod trailers;

use std::collections::{BTreeMap, HashMap};
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

/// Pure core: commits -> (nodes, edges). Deterministic given the same inputs
/// (modulo `now_ms`, which only lands on non-identity fields). Exposed so it can
/// be tested without a git repo.
pub fn build_graph(
    repo: &str,
    recs: &[CommitRecord],
    now_ms: u64,
    embedder: &dyn Embedder,
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

        // Load-bearing trailer edges.
        for t in trailers::parse_trailers(&rec.body) {
            if let Some(ek) = trailers::map_trailer(&t.key) {
                let rid = *ref_ids.entry(t.value.clone()).or_insert_with(|| {
                    let rn = ref_node(repo, &t.value, now_ms);
                    let rid = rn.compute_id();
                    nodes.push(rn);
                    rid
                });
                edges.push(make_edge(id, rid, ek, EdgeMethod::Trailer, now_ms, Some(t.key.clone())));
            }
        }
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
                edges.push(make_edge(id, rid, ek, EdgeMethod::Trailer, now_ms, Some(t.key.clone())));
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
}

impl Ingestor {
    pub fn new(repo_name: impl Into<String>) -> Self {
        Self {
            repo_name: repo_name.into(),
            embedder: Box::new(HashingEmbedder::new(EMBED_DIM)),
        }
    }

    /// Build an ingestor backed by a specific embedder (e.g. a transformer model).
    pub fn with_embedder(repo_name: impl Into<String>, embedder: Box<dyn Embedder>) -> Self {
        Self {
            repo_name: repo_name.into(),
            embedder,
        }
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
        let (mut nodes, mut edges) = build_graph(&self.repo_name, &recs, now, self.embedder.as_ref())?;
        let (doc_nodes, doc_edges, doc_count) =
            build_doc_graph(&self.repo_name, &root, now, self.embedder.as_ref())?;
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
        store.put_meta(&watermark_key(&self.repo_name), &bytes)?;

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

    #[test]
    fn supersedes_trailer_is_applied_with_status_transition() {
        // The full ingest pipeline shape: build the graph, commit the plain
        // edges, apply the supersessions (WP-1.4).
        let body = "details\n\nAgentile-Supersedes: adr:001";
        let recs = vec![rec("aaa", &[], "revisit storage decision", body)];
        let (nodes, edges) = build_graph("r", &recs, 1, &embedder()).unwrap();

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
        let body = "x\n\nAgentile-Supersedes: adr:404";
        let recs = vec![rec("aaa", &[], "subject", body)];
        let (nodes, edges) = build_graph("r", &recs, 1, &embedder()).unwrap();
        let (supersessions, _): (Vec<Edge>, Vec<Edge>) =
            edges.into_iter().partition(|e| e.kind == EdgeKind::Supersedes);
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
        store.put_meta(&watermark_key("r"), &serde_json::to_vec(&wm).unwrap()).unwrap();
        assert_eq!(ing.read_watermark(&store).unwrap(), Some(wm));
    }
}
