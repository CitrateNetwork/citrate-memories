//! Read git history by shelling out to `git` (no libgit2 C dependency).
//!
//! Output is parsed from a separator-delimited `git log` so commit subjects and
//! bodies that contain newlines are handled safely.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::IngestError;

/// Resolve the repository top-level directory, so commit and doc ingestion both
/// operate from the same root (git `ls-files` is otherwise subdir-scoped).
pub fn repo_root(path: &Path) -> Result<PathBuf, IngestError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("not a git repo: {}", stderr.trim())));
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

/// One commit as read from `git log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub sha: String,
    /// Parent shas, in git order. The first is the "selected parent" (spine).
    pub parents: Vec<String>,
    pub author: String,
    /// Author date as unix seconds.
    pub time_secs: i64,
    pub subject: String,
    pub body: String,
}

const US: char = '\u{1f}'; // unit separator between fields
const RS: char = '\u{1e}'; // record separator between commits

/// Read the full history (oldest commit first), so a commit's parents are always
/// processed before it.
pub fn read_commits(repo: &Path) -> Result<Vec<CommitRecord>, IngestError> {
    read_commits_range(repo, None)
}

/// Read history (oldest first) optionally restricted to `since..HEAD` — the
/// commits reachable from HEAD but not from `since`. `None` reads everything.
/// This is the incremental-ingest primitive (MEM-S7 WP-7.1): only the commits
/// added since the last watermark are parsed (and re-embedded), not the whole
/// history. `since` MUST be a sha already known good (caller checks
/// {@link is_ancestor}); an unknown rev makes `git log` fail and we return Err.
pub fn read_commits_range(
    repo: &Path,
    since: Option<&str>,
) -> Result<Vec<CommitRecord>, IngestError> {
    // %H sha · %P parents · %an author · %at unix-secs · %s subject · %b body
    let pretty = format!("--pretty=tformat:%H{US}%P{US}%an{US}%at{US}%s{US}%b{RS}");
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .args(["log", "--reverse", "--no-color"])
        .arg(pretty);
    if let Some(s) = since {
        cmd.arg(format!("{s}..HEAD"));
    }
    let output = cmd
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("git log failed: {}", stderr.trim())));
    }

    Ok(parse_commit_log(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse the separator-delimited `git log` output into records (oldest first).
fn parse_commit_log(text: &str) -> Vec<CommitRecord> {
    let mut records = Vec::new();
    for raw in text.split(RS) {
        let raw = raw.trim_start_matches('\n');
        if raw.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = raw.splitn(6, US).collect();
        if fields.len() < 6 {
            continue; // malformed record — skip rather than panic
        }
        let parents = fields[1]
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let time_secs = fields[3].trim().parse::<i64>().unwrap_or(0);
        records.push(CommitRecord {
            sha: fields[0].trim().to_string(),
            parents,
            author: fields[2].to_string(),
            time_secs,
            subject: fields[4].to_string(),
            body: fields[5].to_string(),
        });
    }
    records
}

/// The pretty format shared by all commit reads.
fn commit_pretty() -> String {
    format!("--pretty=tformat:%H{US}%P{US}%an{US}%at{US}%s{US}%b{RS}")
}

/// Read commits in the range `exclude..include` (reachable from `include` but not
/// from `exclude`), oldest first. This is the in-flight branch primitive
/// (ADR-09 B.2): the commits unique to a branch are `default..branch`. Refs must be
/// already-known-good (the caller resolves them); an unknown ref makes git fail.
pub fn read_commits_between(
    repo: &Path,
    exclude: &str,
    include: &str,
) -> Result<Vec<CommitRecord>, IngestError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--reverse", "--no-color"])
        .arg(commit_pretty())
        .arg(format!("{exclude}..{include}"))
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("git log range failed: {}", stderr.trim())));
    }
    Ok(parse_commit_log(&String::from_utf8_lossy(&output.stdout)))
}

/// The default branch's remote-tracking ref (e.g. `origin/main`), resolved from
/// `origin/HEAD`. Falls back to `origin/main` then `origin/master` when a mirror
/// has no `origin/HEAD` set. This is the canonical spine the in-flight layer is
/// measured against.
pub fn default_branch_ref(repo: &Path) -> Result<String, IngestError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "origin/HEAD"])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    if out.status.success() {
        let r = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !r.is_empty() && r != "origin/HEAD" {
            return Ok(r);
        }
    }
    for cand in ["origin/main", "origin/master"] {
        let ok = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", "--quiet", cand])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(cand.to_string());
        }
    }
    Err(IngestError::Git("could not resolve default branch (origin/HEAD)".into()))
}

/// A non-default remote branch and its tip sha (ADR-09 B.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// Bare branch name (e.g. `feat/foo`), with the `origin/` prefix stripped.
    pub name: String,
    pub tip: String,
}

/// Every remote branch except the default and `origin/HEAD`, with its tip. Used by
/// the in-flight layer to enumerate parallel work. `default_ref` is `origin/<name>`.
///
/// Matches on the FULL refname, not `%(refname:short)`. Git abbreviates
/// `refs/remotes/origin/HEAD` to plain **`origin`** (the trailing `HEAD` is
/// dropped, it does not shorten to `origin/HEAD`), so a `short == "origin/HEAD"`
/// guard never fires and the symref gets enumerated as a phantom branch literally
/// named `origin`, pointing at the default tip. Full refnames are unambiguous.
pub fn list_branches(repo: &Path, default_ref: &str) -> Result<Vec<BranchRef>, IngestError> {
    const REMOTE_PREFIX: &str = "refs/remotes/origin/";
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["for-each-ref", &format!("--format=%(refname){US}%(objectname)"), REMOTE_PREFIX])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IngestError::Git(format!("for-each-ref failed: {}", stderr.trim())));
    }
    // `default_ref` is `origin/<name>`; compare on the bare branch name.
    let default_name = default_ref.strip_prefix("origin/").unwrap_or(default_ref);
    let text = String::from_utf8_lossy(&out.stdout);
    let mut branches = Vec::new();
    for line in text.lines() {
        let (refname, tip) = match line.split_once(US) {
            Some(p) => p,
            None => continue,
        };
        let Some(name) = refname.strip_prefix(REMOTE_PREFIX) else { continue };
        if name.is_empty() || name == "HEAD" || name == default_name {
            continue;
        }
        branches.push(BranchRef { name: name.to_string(), tip: tip.trim().to_string() });
    }
    Ok(branches)
}

/// Current HEAD sha (full, 40-hex). Used to stamp the watermark on incremental
/// ingest and to range against the previous watermark.
pub fn head_sha(repo: &Path) -> Result<String, IngestError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("rev-parse HEAD failed: {}", stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Total commit count reachable from HEAD (`git rev-list --count HEAD`). The
/// watermark's `head_count` so "N commits behind" stays exact after incremental
/// updates (additive counting would drift on merges/rewrites).
pub fn commit_count(repo: &Path) -> Result<usize, IngestError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("rev-list --count failed: {}", stderr.trim())));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<usize>()
        .map_err(|e| IngestError::Git(format!("bad rev-list count: {e}")))
}

/// Is `ancestor` an ancestor of HEAD? Detects history rewrites / force-pushes:
/// when the previous watermark head is NO LONGER reachable from HEAD, an
/// incremental range would be wrong, so the caller must re-derive in full.
/// Returns Ok(false) for an unknown sha (treated as "not an ancestor").
pub fn is_ancestor(repo: &Path, ancestor: &str) -> Result<bool, IngestError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, "HEAD"])
        .status()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;
    // exit 0 = ancestor, 1 = not, other = error (unknown rev → 128). Treat
    // anything non-zero-non-one as "not an ancestor" so we fall back to full.
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_repo_history_oldest_first() {
        // The crate lives inside the citrate-memories git repo.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let commits = read_commits(repo).expect("read commits");
        assert!(commits.len() >= 2, "expected >=2 commits, got {}", commits.len());

        // Oldest first: the genesis commit has no parents.
        assert!(commits[0].parents.is_empty(), "first commit should be a root");
        // The second commit's selected parent is the first commit.
        assert_eq!(commits[1].parents.first().map(String::as_str), Some(commits[0].sha.as_str()));
        // Subjects are populated.
        assert!(!commits[0].subject.is_empty());
    }
}
