//! `mem-corpus`: build and check a release-time knowledge corpus (HUP-S3.1).
//!
//! ```text
//! mem-corpus build  --spec <spec.toml> --sources-base <dir> --out <dir> --source-date-ms <ms>
//!                   [--embed-bge <bge-model-dir>]
//! mem-corpus verify <corpus-dir>
//! ```
//!
//! `--embed-bge` (a build with `--features transformer`) also embeds every node
//! with the BGE model in `<bge-model-dir>` (`config.json`, `tokenizer.json`,
//! `model.safetensors`; the same pinned files the app bundles) and ships the
//! vectors (`mem_corpus::vectors`), so a member's first-run import does not have
//! to embed the corpus on their CPU.
//!
//! `build` is deterministic: the same spec, sources and `--source-date-ms`
//! produce byte-identical output. The release sets `--source-date-ms` (for
//! example from the release commit's timestamp); it is never read off the clock.
//! First-run import on a member's machine is the `mem-mcp import-corpus`
//! subcommand, so the app ships one memory binary.

use std::path::PathBuf;
use std::process::ExitCode;

use mem_corpus::build::{build_corpus, write_corpus, BuildOptions};
use mem_corpus::import::verify_corpus;

const USAGE: &str = "usage:\n  mem-corpus build --spec <spec.toml> --sources-base <dir> --out <dir> --source-date-ms <ms> [--embed-bge <bge-model-dir>]\n  mem-corpus verify <corpus-dir>";

/// Embed `built` with the BGE model in `dir`, recording the weights' sha256.
#[cfg(feature = "transformer")]
fn embed_bge(built: &mut mem_corpus::build::BuiltCorpus, dir: &str) -> Result<String, String> {
    use mem_index::Embedder as _;
    let weights = std::path::Path::new(dir).join("model.safetensors");
    let bytes = std::fs::read(&weights).map_err(|e| format!("read {}: {e}", weights.display()))?;
    let weights_sha256 = mem_corpus::sha256_hex(&bytes);
    drop(bytes);
    // The embedder reads its files from CITRATE_BGE_MODEL_DIR (the same intake the
    // app's memory daemon uses), so the release vectors come from the pinned files.
    std::env::set_var("CITRATE_BGE_MODEL_DIR", dir);
    let embedder = mem_index::TransformerEmbedder::bge_base().map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let mut last = std::time::Instant::now();
    mem_corpus::build::embed_vectors(built, &embedder, &weights_sha256, |tenant, done, total| {
        if done == total || last.elapsed().as_secs() >= 30 {
            last = std::time::Instant::now();
            eprintln!(
                "  embedding {tenant}: {done}/{total} ({:.0} s)",
                started.elapsed().as_secs_f64()
            );
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(format!(
        "embedded with {} (weights sha256 {weights_sha256}) in {:.0} s\n",
        embedder.model_id(),
        started.elapsed().as_secs_f64()
    ))
}

#[cfg(not(feature = "transformer"))]
fn embed_bge(_built: &mut mem_corpus::build::BuiltCorpus, _dir: &str) -> Result<String, String> {
    Err("--embed-bge needs a mem-corpus built with --features transformer".into())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn run(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("build") => {
            let spec = flag(args, "--spec").ok_or("--spec is required")?;
            let base = flag(args, "--sources-base").ok_or("--sources-base is required")?;
            let out = flag(args, "--out").ok_or("--out is required")?;
            let date: u64 = flag(args, "--source-date-ms")
                .ok_or("--source-date-ms is required (the corpus never reads the clock)")?
                .parse()
                .map_err(|_| "--source-date-ms must be an integer")?;
            let text = std::fs::read_to_string(&spec).map_err(|e| format!("read {spec}: {e}"))?;
            let opts = BuildOptions {
                sources_base: PathBuf::from(base),
                source_date_ms: date,
            };
            let mut built = build_corpus(&text, &opts).map_err(|e| e.to_string())?;
            let embedded = match flag(args, "--embed-bge") {
                Some(dir) => Some(embed_bge(&mut built, &dir)?),
                None => None,
            };
            write_corpus(&built, &PathBuf::from(&out)).map_err(|e| e.to_string())?;
            let mut s = format!(
                "built {} -> {out}\nbundle_digest {}\n",
                built.manifest.name, built.manifest.bundle_digest
            );
            for t in &built.manifest.tenants {
                s.push_str(&format!(
                    "  {:<14} {:>7} nodes {:>7} edges\n",
                    t.tenant, t.nodes, t.edges
                ));
            }
            if let Some(e) = embedded {
                s.push_str(&e);
            }
            for src in built.manifest.sources.iter().filter(|s| !s.included) {
                s.push_str(&format!(
                    "  excluded {}: {}\n",
                    src.id,
                    src.excluded_reason.as_deref().unwrap_or("excluded")
                ));
            }
            Ok(s)
        }
        Some("verify") => {
            let dir = args.get(1).ok_or(USAGE)?;
            let v = verify_corpus(&PathBuf::from(dir)).map_err(|e| e.to_string())?;
            Ok(format!(
                "ok {} ({} tenants)\n",
                v.manifest.bundle_digest,
                v.tenants.len()
            ))
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mem-corpus: {e}");
            ExitCode::FAILURE
        }
    }
}
