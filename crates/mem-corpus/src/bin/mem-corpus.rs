//! `mem-corpus`: build and check a release-time knowledge corpus (HUP-S3.1).
//!
//! ```text
//! mem-corpus build  --spec <spec.toml> --sources-base <dir> --out <dir> --source-date-ms <ms>
//! mem-corpus verify <corpus-dir>
//! ```
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

const USAGE: &str = "usage:\n  mem-corpus build --spec <spec.toml> --sources-base <dir> --out <dir> --source-date-ms <ms>\n  mem-corpus verify <corpus-dir>";

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
            let built = build_corpus(&text, &opts).map_err(|e| e.to_string())?;
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
