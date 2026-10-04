//! MEM-S7 auto-ingest throughput: re-ingesting already-known commits/docs reuses
//! the stored vectors instead of re-embedding, with byte-identical nodes.
//!
//! Before this, every reconcile drain re-embedded every tracked doc of the repo and
//! every `default..branch` commit of each advanced in-flight branch — on the live
//! federation store that was minutes of bge CPU per drain, which serialized behind
//! it the canonical ingest of every other queued repo (the freshness lag).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mem_core::{MemoryNode, NodeKind, VersionedVector};
use mem_index::{EmbedError, Embedder, HashingEmbedder};
use mem_ingest::Ingestor;
use mem_store::kv::InMemoryKv;
use mem_store::MemoryDagStore;

/// Counts embed calls; delegates to a real (hashing) embedder so vectors are real.
struct Counting {
    inner: HashingEmbedder,
    calls: Arc<AtomicUsize>,
}
impl Embedder for Counting {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn embed(&self, text: &str) -> Result<VersionedVector, EmbedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.embed(text)
    }
}

fn counting(dim: usize) -> (Box<dyn Embedder>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (Box::new(Counting { inner: HashingEmbedder::new(dim), calls: calls.clone() }), calls)
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(dir).args(args).status().expect("spawn git");
    assert!(st.success(), "git {args:?}");
}

fn commit_file(dir: &Path, name: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(name), body).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn scratch(tag: &str) -> std::path::PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mem-reuse-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@t.t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn sorted_nodes(store: &MemoryDagStore<MemoryNode>) -> Vec<(String, Option<VersionedVector>)> {
    let mut v: Vec<_> = store
        .all_nodes()
        .unwrap()
        .into_iter()
        .map(|n| (n.compute_id().to_hex(), n.embedding))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

#[test]
fn full_reingest_reuses_every_vector_and_is_identical() {
    let root = scratch("full");
    let repo = root.join("r");
    init_repo(&repo);
    commit_file(&repo, "README.md", "# Title\n\nbody one\n", "first commit");
    commit_file(&repo, "docs.md", "---\nauthor: a\n---\n# Doc\n\ntext\n", "second commit");
    let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));

    let (e1, c1) = counting(64);
    Ingestor::with_embedder("r", e1).ingest(&repo, &store).expect("ingest 1");
    let first_calls = c1.load(Ordering::SeqCst);
    assert_eq!(first_calls, 4, "2 commits + 2 docs embedded on first ingest");
    let before = sorted_nodes(&store);

    // A full re-derive (what a force-push triggers) over unchanged history.
    let (e2, c2) = counting(64);
    Ingestor::with_embedder("r", e2).ingest(&repo, &store).expect("ingest 2");
    assert_eq!(c2.load(Ordering::SeqCst), 0, "every vector reused, zero embed calls");
    assert_eq!(sorted_nodes(&store), before, "same ids, same vectors");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn incremental_only_embeds_new_commit_and_changed_doc() {
    let root = scratch("incr");
    let repo = root.join("r");
    init_repo(&repo);
    commit_file(&repo, "a.md", "# A\n\none\n", "c1");
    commit_file(&repo, "b.md", "# B\n\ntwo\n", "c2");
    let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
    let (e1, _) = counting(64);
    Ingestor::with_embedder("r", e1).ingest(&repo, &store).expect("ingest");

    // One new commit that edits one doc: expect exactly 2 embeds (the commit + the
    // edited doc's new blob). The untouched doc's vector is reused.
    commit_file(&repo, "a.md", "# A\n\none, edited\n", "c3");
    let (e2, c2) = counting(64);
    let rep = Ingestor::with_embedder("r", e2).ingest_incremental(&repo, &store).expect("incr");
    assert_eq!(rep.commits, 1);
    assert_eq!(c2.load(Ordering::SeqCst), 2, "new commit + changed doc only");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn model_mismatch_never_reuses() {
    let root = scratch("model");
    let repo = root.join("r");
    init_repo(&repo);
    commit_file(&repo, "a.md", "# A\n\none\n", "c1");
    let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
    let (e1, _) = counting(64);
    Ingestor::with_embedder("r", e1).ingest(&repo, &store).expect("ingest");

    // A different model (dim 32 → different model id): stored 64-d vectors must
    // not be reused across spaces; everything is re-embedded in the new space.
    let (e2, c2) = counting(32);
    Ingestor::with_embedder("r", e2).ingest(&repo, &store).expect("ingest other model");
    assert_eq!(c2.load(Ordering::SeqCst), 2, "1 commit + 1 doc re-embedded");
    let all = store.all_nodes().unwrap();
    assert!(all
        .iter()
        .filter(|n| n.kind != NodeKind::Branch)
        .filter_map(|n| n.embedding.as_ref())
        .all(|v| v.model == "hashing-v1-d32"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn advanced_branch_embeds_only_its_new_commit() {
    let root = scratch("branch");
    let src = root.join("src");
    init_repo(&src);
    commit_file(&src, "a.txt", "1", "base on main");
    git(&src, &["checkout", "-q", "-b", "feat/x"]);
    commit_file(&src, "b.txt", "2", "wip 1");
    commit_file(&src, "c.txt", "3", "wip 2");
    git(&src, &["checkout", "-q", "main"]);

    let mirror = root.join("mirror");
    let st = Command::new("git").args(["clone", "-q"]).arg(&src).arg(&mirror).status().unwrap();
    assert!(st.success());
    let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
    let (e1, c1) = counting(64);
    let ing = Ingestor::with_embedder("r", e1);
    ing.ingest(&mirror, &store).expect("canonical");
    let after_canonical = c1.load(Ordering::SeqCst);
    let r1 = ing.ingest_branches(&mirror, &store).expect("branches");
    assert_eq!(r1.in_flight_commits, 2);
    assert_eq!(c1.load(Ordering::SeqCst) - after_canonical, 2, "two new in-flight commits embedded");

    // The branch advances by one commit: the pass re-walks main..tip (3 commits)
    // but only the one new commit is embedded.
    git(&src, &["checkout", "-q", "feat/x"]);
    commit_file(&src, "d.txt", "4", "wip 3");
    git(&src, &["checkout", "-q", "main"]);
    git(&mirror, &["fetch", "-q", "origin"]);
    let before = c1.load(Ordering::SeqCst);
    let r2 = ing.ingest_branches(&mirror, &store).expect("branches advanced");
    assert_eq!(r2.branches_ingested, 1);
    assert_eq!(r2.in_flight_commits, 3);
    assert_eq!(c1.load(Ordering::SeqCst) - before, 1, "only the new commit is embedded");

    let _ = std::fs::remove_dir_all(&root);
}
