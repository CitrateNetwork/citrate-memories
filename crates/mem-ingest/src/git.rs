//! Read git history by shelling out to `git` (no libgit2 C dependency).
//!
//! Output is parsed from a separator-delimited `git log` so commit subjects and
//! bodies that contain newlines are handled safely.

use std::path::Path;
use std::process::Command;

use crate::IngestError;

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
    // %H sha · %P parents · %an author · %at unix-secs · %s subject · %b body
    let pretty = format!("--pretty=tformat:%H{US}%P{US}%an{US}%at{US}%s{US}%b{RS}");
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--reverse", "--no-color"])
        .arg(pretty)
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("git log failed: {}", stderr.trim())));
    }

    let text = String::from_utf8_lossy(&output.stdout);
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
    Ok(records)
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
