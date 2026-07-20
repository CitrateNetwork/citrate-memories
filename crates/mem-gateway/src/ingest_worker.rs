//! MEM-S7 WP-7.3/7.4 — single-writer ingest worker + reconciler.
//!
//! Drains the webhook queue (filled by `POST /webhook/github`, WP-7.2) and applies
//! `Ingestor::ingest_incremental` (WP-7.1) so the live store — the same one the
//! gateway serves reads from — stays current without a manual backfill.
//!
//! **WP-7.4 reconciler (the completeness backstop).** The webhook is the real-time
//! path, but on its own it is not a guarantee: a dropped delivery, a repo whose hook
//! was never configured, or a brand-new repo would silently drift. So a second timer
//! periodically sweeps every known federation repo, compares its true remote HEAD
//! (`git ls-remote`) against the stored `Watermark.head`, and enqueues any that
//! diverged. This is what lets the graph be trusted as canonical truth: every repo
//! is caught within one reconcile interval even if no webhook ever fires.
//!
//! Two safety properties:
//!   - **Never touches the operator's working clones.** It maintains its OWN mirror
//!     clones under `MEM_INGEST_MIRROR_DIR`, fetched from GitHub and hard-reset to
//!     the default branch. Auto-resetting a dev working tree would clobber WIP.
//!   - **Single writer.** Every ingest holds `write_gate`, serializing with
//!     `/assert` writes; the worker runs one job at a time off a blocking thread.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mem_core::{MemoryNode, NodeKind, Status, VersionedVector};
use mem_index::{EmbedError, Embedder};
use mem_ingest::Ingestor;
use mem_store::MemoryDagStore;

use crate::http::AppState;
use crate::webhook::{PushEvent, ALLOWED_OWNER};

/// Poll interval for the queue. Pushes are bursty; coalescing per cycle means a
/// flurry of commits to one repo costs one ingest.
const POLL_SECS: u64 = 15;

/// Reconcile sweep interval (WP-7.4). A HEAD compare per repo is cheap, so this can
/// be frequent; overridable via `MEM_INGEST_RECONCILE_SECS`. Default 5 minutes.
const RECONCILE_SECS_DEFAULT: u64 = 300;

/// Adapts a shared (`Arc`) query embedder to the `Box<dyn Embedder>` the Ingestor
/// takes, so the worker embeds new commits in the SAME vector space the store was
/// built with (mixing models would trip the index's model guard).
struct SharedEmbedder(Arc<dyn Embedder>);
impl Embedder for SharedEmbedder {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }
    fn dim(&self) -> usize {
        self.0.dim()
    }
    fn embed(&self, text: &str) -> Result<VersionedVector, EmbedError> {
        self.0.embed(text)
    }
}

fn git(args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Run git and capture stdout (for `ls-remote`). Same failure semantics as `git`.
fn git_stdout(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The clone base for repos. SSH scp-syntax by default (the working clones already
/// auth this way); overridable for tests via `MEM_INGEST_GIT_BASE`.
fn git_base() -> String {
    std::env::var("MEM_INGEST_GIT_BASE")
        .unwrap_or_else(|_| "git@github.com:CitrateNetwork".to_string())
}

/// Ensure a gateway-owned mirror of `repo` exists under `base`, fetched to the
/// remote's default branch. Returns the mirror path. `repo` MUST be a
/// webhook-validated safe name (no traversal / shell metachars) — the caller
/// guarantees this via `webhook::allowed_repo`.
pub fn ensure_mirror(base: &Path, repo: &str) -> Result<PathBuf, String> {
    let dir = base.join(repo);
    if dir.join(".git").is_dir() {
        let d = dir.to_string_lossy().to_string();
        git(&["-C", &d, "fetch", "origin", "--prune", "--quiet"])?;
        // origin/HEAD tracks the default branch (set at clone). Hard-reset the
        // mirror to it so ingest reads the freshly-fetched tip.
        git(&["-C", &d, "reset", "--hard", "-q", "origin/HEAD"])?;
    } else {
        std::fs::create_dir_all(base).map_err(|e| format!("mkdir {}: {e}", base.display()))?;
        let url = format!("{}/{}.git", git_base(), repo);
        git(&["clone", "--quiet", &url, &dir.to_string_lossy()])?;
    }
    Ok(dir)
}

/// The mirror root: `MEM_INGEST_MIRROR_DIR` or a sensible user-writable default.
fn mirror_base() -> PathBuf {
    PathBuf::from(
        std::env::var("MEM_INGEST_MIRROR_DIR")
            .unwrap_or_else(|_| "/home/saul/.cache/memrizz/mirrors".to_string()),
    )
}

/// Drain every queued event once: coalesce to unique repos, then mirror + ingest
/// each under the write lock. A failure on one repo is logged and skipped; it does
/// not block the others or crash the worker.
pub fn drain_once(state: &AppState, base: &Path) -> usize {
    let repos: Vec<String> = {
        let mut q = match state.ingest_queue.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        while let Some(ev) = q.pop_front() {
            if seen.insert(ev.repo.clone()) {
                out.push(ev.repo);
            }
        }
        out
    };
    let mut ingested = 0;
    for repo in repos {
        let path = match ensure_mirror(base, &repo) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("mem-ingest: mirror {repo} failed: {e}");
                continue;
            }
        };
        let ingestor = match &state.embedder {
            Some(e) => Ingestor::with_embedder(repo.clone(), Box::new(SharedEmbedder(e.clone()))),
            None => Ingestor::new(repo.clone()),
        };
        // Single-writer: hold the write gate across the ingest.
        let _w = match state.write_gate.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match ingestor.ingest_incremental(&path, &state.store) {
            Ok(rep) => {
                ingested += 1;
                tracing::info!(
                    "mem-ingest: {repo} +{} commits (+{} docs); store now {} nodes, head_count {}",
                    rep.commits,
                    rep.docs,
                    rep.nodes_in_store,
                    rep.watermark.head_count
                );
            }
            Err(e) => tracing::error!("mem-ingest: ingest {repo} failed: {e}"),
        }

        // ADR-09 B.3: in-flight branch layer, under the same write gate. Ingest
        // non-default branches, then reap merged/deleted/advanced Branch nodes so
        // canonical recall promotes merged work automatically. Best-effort: a branch
        // failure never blocks the canonical ingest above.
        match ingestor.ingest_branches(&path, &state.store) {
            Ok(r) if r.branches_ingested > 0 => tracing::info!(
                "mem-ingest: {repo} in-flight +{} branch(es) (+{} commits)",
                r.branches_ingested,
                r.in_flight_commits
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("mem-ingest: branch ingest {repo} failed: {e}"),
        }
        match ingestor.reap_branches(&path, &state.store) {
            Ok(r) if r.archived > 0 => tracing::info!(
                "mem-ingest: {repo} reaped {} branch(es) (merged {} / deleted {} / advanced {})",
                r.archived,
                r.merged,
                r.deleted,
                r.advanced
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("mem-ingest: branch reap {repo} failed: {e}"),
        }
    }
    ingested
}

// --------------------------------------------------------------------------
// WP-7.4 reconciler — the completeness backstop
// --------------------------------------------------------------------------

/// Validate a bare repo name through the same allowlist the webhook uses, so a
/// name that reaches `git` is always org-scoped and free of shell/path metachars.
fn safe_repo(name: &str) -> Option<String> {
    crate::webhook::allowed_repo(&format!("{ALLOWED_OWNER}/{}", name.trim()))
}

/// The set of federation repos to keep current: every repo we've already mirrored
/// (i.e. ingested at least once) plus an explicit `MEM_INGEST_REPOS` list (comma or
/// whitespace separated). The env list is how repos that have *no webhook* — or
/// were never touched — still get swept, which is what makes coverage complete
/// rather than best-effort. New repos also enter naturally on their first webhook.
fn federation_repos(base: &Path) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for e in entries.flatten() {
            if e.path().join(".git").is_dir() {
                if let Some(name) = e.file_name().to_str().and_then(safe_repo) {
                    set.insert(name);
                }
            }
        }
    }
    if let Ok(list) = std::env::var("MEM_INGEST_REPOS") {
        for name in list.split(|c: char| c == ',' || c.is_whitespace()) {
            if !name.trim().is_empty() {
                if let Some(name) = safe_repo(name) {
                    set.insert(name);
                }
            }
        }
    }
    set.into_iter().collect()
}

/// The remote's default-branch tip via `git ls-remote <url> HEAD` — a cheap HEAD
/// compare, no fetch. `None` on any git failure (unreachable/renamed repo): the
/// sweep logs and skips rather than guessing.
fn remote_head(repo: &str) -> Option<String> {
    let url = format!("{}/{}.git", git_base(), repo);
    let out = git_stdout(&["ls-remote", &url, "HEAD"]).ok()?;
    out.split_whitespace().next().map(|s| s.to_string())
}

/// A repo is drifted if it has never been ingested, or its remote head has moved
/// past the ingested watermark.
fn is_drifted(watermark_head: Option<&str>, remote_head: &str) -> bool {
    match watermark_head {
        None => true,
        Some(h) => h != remote_head,
    }
}

/// The default branch name and every branch tip from the remote, via `ls-remote`
/// (no fetch): `(default_name, [(name, tip)])`. Lets the reconciler catch
/// branch-only drift — a push to a feature branch that never touches the default.
fn remote_branches(repo: &str) -> Result<(String, Vec<(String, String)>), String> {
    let url = format!("{}/{}.git", git_base(), repo);
    let sym = git_stdout(&["ls-remote", "--symref", &url, "HEAD"])?;
    let default = sym
        .lines()
        .find_map(|l| l.strip_prefix("ref:").and_then(|r| r.split_whitespace().next()))
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .unwrap_or("")
        .to_string();
    let heads = git_stdout(&["ls-remote", "--heads", &url])?;
    let mut out = Vec::new();
    for line in heads.lines() {
        let mut it = line.split_whitespace();
        if let (Some(sha), Some(refname)) = (it.next(), it.next()) {
            if let Some(name) = refname.strip_prefix("refs/heads/") {
                out.push((name.to_string(), sha.to_string()));
            }
        }
    }
    Ok((default, out))
}

/// True if any non-default branch has moved relative to its stored watermark (new
/// or advanced), or an Active `Branch` node's branch has vanished from the remote
/// (deleted/merged). Either way the repo needs a drain, which ingests + reaps.
fn branch_drifted(repo: &str, store: &MemoryDagStore<MemoryNode>, all: &[MemoryNode]) -> bool {
    let (default, branches) = match remote_branches(repo) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("reconcile: ls-remote --heads {repo} failed: {e}");
            return false;
        }
    };
    let ing = Ingestor::new(repo.to_string());
    for (name, tip) in &branches {
        if *name == default {
            continue;
        }
        let seen = ing.read_branch_watermark(store, name).ok().flatten().and_then(|w| w.head);
        if seen.as_deref() != Some(tip.as_str()) {
            return true; // new or advanced feature branch
        }
    }
    let live: HashSet<&str> = branches.iter().map(|(n, _)| n.as_str()).collect();
    all.iter().any(|n| {
        n.kind == NodeKind::Branch
            && n.repo == repo
            && n.status == Status::Active
            && String::from_utf8_lossy(&n.content)
                .rsplit_once('@')
                .map(|(name, _)| !live.contains(name))
                .unwrap_or(false)
    })
}

/// The testable core of WP-7.4 (+ ADR-09 B.3): for every federation repo, enqueue
/// it if its default HEAD drifted from the watermark OR any feature branch drifted
/// (new/advanced/deleted). Skips repos already queued by a webhook or an earlier
/// sweep. Does not ingest — the single-writer drain loop does that, so reconcile
/// stays lock-light.
pub fn reconcile_into_queue(
    store: &MemoryDagStore<MemoryNode>,
    queue: &Mutex<VecDeque<PushEvent>>,
    base: &Path,
) -> usize {
    let already: HashSet<String> = {
        let q = queue.lock().unwrap_or_else(|p| p.into_inner());
        q.iter().map(|e| e.repo.clone()).collect()
    };
    // One snapshot for branch-node lookups (deleted-branch detection) this sweep.
    let all = store.all_nodes().unwrap_or_default();
    let mut enqueued = 0;
    for repo in federation_repos(base) {
        if already.contains(&repo) {
            continue;
        }
        let remote = match remote_head(&repo) {
            Some(h) => h,
            None => {
                tracing::warn!("reconcile: ls-remote {repo} failed; skipping this sweep");
                continue;
            }
        };
        let wm = Ingestor::new(repo.clone()).read_watermark(store).ok().flatten();
        let default_drift = is_drifted(wm.as_ref().and_then(|w| w.head.as_deref()), &remote);
        if default_drift || branch_drifted(&repo, store, &all) {
            queue
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push_back(PushEvent { repo: repo.clone(), git_ref: "refs/heads/HEAD".into(), after: remote });
            enqueued += 1;
            tracing::info!("reconcile: {repo} drifted → enqueued for ingest");
        }
    }
    enqueued
}

/// WP-7.4 sweep wired to the live gateway state.
pub fn reconcile_once(state: &AppState, base: &Path) -> usize {
    reconcile_into_queue(&state.store, &state.ingest_queue, base)
}

/// Spawn the background workers: the WP-7.4 reconcile sweep (startup + interval) and
/// the WP-7.3 drain loop. The reconciler only enqueues; the drain loop is the one
/// writer, so the two never contend for the store.
pub fn spawn(state: AppState) {
    let base = mirror_base();
    tracing::info!("mem-ingest worker: mirrors at {}", base.display());

    // Reconciler: sweep once at startup (catches everything missed while the gateway
    // was down, plus repos with no webhook) and every RECONCILE_SECS thereafter.
    {
        let state = state.clone();
        let base = base.clone();
        let secs = std::env::var("MEM_INGEST_RECONCILE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(RECONCILE_SECS_DEFAULT);
        tokio::spawn(async move {
            loop {
                let st = state.clone();
                let b = base.clone();
                match tokio::task::spawn_blocking(move || reconcile_once(&st, &b)).await {
                    Ok(n) if n > 0 => tracing::info!("reconcile: {n} repo(s) enqueued for ingest"),
                    Ok(_) => {}
                    Err(e) => tracing::error!("reconcile task panicked: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(secs)).await;
            }
        });
    }

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
            let depth = match state.ingest_queue.lock() {
                Ok(g) => g.len(),
                Err(p) => p.into_inner().len(),
            };
            if depth == 0 {
                continue;
            }
            let st = state.clone();
            let b = base.clone();
            // git + ingest are blocking; keep them off the async reactor.
            if let Err(e) = tokio::task::spawn_blocking(move || drain_once(&st, &b)).await {
                tracing::error!("mem-ingest worker: drain task panicked: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serializes tests that mutate process-global env (`MEM_INGEST_GIT_BASE`,
    /// `MEM_INGEST_REPOS`) so they don't race under `cargo test`'s parallelism.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn run(args: &[&str]) {
        assert!(Command::new("git").args(args).status().unwrap().success(), "git {args:?}");
    }

    /// Create a local git remote `<remotes>/<name>.git` with one commit; returns its
    /// stringified path (for `-C`).
    fn make_remote(remotes: &Path, name: &str) -> String {
        let src = remotes.join(format!("{name}.git"));
        std::fs::create_dir_all(&src).unwrap();
        let s = src.to_string_lossy().to_string();
        run(&["-C", &s, "init", "-q"]);
        run(&["-C", &s, "config", "user.email", "t@t.t"]);
        run(&["-C", &s, "config", "user.name", "t"]);
        run(&["-C", &s, "config", "commit.gpgsign", "false"]);
        std::fs::write(src.join("a.txt"), "1").unwrap();
        run(&["-C", &s, "add", "."]);
        run(&["-C", &s, "commit", "-q", "-m", "first"]);
        s
    }

    #[test]
    fn reconcile_enqueues_drifted_then_skips_current() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mem-reconcile-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        let remotes = root.join("remotes");
        make_remote(&remotes, "citrate-y");

        std::env::set_var("MEM_INGEST_GIT_BASE", remotes.to_string_lossy().to_string());
        std::env::set_var("MEM_INGEST_REPOS", "citrate-y");

        let store = MemoryDagStore::new(Box::new(mem_store::kv::InMemoryKv::new()));
        let queue: Mutex<VecDeque<PushEvent>> = Mutex::new(VecDeque::new());
        let base = root.join("mirrors");
        std::fs::create_dir_all(&base).unwrap();

        // Never ingested → drifted → enqueued exactly once.
        assert_eq!(reconcile_into_queue(&store, &queue, &base), 1, "drifted repo enqueued");
        assert_eq!(queue.lock().unwrap().front().unwrap().repo, "citrate-y");
        queue.lock().unwrap().clear();

        // Actually ingest it (writes a watermark at HEAD), then reconcile again:
        // now current → nothing enqueued (the completeness guarantee is convergent).
        let mirror = ensure_mirror(&base, "citrate-y").expect("mirror");
        Ingestor::new("citrate-y".to_string()).ingest_incremental(&mirror, &store).expect("ingest");
        assert_eq!(reconcile_into_queue(&store, &queue, &base), 0, "current repo not re-enqueued");
        assert!(queue.lock().unwrap().is_empty());

        std::env::remove_var("MEM_INGEST_GIT_BASE");
        std::env::remove_var("MEM_INGEST_REPOS");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn drift_predicate() {
        assert!(is_drifted(None, "abc"), "never-ingested is drifted");
        assert!(is_drifted(Some("abc"), "def"), "moved head is drifted");
        assert!(!is_drifted(Some("abc"), "abc"), "unchanged head is current");
    }

    #[test]
    fn branch_drifted_detects_new_feature_branch() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mem-bdrift-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("remotes").join("citrate-b.git");
        std::fs::create_dir_all(&src).unwrap();
        let s = src.to_string_lossy().to_string();
        run(&["-C", &s, "init", "-q", "-b", "main"]);
        run(&["-C", &s, "config", "user.email", "t@t.t"]);
        run(&["-C", &s, "config", "user.name", "t"]);
        run(&["-C", &s, "config", "commit.gpgsign", "false"]);
        std::fs::write(src.join("a.txt"), "1").unwrap();
        run(&["-C", &s, "add", "."]);
        run(&["-C", &s, "commit", "-q", "-m", "base"]);
        run(&["-C", &s, "checkout", "-q", "-b", "feat/y"]);
        std::fs::write(src.join("b.txt"), "2").unwrap();
        run(&["-C", &s, "add", "."]);
        run(&["-C", &s, "commit", "-q", "-m", "wip"]);
        run(&["-C", &s, "checkout", "-q", "main"]); // HEAD back to the default

        std::env::set_var("MEM_INGEST_GIT_BASE", root.join("remotes").to_string_lossy().to_string());
        let store = MemoryDagStore::new(Box::new(mem_store::kv::InMemoryKv::new()));
        // A non-default branch with no per-branch watermark is drift → needs a drain.
        assert!(branch_drifted("citrate-b", &store, &[]), "a new feature branch must register as drift");

        std::env::remove_var("MEM_INGEST_GIT_BASE");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_mirror_clones_then_fast_forwards() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mem-mirror-test-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        let remotes = root.join("remotes");
        // Named with the `.git` suffix the URL builder appends.
        let src = remotes.join("citrate-x.git");
        std::fs::create_dir_all(&src).unwrap();
        let s = src.to_string_lossy().to_string();
        run(&["-C", &s, "init", "-q"]);
        run(&["-C", &s, "config", "user.email", "t@t.t"]);
        run(&["-C", &s, "config", "user.name", "t"]);
        run(&["-C", &s, "config", "commit.gpgsign", "false"]);
        std::fs::write(src.join("a.txt"), "1").unwrap();
        run(&["-C", &s, "add", "."]);
        run(&["-C", &s, "commit", "-q", "-m", "first"]);

        // Point the clone base at the local "remotes" dir.
        std::env::set_var("MEM_INGEST_GIT_BASE", remotes.to_string_lossy().to_string());
        let mirrors = root.join("mirrors");

        // First call clones the mirror.
        let mdir = ensure_mirror(&mirrors, "citrate-x").expect("clone");
        assert!(mdir.join("a.txt").exists());

        // Advance the source, then a second call fast-forwards the mirror.
        std::fs::write(src.join("b.txt"), "2").unwrap();
        run(&["-C", &s, "add", "."]);
        run(&["-C", &s, "commit", "-q", "-m", "second"]);
        ensure_mirror(&mirrors, "citrate-x").expect("fetch");
        assert!(mdir.join("b.txt").exists(), "mirror fast-forwarded to new commit");

        std::env::remove_var("MEM_INGEST_GIT_BASE");
        let _ = std::fs::remove_dir_all(&root);
    }
}
