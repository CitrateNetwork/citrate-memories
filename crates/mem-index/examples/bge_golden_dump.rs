//! Dump bge-base-en-v1.5 token ids and embeddings for the golden corpus.
//!
//! This writes the fixture that `tests/bge_golden.rs` compares against, and the
//! `bge-parity` CI workflow runs it twice: once at the baseline revision (the
//! candle 0.10.2 lockfile, with this file copied in) and once at the head. It
//! therefore only uses APIs that exist at both revisions: the public
//! `TransformerEmbedder`, plus a `tokenizers::Tokenizer` built from the same
//! `tokenizer.json` with the embedder's truncation (512 tokens).
//!
//! It never downloads weights: `CITRATE_BGE_MODEL_DIR` must point at the pinned
//! bge-base-en-v1.5 directory (`config.json`, `tokenizer.json`,
//! `model.safetensors`), the same layout citrate-core bundles.
//!
//! Usage:
//!   CITRATE_BGE_MODEL_DIR=/path/to/bge-base-en-v1.5 \
//!   cargo run --release -p mem-index --features transformer \
//!     --example bge_golden_dump -- <out.json> [label]
//!
//! Vectors are stored as the little-endian bytes of each f32, hex-encoded, so
//! the file round-trips bit for bit (a decimal rendering could not promise that).

use std::fmt::Write as _;
use std::path::PathBuf;

use mem_index::{Embedder, TransformerEmbedder};
use tokenizers::{Tokenizer, TruncationParams};

/// Mirrors `transformer::MAX_TOKENS`; the golden test checks the embedder's own
/// tokenizer against the ids written here, so a drift between the two fails it.
const MAX_TOKENS: usize = 512;

/// Fixture format tag; bump when the layout changes.
const FORMAT: &str = "bge-golden/1";

/// A paragraph repeated to build the long inputs.
const PARAGRAPH: &str = "The memory DAG stores every assertion as a content-addressed node, \
and each node links to the nodes it supersedes, so an agent can replay how a belief changed \
over time. ";

/// Prefixes of this sentence hit exact sequence lengths (with [CLS] and [SEP]).
const LANES: &str = "the validator finalized the block after two rounds of votes from the \
committee while peers gossiped headers across the network and the explorer indexed every \
transaction with its";

/// The golden corpus: (name, text). Lengths in the names count [CLS] and [SEP].
/// 15/16/17 and 31/32/33 straddle the 16- and 32-lane SIMD reduction widths used
/// by candle's CPU sum kernels, which feed the attention softmax denominators.
fn corpus() -> Vec<(String, String)> {
    let lanes: Vec<&str> = LANES.split(' ').collect();
    let prefix = |words: usize| lanes[..words].join(" ");
    let c: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("whitespace_only", " \t\n ".to_string()),
        ("single_word", "memory".to_string()),
        ("short", "ghostdag blue score finality".to_string()),
        ("seq15", prefix(12)),
        ("seq16", prefix(13)),
        ("seq17", prefix(14)),
        ("seq31", prefix(26)),
        ("seq32", prefix(27)),
        ("seq33", prefix(28)),
        ("paragraph", PARAGRAPH.repeat(2)),
        ("over_512_truncated", PARAGRAPH.repeat(20)),
        ("unicode_accents", "Café naïve résumé, déjà vu: Ångström façade".to_string()),
        ("unicode_cjk", "記憶はブロックチェーンに保存される。区块链上的记忆。".to_string()),
        (
            "unicode_mixed",
            "Память 🚀 ذاكرة \u{200d} zero\u{200b}width Ünïcödé ✓".to_string(),
        ),
        (
            "code",
            "fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> { self.model.forward(&ids)? }"
                .to_string(),
        ),
        (
            "numbers_hex",
            "chainId 40204 block 0x1f2e3d4c5b6a7980 at 2026-11-24T12:00:00Z".to_string(),
        ),
        ("repeated", "reorg ".repeat(40)),
    ];
    c.into_iter().map(|(n, t)| (n.to_string(), t)).collect()
}

fn hex_f32le(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8);
    for x in v {
        for b in x.to_le_bytes() {
            // Writing to a String cannot fail.
            let _ = write!(s, "{b:02x}");
        }
    }
    s
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .ok_or("usage: bge_golden_dump <out.json> [label]")?,
    );
    let label = args.next().unwrap_or_default();

    let dir = std::env::var("CITRATE_BGE_MODEL_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .ok_or("CITRATE_BGE_MODEL_DIR must point at the pinned bge-base-en-v1.5 weights")?;

    let embedder = TransformerEmbedder::bge_base()?;
    let mut tokenizer = Tokenizer::from_file(PathBuf::from(&dir).join("tokenizer.json"))?;
    tokenizer.with_truncation(Some(TruncationParams {
        max_length: MAX_TOKENS,
        ..Default::default()
    }))?;

    let mut entries = Vec::new();
    for (name, text) in corpus() {
        let ids = tokenizer.encode(text.as_str(), true)?.get_ids().to_vec();
        let v = embedder.embed(&text)?;
        if v.data.len() != embedder.dim() {
            return Err(
                format!("{name}: got {} dims, want {}", v.data.len(), embedder.dim()).into(),
            );
        }
        entries.push(serde_json::json!({
            "name": name,
            "text": text,
            "ids": ids,
            "vector_f32le_hex": hex_f32le(&v.data),
        }));
    }

    let doc = serde_json::json!({
        "format": FORMAT,
        "model_id": embedder.model_id(),
        "dim": embedder.dim(),
        "label": label,
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "entries": entries,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&doc)? + "\n")?;
    eprintln!(
        "wrote {} entries to {}",
        doc["entries"].as_array().map_or(0, Vec::len),
        out.display()
    );
    Ok(())
}
