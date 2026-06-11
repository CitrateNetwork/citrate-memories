//! Federation meta-graph (MEM-S4 WP-4.4, planset D4.4).
//!
//! The graph above the graphs: one [`NodeKind::Tenant`] node per repo declared
//! in the federation `manifest.toml`, connected by deterministic `DependsOn`
//! edges from the manifest's `[[drift]]` cross-repo dependency map. Everything
//! lives in the reserved **`federation`** tenant, so the whole existing surface
//! works on it for free: `memory.recall federation` = the federation overview,
//! `memory.search` finds "which repo handles identity", `memory.neighbors` of a
//! tenant node = its dependency blast-radius, and authz/crypto-shred apply per
//! the usual tenant rules.
//!
//! Derived plane: rebuilt deterministically from the manifest (Rule 9 — the
//! manifest stays canonical; this is a projection). Tenant-node ids depend on
//! name+summary content only, so re-ingest after a manifest edit supersedes
//! nothing and dedupes everything unchanged.
//!
//! The manifest parser is deliberately dependency-free (like `frontmatter`):
//! it reads exactly the three shapes this projection needs — `[repos.<name>]`
//! table headers, `key = "value"` pairs, and `[[drift]]` entries — and ignores
//! everything else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mem_core::{
    BelnapValue, Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind, Plane,
    SourceRef, Status, TrustTier, SCHEMA_VERSION,
};
use mem_index::Embedder;
use mem_store::MemoryDagStore;

use crate::{IngestError, Watermark};

/// The reserved tenant the meta-graph lives in.
pub const FEDERATION_TENANT: &str = "federation";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestRepo {
    pub name: String,
    pub tier: String,
    pub role: String,
    pub visibility: String,
    /// Free-text description from the manifest — the searchable meat of the
    /// tenant summary.
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DriftDep {
    pub consumer: String,
    pub dep_repo: String,
}

/// What `[section]` the line cursor is inside.
enum Section {
    Repo(String),
    Drift,
    Other,
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Parse the federation manifest's repo declarations and `[[drift]]`
/// dependency map. Unknown sections/keys are ignored; a repo re-declared later
/// merges over the earlier entry (last writer wins per field).
pub fn parse_manifest(toml: &str) -> (Vec<ManifestRepo>, Vec<DriftDep>) {
    let mut repos: BTreeMap<String, ManifestRepo> = BTreeMap::new();
    let mut deps: BTreeSet<DriftDep> = BTreeSet::new();
    let mut section = Section::Other;
    // The drift entry under construction (consumer, dep_repo seen so far).
    let mut drift: (Option<String>, Option<String>) = (None, None);

    let flush_drift = |d: &mut (Option<String>, Option<String>), deps: &mut BTreeSet<DriftDep>| {
        if let (Some(consumer), Some(dep_repo)) = (d.0.take(), d.1.take()) {
            deps.insert(DriftDep { consumer, dep_repo });
        }
        *d = (None, None);
    };

    for raw in toml.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            flush_drift(&mut drift, &mut deps);
            section = if line == "[[drift]]" {
                Section::Drift
            } else if let Some(rest) = line.strip_prefix("[repos.") {
                let name = unquote(rest.trim_end_matches(']'));
                repos.entry(name.clone()).or_insert_with(|| ManifestRepo { name: name.clone(), ..Default::default() });
                Section::Repo(name)
            } else {
                Section::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), unquote(value));
        match &section {
            Section::Repo(name) => {
                // `name` was inserted at the section header, so the entry exists.
                if let Some(r) = repos.get_mut(name) {
                    match key {
                        "tier" => r.tier = value,
                        "role" => r.role = value,
                        "visibility" => r.visibility = value,
                        "notes" => r.notes = value,
                        _ => {}
                    }
                }
            }
            Section::Drift => match key {
                "consumer" => drift.0 = Some(value),
                "dep_repo" => drift.1 = Some(value),
                _ => {}
            },
            Section::Other => {}
        }
    }
    flush_drift(&mut drift, &mut deps);
    (repos.into_values().collect(), deps.into_iter().collect())
}

fn tenant_node(r: &ManifestRepo, now_ms: u64, embedder: &dyn Embedder) -> Result<MemoryNode, IngestError> {
    let mut content = format!(
        "{} — role: {}, tier: {}, visibility: {}",
        r.name,
        if r.role.is_empty() { "?" } else { &r.role },
        if r.tier.is_empty() { "?" } else { &r.tier },
        if r.visibility.is_empty() { "?" } else { &r.visibility },
    );
    if !r.notes.is_empty() {
        content.push_str(". ");
        content.push_str(&r.notes);
    }
    let embedding = Some(embedder.embed(&content)?);
    Ok(MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind: NodeKind::Tenant,
        repo: FEDERATION_TENANT.into(),
        author: "ingest".into(),
        source_ref: SourceRef::DagNative { key: format!("tenant:{}", r.name) },
        content: content.into_bytes(),
        valid_from: now_ms,
        valid_to: None,
        observed_at: now_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding,
        confidence: vec![BelnapValue::True],
        anchors: vec![],
        status: Status::Active,
    })
}

fn depends_on(from: &MemoryNode, to: &MemoryNode, now_ms: u64) -> Edge {
    Edge {
        from: from.compute_id(),
        to: to.compute_id(),
        kind: EdgeKind::DependsOn,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance {
            method: EdgeMethod::Ingest,
            asserter: "ingest".into(),
            at: now_ms,
            evidence: Some("manifest [[drift]]".into()),
        },
        confidence: vec![BelnapValue::True],
        quarantined: false,
        signature: None,
    }
}

#[derive(Debug, Clone)]
pub struct MetaReport {
    pub tenants: usize,
    pub depends_on: usize,
    /// `[[drift]]` entries whose consumer or dep is not a declared repo —
    /// deterministic property of the manifest, counted, never fatal.
    pub skipped_deps: usize,
    pub watermark: Watermark,
}

/// Build + commit the meta-graph from manifest text. Idempotent (content-
/// addressed nodes/edges dedupe) and atomic (one batch). The federation
/// tenant's watermark records the manifest content hash as its "head".
pub fn ingest_federation_meta_str(
    manifest: &str,
    store: &MemoryDagStore<MemoryNode>,
    embedder: &dyn Embedder,
    now_ms: u64,
) -> Result<MetaReport, IngestError> {
    let (repos, deps) = parse_manifest(manifest);
    let mut nodes: BTreeMap<String, MemoryNode> = BTreeMap::new();
    for r in &repos {
        nodes.insert(r.name.clone(), tenant_node(r, now_ms, embedder)?);
    }
    let mut edges = Vec::new();
    let mut skipped = 0usize;
    for d in &deps {
        match (nodes.get(&d.consumer), nodes.get(&d.dep_repo)) {
            (Some(from), Some(to)) => edges.push(depends_on(from, to, now_ms)),
            _ => skipped += 1,
        }
    }
    let node_vec: Vec<MemoryNode> = nodes.into_values().collect();
    store.commit(&node_vec, &edges)?;

    let watermark = Watermark {
        repo: FEDERATION_TENANT.into(),
        head: Some(blake3::hash(manifest.as_bytes()).to_hex().to_string()),
        head_count: node_vec.len(),
        ingested_at_ms: now_ms,
    };
    let bytes = serde_json::to_vec(&watermark).map_err(|e| IngestError::Serde(e.to_string()))?;
    store.put_meta(&crate::watermark_key(FEDERATION_TENANT), &bytes)?;

    Ok(MetaReport { tenants: node_vec.len(), depends_on: edges.len(), skipped_deps: skipped, watermark })
}

/// File wrapper around [`ingest_federation_meta_str`].
pub fn ingest_federation_meta(
    manifest_path: &Path,
    store: &MemoryDagStore<MemoryNode>,
    embedder: &dyn Embedder,
    now_ms: u64,
) -> Result<MetaReport, IngestError> {
    let manifest = std::fs::read_to_string(manifest_path)
        .map_err(|e| IngestError::Git(format!("read {}: {e}", manifest_path.display())))?;
    ingest_federation_meta_str(&manifest, store, embedder, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_index::HashingEmbedder;
    use mem_store::kv::InMemoryKv;

    const FIXTURE: &str = r#"
# Federation manifest (fixture)
schema = 1

[repos.citrate-chain]
tier = "0"
role = "l1-blockchain"
visibility = "public"
notes = "GHOSTDAG consensus, EVM-compatible execution"

[repos."citrate-identity"]
tier = "1"
role = "identity-authority"
visibility = "private"

[repos.citrate-explorer]
role = "block-explorer"

[routing]
default = "x"

[[drift]]
consumer = "citrate-explorer"
dep_repo = "citrate-chain"
file = "Cargo.toml"

[[drift]]
consumer = "citrate-identity"
dep_repo = "citrate-chain"

[[drift]]
consumer = "citrate-ghost"
dep_repo = "citrate-chain"
"#;

    #[test]
    fn parses_repos_and_drift_deps() {
        let (repos, deps) = parse_manifest(FIXTURE);
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["citrate-chain", "citrate-explorer", "citrate-identity"]);
        let chain = repos.iter().find(|r| r.name == "citrate-chain").unwrap();
        assert_eq!((chain.tier.as_str(), chain.role.as_str()), ("0", "l1-blockchain"));
        assert!(chain.notes.contains("GHOSTDAG"), "notes parsed into the summary");
        let ident = repos.iter().find(|r| r.name == "citrate-identity").unwrap();
        assert_eq!(ident.role, "identity-authority", "quoted table names parse");
        assert_eq!(deps.len(), 3);
        assert!(deps.contains(&DriftDep { consumer: "citrate-explorer".into(), dep_repo: "citrate-chain".into() }));
    }

    #[test]
    fn meta_graph_is_deterministic_idempotent_and_searchable() {
        let store = MemoryDagStore::<MemoryNode>::new(Box::new(InMemoryKv::new()));
        let e = HashingEmbedder::new(64);

        let r1 = ingest_federation_meta_str(FIXTURE, &store, &e, 100).unwrap();
        assert_eq!(r1.tenants, 3);
        assert_eq!(r1.depends_on, 2, "both endpoints must be declared repos");
        assert_eq!(r1.skipped_deps, 1, "citrate-ghost is undeclared");
        assert_eq!(r1.watermark.repo, FEDERATION_TENANT);

        // Re-ingest at a different time: ids exclude timestamps → idempotent.
        let r2 = ingest_federation_meta_str(FIXTURE, &store, &e, 999).unwrap();
        assert_eq!(store.node_count().unwrap(), 3);
        assert_eq!(store.edge_count().unwrap(), 2);
        assert_eq!(r2.watermark.head, r1.watermark.head, "manifest hash is the head");

        // Tenant nodes live in the reserved tenant, embedded and typed.
        let nodes = store.all_nodes().unwrap();
        assert!(nodes.iter().all(|n| n.repo == FEDERATION_TENANT && n.kind == NodeKind::Tenant));
        assert!(nodes.iter().all(|n| n.embedding.is_some()), "summaries are searchable");
        let chain = nodes.iter().find(|n| String::from_utf8_lossy(&n.content).contains("l1-blockchain")).unwrap();
        assert!(String::from_utf8_lossy(&chain.content).contains("GHOSTDAG"), "notes enrich the summary");

        // The dependency edges point consumer -> dep.
        let ident = nodes.iter().find(|n| String::from_utf8_lossy(&n.content).contains("identity-authority")).unwrap();
        let out = store.out_edges(&ident.compute_id()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, EdgeKind::DependsOn);
        assert!(!out[0].quarantined, "manifest deps are load-bearing Derived edges");
    }
}
