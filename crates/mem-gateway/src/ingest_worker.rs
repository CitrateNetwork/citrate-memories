//! MEM-S7 WP-7.3 — single-writer ingest worker.
//!
//! Drains the webhook queue (filled by `POST /webhook/github`, WP-7.2) and applies
//! `Ingestor::ingest_incremental` (WP-7.1) so the live store — the same one the
//! gateway serves reads from — stays current without a manual backfill.
//!
//! Two safety properties:
//!   - **Never touches the operator's working clones.** It maintains its OWN mirror
//!     clones under `MEM_INGEST_MIRROR_DIR`, fetched from GitHub and hard-reset to
//!     the default branch. Auto-resetting a dev working tree would clobber WIP.
//!   - **Single writer.** Every ingest holds `write_gate`, serializing with
//!     `/assert` writes; the worker runs one job at a time off a blocking thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use mem_core::VersionedVector;
use mem_index::{EmbedError, Embedder};
use mem_ingest::Ingestor;

use crate::http::AppState;

/// Poll interval for the queue. Pushes are bursty; coalescing per cycle means a
/// flurry of commits to one repo costs one ingest.
const POLL_SECS: u64 = 15;

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
    }
    ingested
}

/// Spawn the background worker. Idle cycles are cheap (a lock + length check).
pub fn spawn(state: AppState) {
    let base = mirror_base();
    tracing::info!("mem-ingest worker: mirrors at {}", base.display());
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

    fn run(args: &[&str]) {
        assert!(Command::new("git").args(args).status().unwrap().success(), "git {args:?}");
    }

    #[test]
    fn ensure_mirror_clones_then_fast_forwards() {
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
