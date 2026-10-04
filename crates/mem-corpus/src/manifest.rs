//! The corpus manifest: what went in, from where, under which licence, and
//! the hashes that let the importer prove the files on disk are those files.

use serde::{Deserialize, Serialize};

use crate::{sha256_hex, CorpusError};

/// One file that shipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Path relative to the source root.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    /// Chunk nodes minted from the file.
    pub chunks: usize,
}

/// One file that did not ship, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkippedEntry {
    pub path: String,
    pub reason: String,
}

/// A source as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEntry {
    pub id: String,
    pub tenant: String,
    /// The repository the source's nodes cite (format 2).
    pub repo: String,
    /// Where the source root sits inside `repo`; node paths are
    /// `repo_path/<file path>` (file paths below stay root-relative).
    pub repo_path: String,
    pub upstream: String,
    pub commit: String,
    pub license: String,
    pub licence_cleared: bool,
    #[serde(default)]
    pub licence_note: Option<String>,
    #[serde(default)]
    pub attribution: Option<String>,
    /// False when the source was left out as a whole (licence not cleared, or an
    /// optional source absent on the build host). See `excluded_reason`.
    pub included: bool,
    #[serde(default)]
    pub excluded_reason: Option<String>,
    pub files: Vec<FileEntry>,
    pub skipped: Vec<SkippedEntry>,
}

/// One tenant bundle file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantEntry {
    pub tenant: String,
    /// Path relative to the corpus directory.
    pub file: String,
    pub sha256: String,
    pub nodes: usize,
    pub edges: usize,
    /// Precomputed node vectors (see [`crate::vectors`]), when the release
    /// build embedded the corpus.
    #[serde(default)]
    pub vectors: Option<crate::vectors::VectorsEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub name: String,
    /// The corpus's logical timestamp (epoch ms). Lands on node `observed_at`
    /// and bundle `exported_at_ms`; set by the release, never read off the clock.
    pub source_date_ms: u64,
    pub builder: String,
    pub spec_sha256: String,
    /// sha256 of the `skills.lock` copy shipped beside the manifest, if any.
    #[serde(default)]
    pub skills_lock_sha256: Option<String>,
    pub sources: Vec<SourceEntry>,
    pub tenants: Vec<TenantEntry>,
    /// sha256 over the canonical JSON of every field above (with this field
    /// empty). The importer recomputes it; it is also the value the app stores to
    /// know a corpus was imported.
    pub bundle_digest: String,
}

impl Manifest {
    /// Compute the digest over every other field.
    pub fn compute_digest(&self) -> Result<String, CorpusError> {
        let mut m = self.clone();
        m.bundle_digest = String::new();
        let bytes = serde_json::to_vec(&m).map_err(|e| CorpusError::Serde(e.to_string()))?;
        Ok(sha256_hex(&bytes))
    }

    /// Pretty JSON with a trailing newline (stable: struct field order + sorted vecs).
    pub fn to_json(&self) -> Result<String, CorpusError> {
        let mut s =
            serde_json::to_string_pretty(self).map_err(|e| CorpusError::Serde(e.to_string()))?;
        s.push('\n');
        Ok(s)
    }

    pub fn from_json(text: &str) -> Result<Self, CorpusError> {
        serde_json::from_str(text).map_err(|e| CorpusError::Verify(format!("manifest: {e}")))
    }

    /// The NOTICE that ships with the corpus: one section per included source,
    /// with licence, upstream, commit, attribution, and what was changed.
    pub fn notice(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# {} knowledge corpus: notices\n\n", self.name));
        s.push_str(
            "This corpus is a derived work: each source below was split into text chunks for a local \
             memory graph. Executable files are never included. Skills ship only the files the \
             skills.lock pins.\n\n",
        );
        for src in self.sources.iter().filter(|s| s.included) {
            s.push_str(&format!("## {}\n\n", src.id));
            s.push_str(&format!("- Upstream: {}\n", src.upstream));
            s.push_str(&format!("- Commit: {}\n", src.commit));
            s.push_str(&format!("- Licence: {}\n", src.license));
            if let Some(a) = &src.attribution {
                s.push_str(&format!("- Attribution: {a}\n"));
            }
            s.push_str(&format!(
                "- Changes: chunked into memory nodes; {} file(s) included, {} left out (listed in manifest.json).\n\n",
                src.files.len(),
                src.skipped.len()
            ));
        }
        let excluded: Vec<_> = self.sources.iter().filter(|s| !s.included).collect();
        if !excluded.is_empty() {
            s.push_str("## Not included\n\n");
            for src in excluded {
                s.push_str(&format!(
                    "- {}: {}\n",
                    src.id,
                    src.excluded_reason.as_deref().unwrap_or("excluded")
                ));
            }
        }
        s
    }
}
