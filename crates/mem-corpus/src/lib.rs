//! `mem-corpus`: the release-time knowledge corpus (HUP-S3.1).
//!
//! **Build** (release time, on a build host that has the source checkouts):
//! a [`spec::CorpusSpec`] names the sources (public-tier Citrate docs, the
//! Gradient Papers, Agentile, reviewed third-party skills, Solidity references).
//! [`build::build_corpus`] reads them through a symlink-refusing walker, chunks
//! them, and emits one deterministic Derived-plane [`mem_sync::SyncBundle`] per
//! knowledge tenant plus a [`manifest::Manifest`] (sources, commits, licences,
//! per-file sha256, per-tenant bundle sha256, and a digest over all of it), a
//! copy of the `skills.lock` the skills were checked against, and a NOTICE.
//! Same inputs, same bytes: nothing in the output depends on the clock, the
//! host, or directory iteration order.
//!
//! **Import** (first run, on the member's machine): [`import::verify_corpus`]
//! re-hashes every tenant file against the manifest and checks the bundle's
//! shape (knowledge tenants only, Derived plane only, no retracting edges,
//! no edge leaving its bundle). [`import::import_corpus`] then embeds each node
//! with the store's own embedder and merges it, reporting progress. It records
//! each tenant's bundle hash in the store, so a second run is a no-op.
//!
//! The Derived-plane rule holds: node identity is a pure function of the source
//! bytes. Embeddings are added at import time and are not part of identity.

pub mod build;
pub mod chunk;
pub mod import;
pub mod lock;
pub mod manifest;
pub mod progress;
pub mod spec;
pub mod walk;

/// The tenants a knowledge corpus may write. Runtime tenants (`personal`,
/// `chain-state`, `federation`, …) are never writable by a corpus bundle.
pub const KNOWLEDGE_TENANTS: &[&str] = &["citrate-docs", "skills", "refs", "methodology"];

/// Manifest format tag.
pub const FORMAT: &str = "citrate-corpus/1";

/// Manifest file name inside a corpus directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// Copy of the skills lock inside a corpus directory.
pub const SKILLS_LOCK_FILE: &str = "skills.lock";

/// Attribution notice inside a corpus directory.
pub const NOTICE_FILE: &str = "NOTICE.md";

/// Directory holding the per-tenant bundles.
pub const TENANTS_DIR: &str = "tenants";

#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("corpus spec: {0}")]
    Spec(String),
    #[error("skills.lock: {0}")]
    Lock(String),
    #[error("source {source_id}: {msg}")]
    Source { source_id: String, msg: String },
    #[error("io: {0}")]
    Io(String),
    #[error("serialization: {0}")]
    Serde(String),
    /// The corpus on disk does not match its manifest, or breaks a bundle rule.
    /// Nothing is imported when this is returned.
    #[error("corpus verification failed: {0}")]
    Verify(String),
    #[error("store: {0}")]
    Store(String),
    #[error("embedding: {0}")]
    Embed(String),
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
