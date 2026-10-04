//! Release-time corpus build: spec + sources → per-tenant SyncBundles +
//! manifest + skills.lock copy + NOTICE. Deterministic (see the crate docs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use mem_core::{
    BelnapValue, ContentHash, Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind,
    Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION,
};
use mem_ingest::frontmatter;
use mem_sync::SyncBundle;

use crate::chunk::{chunk_text, title_of};
use crate::lock::{Ship, SkillsLock};
use crate::manifest::{FileEntry, Manifest, SkippedEntry, SourceEntry, TenantEntry};
use crate::spec::{CorpusSpec, SourceKind, SourceSpec};
use crate::walk::{self, Refusal};
use crate::{sha256_hex, CorpusError, FORMAT, KNOWLEDGE_TENANTS};

/// Edge asserter recorded on every corpus edge.
pub const CORPUS_ASSERTER: &str = "corpus";

/// Build options.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// The directory every spec `root` / `skills_lock` is relative to.
    pub sources_base: PathBuf,
    /// The corpus's logical timestamp (epoch ms). Never read from the clock.
    pub source_date_ms: u64,
}

/// One tenant's serialized bundle.
#[derive(Debug, Clone)]
pub struct TenantFile {
    pub tenant: String,
    /// Path relative to the corpus directory.
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// A built corpus, not yet written.
#[derive(Debug, Clone)]
pub struct BuiltCorpus {
    pub manifest: Manifest,
    pub tenants: Vec<TenantFile>,
    /// Precomputed vectors per tenant ([`embed_vectors`]); empty unless embedded.
    pub vector_files: Vec<TenantFile>,
    /// The exact bytes of the skills.lock the build checked against.
    pub skills_lock: Option<Vec<u8>>,
}

#[derive(Default)]
struct TenantGraph {
    nodes: BTreeMap<ContentHash, MemoryNode>,
    edges: BTreeMap<Vec<u8>, Edge>,
}

impl TenantGraph {
    fn add_node(&mut self, n: MemoryNode) -> ContentHash {
        let id = n.compute_id();
        self.nodes.entry(id).or_insert(n);
        id
    }
    fn add_edge(&mut self, e: Edge) {
        self.edges.entry(e.key()).or_insert(e);
    }
}

fn src_err(source: &SourceSpec, msg: impl Into<String>) -> CorpusError {
    CorpusError::Source {
        source_id: source.id.clone(),
        msg: msg.into(),
    }
}

/// Build a corpus from a spec's TOML text.
pub fn build_corpus(spec_text: &str, opts: &BuildOptions) -> Result<BuiltCorpus, CorpusError> {
    let spec = CorpusSpec::from_toml(spec_text)?;

    // One skills.lock per corpus: every skills source must name the same file.
    let lock_paths: std::collections::BTreeSet<&str> = spec
        .sources
        .iter()
        .filter_map(|s| s.skills_lock.as_deref())
        .collect();
    if lock_paths.len() > 1 {
        return Err(CorpusError::Spec(
            "all skills sources must use the same skills_lock".into(),
        ));
    }
    let (lock, lock_bytes) = match lock_paths.iter().next() {
        Some(p) => {
            let bytes = std::fs::read(opts.sources_base.join(p))
                .map_err(|e| CorpusError::Lock(format!("read {p}: {e}")))?;
            let text =
                std::str::from_utf8(&bytes).map_err(|_| CorpusError::Lock("not UTF-8".into()))?;
            (Some(SkillsLock::from_toml(text)?), Some(bytes))
        }
        None => (None, None),
    };

    let mut graphs: BTreeMap<String, TenantGraph> = BTreeMap::new();
    let mut entries = Vec::with_capacity(spec.sources.len());
    for source in &spec.sources {
        let graph = graphs.entry(source.tenant.clone()).or_default();
        entries.push(build_source(&spec, source, lock.as_ref(), opts, graph)?);
    }

    let mut tenants = Vec::new();
    let mut tenant_entries = Vec::new();
    for tenant in KNOWLEDGE_TENANTS {
        let Some(g) = graphs.remove(*tenant) else {
            continue;
        };
        if g.nodes.is_empty() {
            continue;
        }
        let bundle = SyncBundle {
            repo: (*tenant).to_string(),
            exported_at_ms: opts.source_date_ms,
            nodes: g.nodes.into_values().collect(),
            edges: g.edges.into_values().collect(),
        };
        let mut bytes = crate::bundle::encode(&bundle)?.into_bytes();
        bytes.push(b'\n');
        let rel_path = crate::tenant_file(tenant);
        tenant_entries.push(TenantEntry {
            tenant: (*tenant).to_string(),
            file: rel_path.clone(),
            sha256: sha256_hex(&bytes),
            nodes: bundle.nodes.len(),
            edges: bundle.edges.len(),
            vectors: None,
        });
        tenants.push(TenantFile {
            tenant: (*tenant).to_string(),
            rel_path,
            bytes,
        });
    }

    let mut manifest = Manifest {
        format: FORMAT.to_string(),
        name: spec.name.clone(),
        source_date_ms: opts.source_date_ms,
        builder: format!("mem-corpus {}", env!("CARGO_PKG_VERSION")),
        spec_sha256: sha256_hex(spec_text.as_bytes()),
        skills_lock_sha256: lock_bytes.as_deref().map(sha256_hex),
        sources: entries,
        tenants: tenant_entries,
        bundle_digest: String::new(),
    };
    manifest.bundle_digest = manifest.compute_digest()?;
    Ok(BuiltCorpus {
        manifest,
        tenants,
        vector_files: Vec::new(),
        skills_lock: lock_bytes,
    })
}

/// Embed every node of a built corpus with `embedder` and attach the vectors
/// (see [`crate::vectors`]): one `tenants/<tenant>.vectors.f16` per tenant, in
/// the tenant file's node order, recorded in the manifest with the model id,
/// dimension and `weights_sha256` (the sha256 of the model weights file), and
/// the manifest digest recomputed. Node text is embedded exactly as the importer
/// would embed it. `on_progress(tenant, done, total)` reports each node.
pub fn embed_vectors(
    built: &mut BuiltCorpus,
    embedder: &dyn mem_index::Embedder,
    weights_sha256: &str,
    mut on_progress: impl FnMut(&str, usize, usize),
) -> Result<(), CorpusError> {
    let dim = embedder.dim();
    if dim == 0 || dim > crate::vectors::MAX_DIM {
        return Err(CorpusError::Embed(format!(
            "unsupported embedding dimension {dim}"
        )));
    }
    let mut files = Vec::with_capacity(built.tenants.len());
    for t in &built.tenants {
        let text = std::str::from_utf8(&t.bytes).map_err(|e| CorpusError::Serde(e.to_string()))?;
        let bundle = crate::bundle::decode(text.trim_end())?;
        let total = bundle.nodes.len();
        let mut vectors = Vec::with_capacity(total);
        for (i, n) in bundle.nodes.iter().enumerate() {
            let text = String::from_utf8_lossy(&n.content);
            let v = embedder
                .embed(&text)
                .map_err(|e| CorpusError::Embed(e.to_string()))?;
            vectors.push(v.data);
            on_progress(&t.tenant, i + 1, total);
        }
        let bytes = crate::vectors::encode(&vectors, dim)?;
        let entry = built
            .manifest
            .tenants
            .iter_mut()
            .find(|e| e.tenant == t.tenant)
            .ok_or_else(|| {
                CorpusError::Serde(format!("tenant {} not in the manifest", t.tenant))
            })?;
        let rel_path = crate::vectors::vectors_file(&t.tenant);
        entry.vectors = Some(crate::vectors::VectorsEntry {
            file: rel_path.clone(),
            sha256: sha256_hex(&bytes),
            encoding: crate::vectors::ENCODING_F16LE.to_string(),
            model: embedder.model_id().to_string(),
            dim,
            weights_sha256: weights_sha256.to_string(),
        });
        files.push(TenantFile {
            tenant: t.tenant.clone(),
            rel_path,
            bytes,
        });
    }
    built.vector_files = files;
    built.manifest.bundle_digest = built.manifest.compute_digest()?;
    Ok(())
}

/// Write a built corpus into `out_dir`, which must not exist or be empty.
pub fn write_corpus(built: &BuiltCorpus, out_dir: &Path) -> Result<(), CorpusError> {
    let io = |e: std::io::Error| CorpusError::Io(e.to_string());
    if out_dir.exists() && std::fs::read_dir(out_dir).map_err(io)?.next().is_some() {
        return Err(CorpusError::Io(format!(
            "{} is not empty",
            out_dir.display()
        )));
    }
    std::fs::create_dir_all(out_dir.join(crate::TENANTS_DIR)).map_err(io)?;
    for t in built.tenants.iter().chain(&built.vector_files) {
        std::fs::write(out_dir.join(&t.rel_path), &t.bytes).map_err(io)?;
    }
    if let Some(lock) = &built.skills_lock {
        std::fs::write(out_dir.join(crate::SKILLS_LOCK_FILE), lock).map_err(io)?;
    }
    std::fs::write(out_dir.join(crate::NOTICE_FILE), built.manifest.notice()).map_err(io)?;
    std::fs::write(
        out_dir.join(crate::MANIFEST_FILE),
        built.manifest.to_json()?,
    )
    .map_err(io)?;
    Ok(())
}

fn excluded_entry(source: &SourceSpec, commit: String, reason: String) -> SourceEntry {
    // `git` asks the build to resolve a checkout that was never read: nothing to pin.
    let commit = if commit == "git" {
        "unpinned".to_string()
    } else {
        commit
    };
    SourceEntry {
        id: source.id.clone(),
        tenant: source.tenant.clone(),
        repo: source.cite_repo().to_string(),
        repo_path: source.repo_path.clone(),
        upstream: source.upstream.clone(),
        commit,
        license: source.license.clone(),
        licence_cleared: source.licence_cleared,
        licence_note: source.licence_note.clone(),
        attribution: source.attribution.clone(),
        included: false,
        excluded_reason: Some(reason),
        files: vec![],
        skipped: vec![],
    }
}

fn build_source(
    spec: &CorpusSpec,
    source: &SourceSpec,
    lock: Option<&SkillsLock>,
    opts: &BuildOptions,
    graph: &mut TenantGraph,
) -> Result<SourceEntry, CorpusError> {
    let root = opts.sources_base.join(&source.root);
    let lock_source = match (source.kind, lock, source.lock_source.as_deref()) {
        (SourceKind::Skills, Some(l), Some(label)) => Some(
            l.source(label)
                .ok_or_else(|| src_err(source, format!("skills.lock has no source {label:?}")))?,
        ),
        _ => None,
    };
    let pinned = source
        .commit
        .clone()
        .or_else(|| lock_source.map(|l| l.commit.clone()))
        .unwrap_or_else(|| "unpinned".to_string());

    if !source.licence_cleared {
        // Never read a source whose redistribution is not cleared.
        return Ok(excluded_entry(
            source,
            pinned,
            "licence not cleared for redistribution (pending owner sign-off)".into(),
        ));
    }
    if !walk::is_real_dir(&root) {
        if source.optional {
            return Ok(excluded_entry(
                source,
                pinned,
                "source not present on the build host".into(),
            ));
        }
        return Err(src_err(
            source,
            format!(
                "root {} is missing (and the source is not optional)",
                source.root
            ),
        ));
    }
    let commit = if pinned == "git" {
        resolve_git(&root)
    } else {
        pinned
    };

    let mut files = Vec::new();
    let mut skipped = Vec::new();
    match source.kind {
        SourceKind::Docs => build_docs(spec, source, &root, opts, graph, &mut files, &mut skipped)?,
        SourceKind::Skills => {
            let lock =
                lock.ok_or_else(|| src_err(source, "skills source without a skills.lock"))?;
            let label = source.lock_source.as_deref().unwrap_or_default();
            build_skills(
                spec,
                source,
                lock,
                label,
                &root,
                opts,
                graph,
                &mut files,
                &mut skipped,
            )?;
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    // A docs source that yields nothing (an empty or uninitialised checkout) is
    // not "shipped with 0 files". A skills source may legitimately ship nothing
    // when the lock excludes every skill; its skipped list says so.
    if source.kind == SourceKind::Docs && files.is_empty() {
        if source.optional {
            return Ok(excluded_entry(
                source,
                commit,
                "no matching files on the build host".into(),
            ));
        }
        return Err(src_err(
            source,
            format!(
                "no matching files under {} (and the source is not optional)",
                source.root
            ),
        ));
    }
    Ok(SourceEntry {
        id: source.id.clone(),
        tenant: source.tenant.clone(),
        repo: source.cite_repo().to_string(),
        repo_path: source.repo_path.clone(),
        upstream: source.upstream.clone(),
        commit,
        license: source.license.clone(),
        licence_cleared: true,
        licence_note: source.licence_note.clone(),
        attribution: source.attribution.clone(),
        included: true,
        excluded_reason: None,
        files,
        skipped,
    })
}

/// `HEAD` of the work tree holding `root`, suffixed `-dirty` when anything under
/// `root` differs from `HEAD` (including untracked files). `"unpinned"` if `root`
/// is not in a git work tree.
fn resolve_git(root: &Path) -> String {
    let head = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output();
    let Ok(head) = head else {
        return "unpinned".into();
    };
    if !head.status.success() {
        return "unpinned".into();
    }
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let dirty = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--", "."])
        .output()
        .map(|o| !o.status.success() || !o.stdout.is_empty())
        .unwrap_or(true);
    if dirty {
        format!("{sha}-dirty")
    } else {
        sha
    }
}

#[allow(clippy::too_many_arguments)]
fn build_docs(
    spec: &CorpusSpec,
    source: &SourceSpec,
    root: &Path,
    opts: &BuildOptions,
    graph: &mut TenantGraph,
    files: &mut Vec<FileEntry>,
    skipped: &mut Vec<SkippedEntry>,
) -> Result<(), CorpusError> {
    let candidates: Vec<String> = if source.include.is_empty() {
        let (list, walk_skipped) = walk::list_files(root, &source.extensions, &source.exclude_dirs)
            .map_err(|e| src_err(source, e))?;
        skipped.extend(
            walk_skipped
                .into_iter()
                .map(|(path, reason)| SkippedEntry { path, reason }),
        );
        list
    } else {
        let mut l = source.include.clone();
        l.sort();
        l.dedup();
        l
    };
    for rel in candidates {
        let bytes = match walk::read_regular(root, &rel) {
            Ok(b) => b,
            Err(Refusal::Missing) if source.include.is_empty() => continue,
            Err(Refusal::Missing) => {
                return Err(src_err(source, format!("listed file {rel} is missing")));
            }
            Err(r) => {
                skipped.push(SkippedEntry {
                    path: rel,
                    reason: r.to_string(),
                });
                continue;
            }
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            skipped.push(SkippedEntry {
                path: rel,
                reason: "not UTF-8".into(),
            });
            continue;
        };
        let (fm, _) = frontmatter::parse(text);
        if let Some((k, want)) = source
            .require_frontmatter
            .iter()
            .find(|(k, want)| fm.get(*k).map(|v| v.trim().trim_matches('"')) != Some(want.as_str()))
        {
            let got = fm.get(k).map(String::as_str).unwrap_or("(absent)");
            skipped.push(SkippedEntry {
                path: rel,
                reason: format!("frontmatter {k} = {got:?}, required {want:?}"),
            });
            continue;
        }
        let chunks = add_file(spec, source, &rel, text, &bytes, None, opts, graph);
        files.push(FileEntry {
            path: rel,
            sha256: sha256_hex(&bytes),
            bytes: bytes.len() as u64,
            chunks,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_skills(
    spec: &CorpusSpec,
    source: &SourceSpec,
    lock: &SkillsLock,
    label: &str,
    root: &Path,
    opts: &BuildOptions,
    graph: &mut TenantGraph,
    files: &mut Vec<FileEntry>,
    skipped: &mut Vec<SkippedEntry>,
) -> Result<(), CorpusError> {
    let mut skills: Vec<_> = lock.skills.iter().filter(|s| s.source == label).collect();
    skills.sort_by(|a, b| a.path.cmp(&b.path).then(a.name.cmp(&b.name)));
    for skill in skills {
        if skill.ship()? == Ship::No {
            skipped.push(SkippedEntry {
                path: format!("{}/SKILL.md", skill.path),
                reason: format!("skills.lock verdict {:?}", skill.verdict),
            });
            continue;
        }
        walk::check_relative(&skill.path).map_err(|e| src_err(source, e))?;
        let want = skill.skill_md_sha256.as_deref().ok_or_else(|| {
            src_err(
                source,
                format!("skill {} ships without a SKILL.md hash", skill.name),
            )
        })?;
        let skill_md = format!("{}/SKILL.md", skill.path);
        let bytes = read_pinned(source, root, &skill_md, want)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| src_err(source, format!("{skill_md} is not UTF-8")))?;
        let (fm, _) = frontmatter::parse(text);
        let description = fm
            .get("description")
            .map(|d| d.trim().trim_matches('"').to_string())
            .filter(|d| !d.is_empty() && d != ">" && d != "|" && d != ">-" && d != "|-");
        let headline = match description {
            Some(d) => format!("Skill {}: {d}", skill.name),
            None => format!("Skill {}", skill.name),
        };
        let chunks = add_file(
            spec,
            source,
            &skill_md,
            text,
            &bytes,
            Some(&headline),
            opts,
            graph,
        );
        let skill_doc = doc_node(
            source,
            &skill_md,
            &sha256_hex(&bytes),
            &headline,
            opts.source_date_ms,
            &fm,
        );
        let skill_id = skill_doc.compute_id();
        files.push(FileEntry {
            path: skill_md,
            sha256: sha256_hex(&bytes),
            bytes: bytes.len() as u64,
            chunks,
        });

        let mut refs = skill.refs.clone();
        refs.sort_by(|a, b| a.path.cmp(&b.path));
        for r in refs {
            walk::check_relative(&r.path).map_err(|e| src_err(source, e))?;
            let rel = format!("{}/{}", skill.path, r.path);
            let bytes = read_pinned(source, root, &rel, &r.sha256)?;
            let Ok(text) = std::str::from_utf8(&bytes) else {
                skipped.push(SkippedEntry {
                    path: rel,
                    reason: "pinned, but not UTF-8 text".into(),
                });
                continue;
            };
            let chunks = add_file(spec, source, &rel, text, &bytes, None, opts, graph);
            let (rfm, rbody) = frontmatter::parse(text);
            let rtitle = title_of(&rel, rfm.get("title").map(String::as_str), rbody);
            let ref_doc = doc_node(
                source,
                &rel,
                &sha256_hex(&bytes),
                &rtitle,
                opts.source_date_ms,
                &rfm,
            );
            graph.add_edge(corpus_edge(
                skill_id,
                ref_doc.compute_id(),
                EdgeKind::References,
                opts.source_date_ms,
                &rel,
            ));
            files.push(FileEntry {
                path: rel,
                sha256: sha256_hex(&bytes),
                bytes: bytes.len() as u64,
                chunks,
            });
        }
    }
    Ok(())
}

/// Read a file the skills.lock pins and fail the build on any byte of drift.
fn read_pinned(
    source: &SourceSpec,
    root: &Path,
    rel: &str,
    want: &str,
) -> Result<Vec<u8>, CorpusError> {
    let bytes =
        walk::read_regular(root, rel).map_err(|r| src_err(source, format!("{rel}: {r}")))?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(want) {
        return Err(src_err(
            source,
            format!("{rel}: sha256 {got} does not match skills.lock {want} (re-run the skill-intake review)"),
        ));
    }
    Ok(bytes)
}

fn doc_node(
    source: &SourceSpec,
    rel: &str,
    file_sha: &str,
    title: &str,
    source_date_ms: u64,
    fm: &BTreeMap<String, String>,
) -> MemoryNode {
    node(
        source,
        rel,
        file_sha,
        (0, 0),
        title.as_bytes().to_vec(),
        source_date_ms,
        fm,
    )
}

fn node(
    source: &SourceSpec,
    rel: &str,
    file_sha: &str,
    span: (u64, u64),
    content: Vec<u8>,
    source_date_ms: u64,
    fm: &BTreeMap<String, String>,
) -> MemoryNode {
    MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind: NodeKind::Doc,
        repo: source.tenant.clone(),
        author: format!("corpus:{}", source.id),
        // `git_sha` carries the file's sha256: the content hash of the exact
        // bytes chunked, which is what makes the id a pure function of the source.
        // Format 2: a repository-relative citation (`<repo>:<path>`), see `cite`.
        source_ref: SourceRef::Artifact {
            repo: source.cite_repo().to_string(),
            path: crate::cite::repo_relative(&source.repo_path, rel),
            git_sha: file_sha.to_string(),
            byte_start: span.0,
            byte_end: span.1,
        },
        content,
        valid_from: frontmatter::created_ms(fm).unwrap_or(source_date_ms),
        valid_to: None,
        observed_at: source_date_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding: None,
        confidence: vec![BelnapValue::True],
        anchors: vec![],
        status: Status::Active,
    }
}

fn corpus_edge(
    from: ContentHash,
    to: ContentHash,
    kind: EdgeKind,
    at: u64,
    evidence: &str,
) -> Edge {
    Edge {
        from,
        to,
        kind,
        plane: Plane::Derived,
        trust_tier: TrustTier::DerivedDeterministic,
        provenance: EdgeProvenance {
            method: EdgeMethod::Ingest,
            asserter: CORPUS_ASSERTER.to_string(),
            at,
            evidence: Some(evidence.to_string()),
        },
        confidence: vec![BelnapValue::True],
        quarantined: false,
        signature: None,
    }
}

/// Mint the doc node and its chunk nodes for one file; returns the chunk count.
#[allow(clippy::too_many_arguments)]
fn add_file(
    spec: &CorpusSpec,
    source: &SourceSpec,
    rel: &str,
    text: &str,
    bytes: &[u8],
    headline: Option<&str>,
    opts: &BuildOptions,
    graph: &mut TenantGraph,
) -> usize {
    let file_sha = sha256_hex(bytes);
    let (fm, body) = frontmatter::parse(text);
    let body_offset = text.len() - body.len();
    let title = match headline {
        Some(h) => h.to_string(),
        None => title_of(rel, fm.get("title").map(String::as_str), body),
    };
    let doc_id = graph.add_node(doc_node(
        source,
        rel,
        &file_sha,
        &title,
        opts.source_date_ms,
        &fm,
    ));
    let chunks = chunk_text(&title, body, body_offset, spec.max_chunk_chars);
    for c in &chunks {
        let content = format!("{}\n\n{}", c.breadcrumb, c.text).into_bytes();
        let n = node(
            source,
            rel,
            &file_sha,
            (c.byte_start, c.byte_end),
            content,
            opts.source_date_ms,
            &fm,
        );
        let id = graph.add_node(n);
        graph.add_edge(corpus_edge(
            id,
            doc_id,
            EdgeKind::DerivedFrom,
            opts.source_date_ms,
            rel,
        ));
    }
    chunks.len()
}
