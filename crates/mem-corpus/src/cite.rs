//! Citations for corpus nodes (corpus format 2).
//!
//! Every corpus node's `source_ref` is `Artifact { repo, path, .. }` with `repo`
//! the repository the source cites as and `path` repository-relative, so a
//! passage can be cited as `<repo>:<path>#<anchor>`, the same shape the Citrate
//! QA eval checks (`src/agent/eval/qa.ts` in citrate-core). The anchor is the
//! GitHub-style slug of the chunk's section heading, taken from the
//! `Title › Section` breadcrumb the builder puts on the first line of each chunk.

use mem_core::SourceRef;

/// The breadcrumb separator the chunker writes (`Title › Section`).
pub const BREADCRUMB_SEP: &str = " \u{203a} ";

/// GitHub-style heading slug: lowercase, drop everything but letters, numbers,
/// whitespace, `_` and `-`, then each whitespace character becomes `-`.
pub fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '_' || *c == '-')
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect()
}

/// `prefix/rel`, or `rel` when the prefix is empty.
pub fn repo_relative(prefix: &str, rel: &str) -> String {
    let p = prefix.trim_matches('/');
    if p.is_empty() {
        rel.to_string()
    } else {
        format!("{p}/{rel}")
    }
}

/// The section anchor of a chunk: the slug of the text after the last
/// breadcrumb separator on the first line. `None` for a file-level node or a
/// chunk before the first heading (no section in its breadcrumb).
pub fn section_anchor(content: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(content).ok()?;
    let first = text.lines().next()?;
    let (_, section) = first.rsplit_once(BREADCRUMB_SEP)?;
    let s = slug(section);
    (!s.is_empty()).then_some(s)
}

/// `<repo>:<path>[#<anchor>]` for an artifact node; `None` for any other
/// source kind (commits, assertions, chain events are not document passages).
pub fn cite(source: &SourceRef, content: &[u8]) -> Option<String> {
    match source {
        SourceRef::Artifact { repo, path, .. } => Some(match section_anchor(content) {
            Some(a) => format!("{repo}:{path}#{a}"),
            None => format!("{repo}:{path}"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_the_github_rule_used_by_the_qa_eval() {
        assert_eq!(slug("What it is"), "what-it-is");
        assert_eq!(slug("Step 6, run live"), "step-6-run-live");
        assert_eq!(slug("  `eth_chainId` (0x9d0c)!  "), "eth_chainid-0x9d0c");
        assert_eq!(slug("GhostDAG k-cluster"), "ghostdag-k-cluster");
        assert_eq!(slug("Präzision über Alles"), "präzision-über-alles");
    }

    #[test]
    fn anchor_comes_from_the_breadcrumb_section_only() {
        assert_eq!(
            section_anchor("Genesis \u{203a} What it is\n\nbody".as_bytes()).as_deref(),
            Some("what-it-is")
        );
        // A file-level title node, or a chunk before the first heading.
        assert_eq!(section_anchor(b"Genesis"), None);
        assert_eq!(section_anchor("Genesis\n\nintro text".as_bytes()), None);
        // A separator later in the body is not a breadcrumb.
        assert_eq!(section_anchor("Genesis\n\na \u{203a} b".as_bytes()), None);
        assert_eq!(section_anchor(&[0xff, 0xfe]), None);
    }

    #[test]
    fn cite_is_repo_path_and_anchor_for_artifacts_only() {
        let a = SourceRef::Artifact {
            repo: "citrate-docs".into(),
            path: "content/chain/genesis.md".into(),
            git_sha: "00".into(),
            byte_start: 0,
            byte_end: 1,
        };
        assert_eq!(
            cite(&a, "Genesis \u{203a} What it is\n\nx".as_bytes()).as_deref(),
            Some("citrate-docs:content/chain/genesis.md#what-it-is")
        );
        assert_eq!(
            cite(&a, b"Genesis").as_deref(),
            Some("citrate-docs:content/chain/genesis.md")
        );
        let c = SourceRef::GitCommit {
            repo: "r".into(),
            sha: "s".into(),
        };
        assert_eq!(cite(&c, b"x"), None);
    }

    #[test]
    fn repo_relative_joins_without_doubling_slashes() {
        assert_eq!(repo_relative("content", "chain/a.md"), "content/chain/a.md");
        assert_eq!(repo_relative("content/", "a.md"), "content/a.md");
        assert_eq!(repo_relative("", "README.md"), "README.md");
    }
}
