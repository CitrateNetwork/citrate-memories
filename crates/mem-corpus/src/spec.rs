//! The corpus spec (`corpus/*.toml`): which sources go into a release-time
//! knowledge bundle, which tenant each lands in, and the licence facts the
//! manifest must carry.
//!
//! The spec is plain data. Nothing here touches the filesystem; `build` reads
//! the sources the spec names, relative to a `--sources-base` directory.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::CorpusError;

/// Current spec format version.
pub const SPEC_VERSION: u32 = 1;

/// How a source's files are turned into nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// Walk `root` (or the explicit `include` list) for files with the listed
    /// extensions.
    Docs,
    /// Ship only the skills a HUP-S3.6 `skills.lock` admits for `lock_source`,
    /// and only the files the lock pins, each checked against its sha256.
    Skills,
}

/// One corpus source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    /// Stable source id (recorded on every node's `source_ref.repo`).
    pub id: String,
    /// The memory tenant the source's nodes land in. Must be one of
    /// [`crate::KNOWLEDGE_TENANTS`].
    pub tenant: String,
    pub kind: SourceKind,
    /// Directory relative to the sources base.
    pub root: String,
    /// Upstream location, for the manifest and NOTICE.
    pub upstream: String,
    /// SPDX-style licence expression or a short description of it.
    pub license: String,
    /// Whether redistribution of this source inside the app bundle is cleared.
    /// A source that is not cleared is listed in the manifest and never read.
    pub licence_cleared: bool,
    /// Who decided the licence question, and where (free text, for the manifest).
    #[serde(default)]
    pub licence_note: Option<String>,
    /// Attribution line the NOTICE must carry (licences like CC-BY-SA need one).
    #[serde(default)]
    pub attribution: Option<String>,
    /// A pinned upstream commit. When absent, the build records the checkout's
    /// `HEAD` if `root` is inside a git work tree, else `"unpinned"`.
    #[serde(default)]
    pub commit: Option<String>,
    /// A source that may be absent on the build host. Absent optional sources
    /// are recorded as excluded; an absent required source fails the build.
    #[serde(default)]
    pub optional: bool,
    /// `docs`: file extensions to include (without the dot), e.g. `["md"]`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// `docs`: explicit file list relative to `root`. When non-empty, only
    /// these files are read (no directory walk).
    #[serde(default)]
    pub include: Vec<String>,
    /// `docs`: directory names skipped anywhere in the walk.
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    /// `docs`: frontmatter values a file must carry to ship, e.g.
    /// `{ tier = "public" }`. A file missing any of them is skipped and listed.
    #[serde(default)]
    pub require_frontmatter: BTreeMap<String, String>,
    /// `skills`: path to the `skills.lock`, relative to the sources base.
    #[serde(default)]
    pub skills_lock: Option<String>,
    /// `skills`: the `[[source]].label` in the lock this source corresponds to.
    #[serde(default)]
    pub lock_source: Option<String>,
}

/// A whole corpus spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusSpec {
    pub version: u32,
    /// Corpus name, e.g. `hermes-knowledge`.
    pub name: String,
    /// Target maximum characters per chunk.
    #[serde(default = "default_chunk_chars")]
    pub max_chunk_chars: usize,
    #[serde(rename = "source", default)]
    pub sources: Vec<SourceSpec>,
}

fn default_chunk_chars() -> usize {
    1200
}

impl CorpusSpec {
    /// Parse and validate a spec.
    pub fn from_toml(text: &str) -> Result<Self, CorpusError> {
        let spec: CorpusSpec =
            toml::from_str(text).map_err(|e| CorpusError::Spec(e.to_string()))?;
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<(), CorpusError> {
        if self.version != SPEC_VERSION {
            return Err(CorpusError::Spec(format!(
                "unsupported spec version {} (expected {SPEC_VERSION})",
                self.version
            )));
        }
        if self.max_chunk_chars < 200 {
            return Err(CorpusError::Spec(
                "max_chunk_chars must be at least 200".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.sources {
            if !seen.insert(s.id.as_str()) {
                return Err(CorpusError::Spec(format!("duplicate source id {:?}", s.id)));
            }
            if !crate::KNOWLEDGE_TENANTS.contains(&s.tenant.as_str()) {
                return Err(CorpusError::Spec(format!(
                    "source {:?}: tenant {:?} is not a knowledge tenant",
                    s.id, s.tenant
                )));
            }
            if s.id.is_empty() || s.id.contains(['/', '\\', '|']) {
                return Err(CorpusError::Spec(format!(
                    "source id {:?} is not a plain name",
                    s.id
                )));
            }
            crate::walk::check_relative(&s.root)
                .map_err(|e| CorpusError::Spec(format!("source {:?} root: {e}", s.id)))?;
            for f in &s.include {
                crate::walk::check_relative(f)
                    .map_err(|e| CorpusError::Spec(format!("source {:?} include: {e}", s.id)))?;
            }
            match s.kind {
                SourceKind::Docs => {
                    if s.extensions.is_empty() && s.include.is_empty() {
                        return Err(CorpusError::Spec(format!(
                            "docs source {:?} needs `extensions` or `include`",
                            s.id
                        )));
                    }
                }
                SourceKind::Skills => match (&s.skills_lock, &s.lock_source) {
                    (Some(lock), Some(_)) => crate::walk::check_relative(lock).map_err(|e| {
                        CorpusError::Spec(format!("source {:?} skills_lock: {e}", s.id))
                    })?,
                    _ => {
                        return Err(CorpusError::Spec(format!(
                            "skills source {:?} needs `skills_lock` and `lock_source`",
                            s.id
                        )))
                    }
                },
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = r#"
version = 1
name = "t"

[[source]]
id = "docs"
tenant = "citrate-docs"
kind = "docs"
root = "docs"
upstream = "u"
license = "Apache-2.0"
licence_cleared = true
extensions = ["md"]
"#;

    #[test]
    fn parses_minimal_spec() {
        let s = CorpusSpec::from_toml(MIN).unwrap();
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.max_chunk_chars, 1200);
    }

    #[test]
    fn rejects_runtime_tenants() {
        for t in ["personal", "chain-state", "federation"] {
            let bad = MIN.replace("tenant = \"citrate-docs\"", &format!("tenant = \"{t}\""));
            assert!(CorpusSpec::from_toml(&bad).is_err(), "{t} must be refused");
        }
    }

    #[test]
    fn rejects_escaping_roots() {
        for r in ["../x", "/abs", "a/../../b"] {
            let bad = MIN.replace("root = \"docs\"", &format!("root = \"{r}\""));
            assert!(CorpusSpec::from_toml(&bad).is_err(), "{r} must be refused");
        }
    }

    #[test]
    fn rejects_unknown_keys_and_duplicate_ids() {
        assert!(CorpusSpec::from_toml(&format!("{MIN}\nsurprise = 1\n")).is_err());
        let dup = format!("{MIN}\n{}", &MIN[MIN.find("[[source]]").unwrap()..]);
        assert!(CorpusSpec::from_toml(&dup).is_err());
    }

    #[test]
    fn skills_source_needs_lock() {
        let s = MIN.replace("kind = \"docs\"", "kind = \"skills\"");
        assert!(CorpusSpec::from_toml(&s).is_err());
    }
}
