//! First-run import: verify a corpus directory against its manifest, then embed
//! and merge each tenant bundle into the local store, idempotently.
//!
//! **Why the trusted merge is right here.** A corpus is release content: it
//! ships inside the signed app bundle beside its manifest, and nothing is
//! imported until every tenant file re-hashes to the manifest and the manifest
//! re-hashes to its own digest. On top of that, [`verify_corpus`] narrows what a
//! corpus can say at all: knowledge tenants only (never `personal` or
//! `chain-state`), Derived plane only, unsigned deterministic nodes only, and
//! only `DerivedFrom` / `References` edges between nodes of the same bundle, so
//! a corpus can add knowledge but can never retire, contradict or re-point
//! anything already in the store. Within those rules the merge goes through
//! [`mem_sync::merge_bundle_trusted`], the in-process path for deterministic
//! ingest.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use mem_core::{ContentHash, EdgeKind, MemoryNode, Plane, Status, TrustTier};
use mem_index::Embedder;
use mem_store::MemoryDagStore;
use mem_sync::SyncBundle;

use crate::manifest::{Manifest, TenantEntry};
use crate::{
    sha256_hex, CorpusError, FORMAT, KNOWLEDGE_TENANTS, MANIFEST_FILE, SKILLS_LOCK_FILE,
    TENANTS_DIR,
};

/// Largest manifest the importer will read.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
/// Largest single tenant bundle the importer will read.
pub const MAX_BUNDLE_BYTES: u64 = 1024 * 1024 * 1024;
/// Nodes embedded and merged per progress step.
pub const IMPORT_BATCH: usize = 128;

/// Store meta key recording the bundle hash last imported for a tenant.
pub fn tenant_meta_key(tenant: &str) -> Vec<u8> {
    format!("corpus_tenant_sha256:{tenant}").into_bytes()
}

/// A corpus whose files match its manifest and whose bundles obey the rules.
#[derive(Debug, Clone)]
pub struct VerifiedCorpus {
    pub manifest: Manifest,
    pub tenants: Vec<(TenantEntry, SyncBundle)>,
}

fn verr(msg: impl Into<String>) -> CorpusError {
    CorpusError::Verify(msg.into())
}

/// Read a regular file (never through a symlink) up to `cap` bytes.
fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>, CorpusError> {
    let meta =
        std::fs::symlink_metadata(path).map_err(|e| verr(format!("{}: {e}", path.display())))?;
    if !meta.file_type().is_file() {
        return Err(verr(format!("{} is not a regular file", path.display())));
    }
    if meta.len() > cap {
        return Err(verr(format!("{} exceeds {cap} bytes", path.display())));
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(cap + 1).read_to_end(&mut buf))
        .map_err(|e| verr(format!("{}: {e}", path.display())))?;
    if buf.len() as u64 > cap {
        return Err(verr(format!("{} exceeds {cap} bytes", path.display())));
    }
    Ok(buf)
}

/// Verify `dir` against its manifest. Nothing is written anywhere.
pub fn verify_corpus(dir: &Path) -> Result<VerifiedCorpus, CorpusError> {
    if !std::fs::symlink_metadata(dir)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
    {
        return Err(verr(format!("{} is not a corpus directory", dir.display())));
    }
    let raw = read_capped(&dir.join(MANIFEST_FILE), MAX_MANIFEST_BYTES)?;
    let text = std::str::from_utf8(&raw).map_err(|_| verr("manifest is not UTF-8"))?;
    let manifest = Manifest::from_json(text)?;
    if manifest.format != FORMAT {
        return Err(verr(format!(
            "unsupported corpus format {:?}",
            manifest.format
        )));
    }
    let digest = manifest.compute_digest()?;
    if digest != manifest.bundle_digest {
        return Err(verr("manifest digest does not match its contents"));
    }
    if let Some(want) = &manifest.skills_lock_sha256 {
        let lock = read_capped(&dir.join(SKILLS_LOCK_FILE), MAX_MANIFEST_BYTES)?;
        if &sha256_hex(&lock) != want {
            return Err(verr("skills.lock does not match the manifest"));
        }
    }

    let mut seen = BTreeSet::new();
    let mut tenants = Vec::with_capacity(manifest.tenants.len());
    for t in &manifest.tenants {
        if !KNOWLEDGE_TENANTS.contains(&t.tenant.as_str()) {
            return Err(verr(format!(
                "tenant {:?} is not a knowledge tenant",
                t.tenant
            )));
        }
        if !seen.insert(t.tenant.clone()) {
            return Err(verr(format!("tenant {:?} listed twice", t.tenant)));
        }
        let expected_file = format!("{TENANTS_DIR}/{}.syncbundle.json", t.tenant);
        if t.file != expected_file {
            return Err(verr(format!(
                "tenant {:?} file must be {expected_file}",
                t.tenant
            )));
        }
        let bytes = read_capped(&dir.join(&t.file), MAX_BUNDLE_BYTES)?;
        if sha256_hex(&bytes) != t.sha256 {
            return Err(verr(format!("{} does not match its manifest hash", t.file)));
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| verr(format!("{} is not UTF-8", t.file)))?;
        let bundle =
            SyncBundle::from_json(text.trim_end()).map_err(|e| verr(format!("{}: {e}", t.file)))?;
        check_bundle(t, &bundle)?;
        tenants.push((t.clone(), bundle));
    }
    Ok(VerifiedCorpus { manifest, tenants })
}

/// The shape rules a corpus bundle must obey (see the module docs).
fn check_bundle(t: &TenantEntry, b: &SyncBundle) -> Result<(), CorpusError> {
    if b.repo != t.tenant {
        return Err(verr(format!(
            "bundle repo {:?} is not tenant {:?}",
            b.repo, t.tenant
        )));
    }
    if b.nodes.len() != t.nodes || b.edges.len() != t.edges {
        return Err(verr(format!(
            "tenant {:?} counts do not match the manifest",
            t.tenant
        )));
    }
    let mut ids = BTreeSet::new();
    for n in &b.nodes {
        let ok = n.repo == t.tenant
            && n.plane == Plane::Derived
            && n.trust_tier == TrustTier::DerivedDeterministic
            && n.signature.is_none()
            && n.status == Status::Active
            && n.valid_to.is_none()
            && n.author.starts_with("corpus:");
        if !ok {
            return Err(verr(format!(
                "tenant {:?}: node {} is not a plain corpus node",
                t.tenant,
                &n.compute_id().to_hex()[..12]
            )));
        }
        ids.insert(n.compute_id());
    }
    for e in &b.edges {
        let kind_ok = matches!(e.kind, EdgeKind::DerivedFrom | EdgeKind::References);
        let ok = kind_ok
            && e.plane == Plane::Derived
            && !e.quarantined
            && e.signature.is_none()
            && ids.contains(&e.from)
            && ids.contains(&e.to);
        if !ok {
            return Err(verr(format!(
                "tenant {:?}: edge {:?} {}->{} is not allowed in a corpus",
                t.tenant,
                e.kind,
                &e.from.to_hex()[..10],
                &e.to.to_hex()[..10]
            )));
        }
    }
    Ok(())
}

/// Progress reported while importing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportEvent {
    /// A tenant's import is starting.
    TenantStart {
        tenant: String,
        nodes: usize,
        edges: usize,
    },
    /// Nodes embedded and merged so far for this tenant.
    Progress {
        tenant: String,
        done: usize,
        total: usize,
    },
    /// The tenant's bundle was already imported (same hash): nothing written.
    TenantSkipped { tenant: String, reason: String },
    /// The tenant finished.
    TenantDone { tenant: String },
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub bundle_digest: String,
    pub embed_model: String,
    pub tenants_imported: Vec<String>,
    pub tenants_skipped: Vec<String>,
    pub nodes_added: usize,
    pub nodes_merged: usize,
    pub edges_added: usize,
}

/// Embed and merge a verified corpus into `store`.
///
/// Idempotent: a tenant whose bundle hash is already recorded in the store is
/// skipped; re-merging is a CRDT no-op anyway. The tenant hash is recorded only
/// after the whole tenant landed, so an interrupted import resumes on the next
/// run instead of being marked done.
pub fn import_corpus(
    store: &MemoryDagStore<MemoryNode>,
    corpus: &VerifiedCorpus,
    embedder: &dyn Embedder,
    mut on_event: impl FnMut(ImportEvent),
) -> Result<ImportReport, CorpusError> {
    let store_err = |e: mem_store::StoreError| CorpusError::Store(e.to_string());
    let sync_err = |e: mem_sync::SyncError| CorpusError::Store(e.to_string());
    let mut report = ImportReport {
        bundle_digest: corpus.manifest.bundle_digest.clone(),
        embed_model: embedder.model_id().to_string(),
        ..ImportReport::default()
    };
    for (entry, bundle) in &corpus.tenants {
        let key = tenant_meta_key(&entry.tenant);
        let done_hash = store.get_meta(&key).map_err(store_err)?;
        if done_hash.as_deref() == Some(entry.sha256.as_bytes()) {
            on_event(ImportEvent::TenantSkipped {
                tenant: entry.tenant.clone(),
                reason: "already-imported".into(),
            });
            report.tenants_skipped.push(entry.tenant.clone());
            continue;
        }
        on_event(ImportEvent::TenantStart {
            tenant: entry.tenant.clone(),
            nodes: bundle.nodes.len(),
            edges: bundle.edges.len(),
        });
        let total = bundle.nodes.len();
        let mut done = 0usize;
        for batch in bundle.nodes.chunks(IMPORT_BATCH) {
            let mut nodes = Vec::with_capacity(batch.len());
            for n in batch {
                let mut n = n.clone();
                let text = String::from_utf8_lossy(&n.content).into_owned();
                n.embedding = Some(
                    embedder
                        .embed(&text)
                        .map_err(|e| CorpusError::Embed(e.to_string()))?,
                );
                nodes.push(n);
            }
            let part = SyncBundle {
                repo: bundle.repo.clone(),
                exported_at_ms: bundle.exported_at_ms,
                nodes,
                edges: vec![],
            };
            let out = mem_sync::merge_bundle_trusted(store, &part).map_err(sync_err)?;
            report.nodes_added += out.nodes_added;
            report.nodes_merged += out.nodes_merged;
            done += batch.len();
            on_event(ImportEvent::Progress {
                tenant: entry.tenant.clone(),
                done,
                total,
            });
        }
        if !bundle.edges.is_empty() {
            let part = SyncBundle {
                repo: bundle.repo.clone(),
                exported_at_ms: bundle.exported_at_ms,
                nodes: vec![],
                edges: bundle.edges.clone(),
            };
            let out = mem_sync::merge_bundle_trusted(store, &part).map_err(sync_err)?;
            report.edges_added += out.edges_added;
        }
        store
            .put_meta(&entry.tenant, &key, entry.sha256.as_bytes())
            .map_err(store_err)?;
        report.tenants_imported.push(entry.tenant.clone());
        on_event(ImportEvent::TenantDone {
            tenant: entry.tenant.clone(),
        });
    }
    Ok(report)
}

/// Count of corpus nodes per tenant currently in `store` (for reports/tests).
pub fn tenant_node_counts(
    store: &MemoryDagStore<MemoryNode>,
) -> Result<BTreeMap<String, usize>, CorpusError> {
    let mut out = BTreeMap::new();
    for n in store
        .all_nodes()
        .map_err(|e| CorpusError::Store(e.to_string()))?
    {
        *out.entry(n.repo).or_insert(0) += 1;
    }
    Ok(out)
}

/// Ids of a bundle's nodes (for tests).
pub fn node_ids(b: &SyncBundle) -> BTreeSet<ContentHash> {
    b.nodes.iter().map(|n| n.compute_id()).collect()
}
