//! ADR-09 B.5 — adversarial / idempotency proof for the in-flight branch layer.
//!
//! Real-git integration tests that hammer the invariants together:
//!   - idempotency: repeated triggers add nothing (content-addressed + idempotent reap)
//!   - convergence: any trigger order yields the same graph for the same git state
//!   - canonical purity under force-push: an orphaned in-flight commit never leaks
//!     into canonical recall (reachability-defined, so branch-node state can't break it)
//!   - merge promotes and delete archives, with canonical purity preserved throughout
//!
//! Public-API only (Ingestor / Recall / MemoryDagStore).

use std::path::Path;
use std::process::Command;

use mem_core::MemoryNode;
use mem_ingest::Ingestor;
use mem_query::Recall;
use mem_store::kv::InMemoryKv;
use mem_store::MemoryDagStore;

fn sh(dir: &str, args: &[&str]) {
    let mut a = vec!["-C", dir];
    a.extend_from_slice(args);
    assert!(Command::new("git").args(&a).status().unwrap().success(), "git -C {dir} {args:?}");
}

fn init_repo(dir: &str) {
    std::fs::create_dir_all(dir).unwrap();
    sh(dir, &["init", "-q", "-b", "main"]);
    sh(dir, &["config", "user.email", "t@t.t"]);
    sh(dir, &["config", "user.name", "t"]);
    sh(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit(dir: &str, file: &str, content: &str, msg: &str) {
    std::fs::write(Path::new(dir).join(file), content).unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", msg]);
}

/// Fetch + hard-reset the mirror to the default tip, exactly like the ingest worker's
/// `ensure_mirror` does before an ingest.
fn refresh(mirror: &Path) {
    let m = mirror.to_string_lossy().to_string();
    sh(&m, &["fetch", "origin", "--prune", "-q"]);
    sh(&m, &["reset", "--hard", "-q", "origin/HEAD"]);
}

fn clone(src: &str, dst: &str) {
    assert!(Command::new("git").args(["clone", "-q", src, dst]).status().unwrap().success());
}

/// One full drain cycle, matching the gateway worker: canonical incremental ingest,
/// then in-flight branch ingest, then reap.
fn drain(ing: &Ingestor, mirror: &Path, store: &MemoryDagStore<MemoryNode>) {
    ing.ingest_incremental(mirror, store).unwrap();
    ing.ingest_branches(mirror, store).unwrap();
    ing.reap_branches(mirror, store).unwrap();
}

fn store() -> MemoryDagStore<MemoryNode> {
    MemoryDagStore::new(Box::new(InMemoryKv::new()))
}

fn titles(store: &MemoryDagStore<MemoryNode>, repo: &str, in_flight: bool) -> Vec<String> {
    Recall::new(store)
        .with_in_flight(in_flight)
        .storyline(repo, 50)
        .unwrap()
        .items
        .into_iter()
        .map(|i| i.title)
        .collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("mem-b5-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

// --------------------------------------------------------------------------

#[test]
fn repeated_drains_are_idempotent() {
    let root = scratch("idem");
    let src = root.join("src");
    let s = src.to_string_lossy().to_string();
    init_repo(&s);
    commit(&s, "a.txt", "1", "base");
    sh(&s, &["checkout", "-q", "-b", "feat/x"]);
    commit(&s, "b.txt", "2", "wip x");
    sh(&s, &["checkout", "-q", "main"]);
    let mirror = root.join("mirror");
    clone(&s, &mirror.to_string_lossy());

    let st = store();
    let ing = Ingestor::new("repo");
    drain(&ing, &mirror, &st);
    let (n1, e1) = (st.node_count().unwrap(), st.edge_count().unwrap());
    // Two more drains against the same git state must add nothing.
    drain(&ing, &mirror, &st);
    drain(&ing, &mirror, &st);
    assert_eq!((st.node_count().unwrap(), st.edge_count().unwrap()), (n1, e1), "re-drain must be a no-op on counts");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn convergent_regardless_of_trigger_order() {
    let root = scratch("conv");
    let src = root.join("src");
    let s = src.to_string_lossy().to_string();
    init_repo(&s);
    commit(&s, "a.txt", "1", "base");
    sh(&s, &["checkout", "-q", "-b", "feat/x"]);
    commit(&s, "b.txt", "2", "wip x");
    sh(&s, &["checkout", "-q", "main"]);
    let mirror = root.join("mirror");
    clone(&s, &mirror.to_string_lossy());

    // Order A: canonical ingest first, then branches. Order B: branches first.
    let a = store();
    let ia = Ingestor::new("repo");
    ia.ingest_incremental(&mirror, &a).unwrap();
    ia.ingest_branches(&mirror, &a).unwrap();

    let b = store();
    let ib = Ingestor::new("repo");
    ib.ingest_branches(&mirror, &b).unwrap();
    ib.ingest_incremental(&mirror, &b).unwrap();

    let mut ca = titles(&a, "repo", false);
    let mut cb = titles(&b, "repo", false);
    ca.sort();
    cb.sort();
    assert_eq!(ca, cb, "canonical recall converges regardless of trigger order");
    let mut fa = titles(&a, "repo", true);
    let mut fb = titles(&b, "repo", true);
    fa.sort();
    fb.sort();
    assert_eq!(fa, fb, "in-flight recall converges regardless of trigger order");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn force_push_orphan_never_leaks_into_canonical() {
    let root = scratch("fpush");
    let src = root.join("src");
    let s = src.to_string_lossy().to_string();
    init_repo(&s);
    commit(&s, "a.txt", "1", "base");
    sh(&s, &["checkout", "-q", "-b", "feat/x"]);
    commit(&s, "b.txt", "2", "orphan work w1");
    sh(&s, &["checkout", "-q", "main"]);
    let mirror = root.join("mirror");
    clone(&s, &mirror.to_string_lossy());

    let st = store();
    let ing = Ingestor::new("repo");
    drain(&ing, &mirror, &st);
    // Pre-force-push: w1 is in-flight, not canonical.
    assert!(!titles(&st, "repo", false).iter().any(|t| t == "orphan work w1"), "w1 excluded from canonical");
    assert!(titles(&st, "repo", true).iter().any(|t| t == "orphan work w1"), "w1 visible as in-flight");

    // Force-push feat/x: discard w1, put a fresh w2 on top of main.
    sh(&s, &["checkout", "-q", "feat/x"]);
    sh(&s, &["reset", "--hard", "-q", "main"]);
    commit(&s, "c.txt", "3", "kept work w2");
    sh(&s, &["checkout", "-q", "main"]);
    refresh(&mirror);
    drain(&ing, &mirror, &st);

    // The crown-jewel invariant: neither unmerged commit is ever canonical, even
    // though w1's branch node is now archived (grow-only store still holds w1).
    let canonical = titles(&st, "repo", false);
    assert!(!canonical.iter().any(|t| t == "orphan work w1"), "orphaned w1 must NOT leak into canonical: {canonical:?}");
    assert!(!canonical.iter().any(|t| t == "kept work w2"), "unmerged w2 must NOT be canonical: {canonical:?}");
    // The current branch work is still visible in-flight.
    assert!(titles(&st, "repo", true).iter().any(|t| t == "kept work w2"), "w2 visible as in-flight");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn merge_promotes_and_delete_archives_preserving_purity() {
    let root = scratch("mrgdel");
    let src = root.join("src");
    let s = src.to_string_lossy().to_string();
    init_repo(&s);
    commit(&s, "a.txt", "1", "base");
    // Two branches: feat/merge (will merge) and feat/drop (will be deleted unmerged).
    sh(&s, &["checkout", "-q", "-b", "feat/merge"]);
    commit(&s, "m.txt", "1", "work that merges");
    sh(&s, &["checkout", "-q", "main"]);
    sh(&s, &["checkout", "-q", "-b", "feat/drop"]);
    commit(&s, "d.txt", "1", "work that is dropped");
    sh(&s, &["checkout", "-q", "main"]);
    let mirror = root.join("mirror");
    clone(&s, &mirror.to_string_lossy());

    let st = store();
    let ing = Ingestor::new("repo");
    drain(&ing, &mirror, &st);
    // Both are in-flight, neither canonical.
    let c0 = titles(&st, "repo", false);
    assert!(!c0.iter().any(|t| t == "work that merges" || t == "work that is dropped"));
    assert!(titles(&st, "repo", true).iter().any(|t| t == "work that merges"));

    // Merge feat/merge into main; delete feat/drop unmerged.
    sh(&s, &["merge", "-q", "--no-ff", "feat/merge", "-m", "merge feat/merge"]);
    sh(&s, &["branch", "-D", "feat/merge"]);
    sh(&s, &["branch", "-D", "feat/drop"]);
    refresh(&mirror);
    drain(&ing, &mirror, &st);

    let canonical = titles(&st, "repo", false);
    assert!(canonical.iter().any(|t| t == "work that merges"), "merged work promotes to canonical: {canonical:?}");
    assert!(!canonical.iter().any(|t| t == "work that is dropped"), "dropped work must NEVER be canonical: {canonical:?}");

    let _ = std::fs::remove_dir_all(&root);
}
