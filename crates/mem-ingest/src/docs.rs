//! Markdown document ingestion helpers (WP-1.3): classify a `.md` file into a
//! node kind, derive a title, map fenced-block directives to edges, and list the
//! tracked markdown files in a repo.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use mem_core::{ClaimStatus, EdgeKind, NarrativeKind, NodeKind, SprintPhase, Status};

use crate::IngestError;

/// Classify a doc by path + frontmatter. Deterministic.
pub fn classify(rel_path: &str, fm: &BTreeMap<String, String>, body: &str) -> NodeKind {
    // Leading slash so `/segment/` checks match both top-level and nested paths.
    let p = format!("/{}", rel_path.to_ascii_lowercase());
    let file = p.rsplit('/').next().unwrap_or(&p);

    // A fenced `node: claim` with a claim-status wins (explicit author intent).
    if let Some(cs) = claim_status_from_body(body) {
        return NodeKind::Claim(cs);
    }

    if file.starts_with("adr-") || p.contains("/adrs/") {
        NodeKind::Adr
    } else if p.contains("/sprints/") || fm.contains_key("sprint") {
        // Phase is approximate from a document; RETRO/archived => Close.
        let phase = if file.contains("retro") || fm.get("status").map(String::as_str) == Some("archived") {
            SprintPhase::Close
        } else {
            SprintPhase::Kickoff
        };
        NodeKind::Sprint(phase)
    } else if p.contains("/journal/") || p.contains("/journals/") {
        NodeKind::Narrative(NarrativeKind::Journal)
    } else if p.contains("/essays/") {
        NodeKind::Narrative(NarrativeKind::Essay)
    } else if p.contains("/case_studies/") || p.contains("/case-studies/") {
        NodeKind::Narrative(NarrativeKind::CaseStudy)
    } else if p.contains("/handoffs/") || file.contains("handoff") {
        NodeKind::Handoff
    } else {
        NodeKind::Doc
    }
}

fn claim_status_from_body(body: &str) -> Option<ClaimStatus> {
    let blocks = crate::trailers::parse_agentile_blocks(body);
    let is_claim = blocks.iter().any(|t| t.key.eq_ignore_ascii_case("node") && t.value.eq_ignore_ascii_case("claim"));
    if !is_claim {
        return None;
    }
    blocks
        .iter()
        .find(|t| t.key.eq_ignore_ascii_case("claim-status"))
        .and_then(|t| match t.value.to_ascii_lowercase().as_str() {
            "confirmed" => Some(ClaimStatus::Confirmed),
            "corrected" => Some(ClaimStatus::Corrected),
            "refined" => Some(ClaimStatus::Refined),
            _ => None,
        })
}

/// Map a fenced-block directive key to the edge it asserts.
pub fn map_block_directive(key: &str) -> Option<EdgeKind> {
    match key.to_ascii_lowercase().as_str() {
        "implements" => Some(EdgeKind::Implements),
        "decides" => Some(EdgeKind::Decides),
        "supersedes" => Some(EdgeKind::Supersedes),
        "refutes" => Some(EdgeKind::Refutes),
        "depends-on" => Some(EdgeKind::DependsOn),
        "references" | "ref" | "see-also" => Some(EdgeKind::References),
        _ => None, // node:/claim-status:/why:/anchors: are attributes, not edges
    }
}

/// Document title: first `# H1` heading, else frontmatter title/sprint, else the
/// filename stem. Identity-bearing, so it is stable and human-meaningful.
pub fn doc_title(rel_path: &str, fm: &BTreeMap<String, String>, body: &str) -> String {
    for line in body.lines() {
        let l = line.trim();
        if let Some(h) = l.strip_prefix("# ") {
            if !h.trim().is_empty() {
                return h.trim().to_string();
            }
        }
    }
    if let Some(t) = fm.get("title").or_else(|| fm.get("sprint")) {
        if !t.is_empty() {
            return t.clone();
        }
    }
    rel_path
        .rsplit('/')
        .next()
        .unwrap_or(rel_path)
        .trim_end_matches(".md")
        .to_string()
}

/// Map a frontmatter `status:` to a node [`Status`].
pub fn status_from_fm(fm: &BTreeMap<String, String>) -> Status {
    match fm.get("status").map(String::as_str) {
        Some("superseded") => Status::Superseded,
        Some("archived") => Status::Archived,
        _ => Status::Active,
    }
}

/// List tracked markdown files (relative paths), via `git ls-files`. Deterministic
/// and gitignore-aware.
pub fn list_md_files(repo: &Path) -> Result<Vec<String>, IngestError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--", "*.md"])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git ls-files: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("git ls-files failed: {}", stderr.trim())));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

/// Map each tracked markdown file to its git **blob sha** (from the index), via
/// `git ls-files -s`. MEM-B-014: a doc node's identity must be a pure function of
/// git — the blob sha is the same on every machine and immune to worktree
/// nondeterminism (autocrlf / `.gitattributes` smudge filters / LFS / a dirty
/// tree), unlike the materialized-file byte length it replaces.
pub fn list_md_blobs(repo: &Path) -> Result<BTreeMap<String, String>, IngestError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-s", "--", "*.md"])
        .output()
        .map_err(|e| IngestError::Git(format!("failed to run git ls-files -s: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IngestError::Git(format!("git ls-files -s failed: {}", stderr.trim())));
    }
    // Each line: `<mode> <sha> <stage>\t<path>` (e.g. `100644 <sha> 0\tdocs/x.md`).
    let text = String::from_utf8_lossy(&output.stdout);
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some((meta, path)) = line.split_once('\t') else { continue };
        let mut cols = meta.split_whitespace();
        let (_mode, sha) = (cols.next(), cols.next());
        if let Some(sha) = sha {
            out.insert(path.trim().to_string(), sha.to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn classifies_by_path_and_frontmatter() {
        assert_eq!(classify("docs/adrs/ADR-2026-x.md", &fm(&[]), ""), NodeKind::Adr);
        assert!(matches!(classify("a/ADR-foo.md", &fm(&[]), ""), NodeKind::Adr));
        assert!(matches!(
            classify("repos/x/sprints/active/MEM-S1.md", &fm(&[]), ""),
            NodeKind::Sprint(SprintPhase::Kickoff)
        ));
        assert!(matches!(
            classify("x.md", &fm(&[("sprint", "MEM-S0"), ("status", "archived")]), ""),
            NodeKind::Sprint(SprintPhase::Close)
        ));
        assert!(matches!(
            classify("docs/journal/saul/2026-06-04.md", &fm(&[]), ""),
            NodeKind::Narrative(NarrativeKind::Journal)
        ));
        assert_eq!(classify("PLANSET/00_OVERVIEW.md", &fm(&[]), ""), NodeKind::Doc);
        assert_eq!(classify("handoffs/PIN_DGX.md", &fm(&[]), ""), NodeKind::Handoff);
    }

    #[test]
    fn claim_block_overrides_classification() {
        let body = "intro\n\n```agentile\nnode: claim\nclaim-status: corrected\n```\n";
        assert!(matches!(classify("PLANSET/x.md", &fm(&[]), body), NodeKind::Claim(ClaimStatus::Corrected)));
    }

    #[test]
    fn title_prefers_h1_then_frontmatter_then_filename() {
        assert_eq!(doc_title("a/b.md", &fm(&[]), "# Real Title\n\nbody"), "Real Title");
        assert_eq!(doc_title("a/b.md", &fm(&[("sprint", "MEM-S0")]), "no heading"), "MEM-S0");
        assert_eq!(doc_title("a/my-doc.md", &fm(&[]), "no heading"), "my-doc");
    }

    #[test]
    fn maps_only_edge_directives() {
        assert_eq!(map_block_directive("supersedes"), Some(EdgeKind::Supersedes));
        assert_eq!(map_block_directive("implements"), Some(EdgeKind::Implements));
        assert_eq!(map_block_directive("claim-status"), None);
        assert_eq!(map_block_directive("why"), None);
    }
}
