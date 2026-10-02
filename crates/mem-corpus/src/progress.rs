//! The JSON-lines progress contract for a first-run import.
//!
//! `mem-mcp import-corpus <store> <corpus-dir>` writes one JSON object per line
//! to stdout. citrate-core reads these lines to drive its progress UI, so the
//! shapes below are a cross-repo contract; add fields, never rename them.
//!
//! ```text
//! {"event":"verified","bundle_digest":"…","tenants":4,"nodes":19,"edges":13}
//! {"event":"tenant_start","tenant":"citrate-docs","nodes":9,"edges":6}
//! {"event":"progress","tenant":"citrate-docs","done":9,"total":9}
//! {"event":"tenant_skipped","tenant":"skills","reason":"already-imported"}
//! {"event":"tenant_done","tenant":"citrate-docs"}
//! {"event":"done","bundle_digest":"…","embed_model":"…","nodes_added":19,"nodes_merged":0,"edges_added":13,"tenants_imported":[…],"tenants_skipped":[…]}
//! {"event":"error","stage":"verify"|"import","message":"…"}
//! ```
//!
//! An `error` line is always the last line, and nothing was imported for any
//! tenant that has no `tenant_done` line.

use std::io::Write;
use std::path::Path;

use mem_core::MemoryNode;
use mem_index::Embedder;
use mem_store::MemoryDagStore;
use serde_json::{json, Value};

use crate::import::{import_corpus, verify_corpus, ImportEvent, ImportReport};
use crate::CorpusError;

fn emit(out: &mut dyn Write, v: Value) {
    // A closed stdout (the parent went away) must not abort the import itself;
    // the store write is the part that matters.
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

/// Verify `dir` and import it into `store`, writing the JSON-lines contract to `out`.
pub fn import_dir_with_progress(
    store: &MemoryDagStore<MemoryNode>,
    dir: &Path,
    embedder: &dyn Embedder,
    out: &mut dyn Write,
) -> Result<ImportReport, CorpusError> {
    let corpus = match verify_corpus(dir) {
        Ok(c) => c,
        Err(e) => {
            emit(
                out,
                json!({ "event": "error", "stage": "verify", "message": e.to_string() }),
            );
            return Err(e);
        }
    };
    emit(
        out,
        json!({
            "event": "verified",
            "bundle_digest": corpus.manifest.bundle_digest,
            "tenants": corpus.tenants.len(),
            "nodes": corpus.tenants.iter().map(|(t, _)| t.nodes).sum::<usize>(),
            "edges": corpus.tenants.iter().map(|(t, _)| t.edges).sum::<usize>(),
        }),
    );
    let result = import_corpus(store, &corpus, embedder, |ev| {
        let v = match ev {
            ImportEvent::TenantStart {
                tenant,
                nodes,
                edges,
            } => {
                json!({ "event": "tenant_start", "tenant": tenant, "nodes": nodes, "edges": edges })
            }
            ImportEvent::Progress {
                tenant,
                done,
                total,
            } => {
                json!({ "event": "progress", "tenant": tenant, "done": done, "total": total })
            }
            ImportEvent::TenantSkipped { tenant, reason } => {
                json!({ "event": "tenant_skipped", "tenant": tenant, "reason": reason })
            }
            ImportEvent::TenantDone { tenant } => {
                json!({ "event": "tenant_done", "tenant": tenant })
            }
        };
        emit(out, v);
    });
    match result {
        Ok(r) => {
            emit(
                out,
                json!({
                    "event": "done",
                    "bundle_digest": r.bundle_digest,
                    "embed_model": r.embed_model,
                    "nodes_added": r.nodes_added,
                    "nodes_merged": r.nodes_merged,
                    "edges_added": r.edges_added,
                    "tenants_imported": r.tenants_imported,
                    "tenants_skipped": r.tenants_skipped,
                }),
            );
            Ok(r)
        }
        Err(e) => {
            emit(
                out,
                json!({ "event": "error", "stage": "import", "message": e.to_string() }),
            );
            Err(e)
        }
    }
}
