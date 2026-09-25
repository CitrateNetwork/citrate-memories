//! PBA-L6b-004 regression (2026-09-24 pre-bounty audit): a tracked `*.md`
//! symlink must never make the ingestor read a file OUTSIDE the repo, and a
//! symlink to a device must not become an unbounded read under the write gate.
//! This is the audit PoC `evidence/memories/l6b_ingest_symlink.rs` with its
//! assertion inverted, driven through the public `Ingestor::ingest` entry point.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use mem_core::MemoryNode;
use mem_ingest::Ingestor;
use mem_store::{kv::InMemoryKv, MemoryDagStore};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// A fresh scratch dir unique to this test + process.
fn scratch(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("pba-l6b-004-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn contents(store: &MemoryDagStore<MemoryNode>) -> Vec<String> {
    store
        .all_nodes()
        .unwrap()
        .into_iter()
        .map(|n| String::from_utf8_lossy(&n.content).to_string())
        .collect()
}

/// Inverted PoC: `docs/leak.md -> <host>/mem-gateway.env` must NOT surface the
/// host file's heading as a node, while the repo's real docs still ingest.
#[test]
fn pba_l6b_004_symlinked_md_does_not_read_host_file_outside_repo() {
    let base = scratch("hostfile");
    let repo = base.join("repo");
    let host = base.join("host-only");
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::create_dir_all(&host).unwrap();
    let secret = host.join("mem-gateway.env");
    std::fs::write(&secret, "# HOST-SECRET MEM_CONNECT_SECRET=hunter2-not-in-repo\nX=1\n").unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "# readme heading\n").unwrap();
    std::os::unix::fs::symlink(&secret, repo.join("docs/leak.md")).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "docs: add leak"]);

    let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    Ingestor::new("victim-repo").ingest(&repo, &store).unwrap();
    let all = contents(&store);
    let _ = std::fs::remove_dir_all(&base);

    let leaked: Vec<&String> = all.iter().filter(|c| c.contains("HOST-SECRET")).collect();
    assert!(leaked.is_empty(), "PBA-L6b-004: host file content landed in the store: {leaked:?}");
    assert!(
        all.iter().any(|c| c.contains("readme heading")),
        "no false negative: the repo's regular README.md still ingests"
    );
}

/// DoS half: a tracked `*.md` symlink to `/dev/zero` used to be an unbounded
/// read under the gateway's write gate (OOM → crash loop). Ingest must finish
/// promptly and skip it. (Not run red: the pre-fix behaviour exhausts memory.)
#[test]
fn pba_l6b_004_symlink_to_dev_zero_is_skipped_not_read() {
    let base = scratch("devzero");
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "# ok\n").unwrap();
    std::os::unix::fs::symlink("/dev/zero", repo.join("zero.md")).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "zero"]);

    let started = std::time::Instant::now();
    let store: MemoryDagStore<MemoryNode> = MemoryDagStore::new(Box::new(InMemoryKv::new()));
    Ingestor::new("r").ingest(&repo, &store).unwrap();
    let _ = std::fs::remove_dir_all(&base);
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "ingest must not read /dev/zero");
    assert!(contents(&store).iter().any(|c| c.contains("ok")));
}

/// `read_tracked_doc` unit contract: regular in-repo file ok; symlink (even to
/// an in-repo file), oversize file, and a path under a symlinked directory that
/// escapes the repo are all refused.
#[test]
fn pba_l6b_004_read_tracked_doc_contract() {
    use mem_ingest::{read_tracked_doc, MAX_DOC_BYTES};
    let base = scratch("contract");
    let repo = base.join("repo");
    let outside = base.join("outside");
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(repo.join("docs/a.md"), "# a\n").unwrap();
    std::fs::write(outside.join("secret.md"), "# secret\n").unwrap();
    std::os::unix::fs::symlink(repo.join("docs/a.md"), repo.join("docs/link.md")).unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("escape")).unwrap();
    let exact = vec![b'x'; MAX_DOC_BYTES as usize];
    std::fs::write(repo.join("exact.md"), &exact).unwrap();
    let mut big = exact.clone();
    big.push(b'x');
    std::fs::write(repo.join("big.md"), &big).unwrap();

    assert_eq!(read_tracked_doc(&repo, "docs/a.md").as_deref(), Some(&b"# a\n"[..]));
    assert!(read_tracked_doc(&repo, "docs/link.md").is_none(), "symlink never followed");
    assert!(read_tracked_doc(&repo, "escape/secret.md").is_none(), "symlinked parent escaping the repo");
    assert!(read_tracked_doc(&repo, "docs").is_none(), "directory refused");
    assert!(read_tracked_doc(&repo, "missing.md").is_none());
    assert_eq!(read_tracked_doc(&repo, "exact.md").map(|b| b.len()), Some(MAX_DOC_BYTES as usize), "cap is inclusive");
    assert!(read_tracked_doc(&repo, "big.md").is_none(), "over the cap is skipped");
    let _ = std::fs::remove_dir_all(&base);
}

/// Tripwire: ingest source must read repo-tracked files only through
/// `read_tracked_doc` — no direct, symlink-following `fs::read` of a repo path.
#[test]
fn pba_l6b_004_tripwire_no_direct_repo_reads() {
    let src = include_str!("../src/lib.rs");
    for needle in [concat!("fs::read(repo", "_root"), concat!("fs::read_to_string(repo", "_root")] {
        assert!(!src.contains(needle), "PBA-L6b-004: direct repo read `{needle}` bypasses read_tracked_doc");
    }
    assert!(src.contains("read_tracked_doc(repo_root, rel)"), "build_doc_graph must use read_tracked_doc");
}
