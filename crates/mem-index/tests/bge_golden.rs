//! bge-base-en-v1.5 golden test: token ids and embeddings must match the
//! output of candle 0.10.2, so a dependency upgrade can never silently change
//! the vectors stored in an existing index (that would need a re-embed).
//!
//! The fixture `fixtures/bge_golden.json` was written by the `bge_golden_dump`
//! example at revision 5aeda317 (candle-core 0.10.2, tokenizers 0.22 with onig)
//! on an x86_64 Linux GitHub runner by the bge-parity workflow, not by the
//! upgraded code. Its `label` field records that.
//!
//! Criteria, per corpus entry:
//! * token ids identical to the fixture. Tokenization is discrete, so there is
//!   no tolerance: one changed id is a different input to the model.
//! * cosine(fixture, now) >= 0.99999 and max |fixture[i] - now[i]| <= 1e-5.
//!
//! Why that tolerance: the vectors are unit length with 768 components (typical
//! magnitude around 0.036), and f32 has a 2^-24 (about 6e-8) relative epsilon.
//! A change in summation order inside candle's CPU kernels (SIMD lane width,
//! AVX2 or NEON path, gemm blocking) moves results by a few ulps. Measured for
//! candle 0.10.2 -> 0.11.1 (the vec_sum lane change, which only reaches
//! sequences of 32+ tokens on x86_64 and 16+ on aarch64): max abs diff 1.5e-7
//! on x86_64 and 1.8e-7 on aarch64, 1 - cosine under 1e-12, token ids equal;
//! x86_64 against aarch64 differs by at most 2.6e-7. 1e-5 is about 40 times
//! that, so ulp-level drift passes, while a real numerical change (attention
//! masking, layer norm epsilon, pooling, weights) moves components by orders of
//! magnitude more and fails. Cosine >= 0.99999 keeps the ranking of stored
//! neighbours stable. Do not loosen these numbers to make an upgrade pass: if
//! they fail, stored indexes need a re-embed, and that is a decision for the
//! owner.

use serde_json::Value;

/// Minimum cosine similarity between the fixture vector and the current one.
#[cfg(feature = "transformer")]
const MIN_COSINE: f64 = 0.99999;
/// Maximum absolute difference of any single component.
#[cfg(feature = "transformer")]
const MAX_ABS_DIFF: f32 = 1e-5;
/// bge-base-en-v1.5 hidden size.
const DIM: usize = 768;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/bge_golden.json")).expect("fixture parses")
}

fn entries(doc: &Value) -> &[Value] {
    doc["entries"].as_array().expect("entries array")
}

fn vector(entry: &Value) -> Vec<f32> {
    let hex = entry["vector_f32le_hex"].as_str().expect("vector hex");
    assert_eq!(hex.len() % 8, 0, "{}: hex length", entry["name"]);
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex digit"))
        .collect();
    // The length check above makes the remainder empty.
    let (words, _) = bytes.as_chunks::<4>();
    words.iter().map(|w| f32::from_le_bytes(*w)).collect()
}

fn ids(entry: &Value) -> Vec<u32> {
    entry["ids"]
        .as_array()
        .expect("ids array")
        .iter()
        .map(|v| u32::try_from(v.as_u64().expect("id is an integer")).expect("id fits u32"))
        .collect()
}

/// The fixture itself: provenance, shape, and the coverage the corpus promises.
/// Runs in every build (no weights needed).
#[test]
fn bge_golden_fixture_is_well_formed() {
    let doc = fixture();
    assert_eq!(doc["format"], "bge-golden/1");
    assert_eq!(doc["model_id"], "bge-base-en-v1.5");
    assert_eq!(doc["dim"], DIM);
    let label = doc["label"].as_str().expect("label");
    assert!(
        label.starts_with("candle-core 0.10.2 @ 5aeda317"),
        "fixture must come from candle 0.10.2 at 5aeda317, got {label:?}"
    );

    let es = entries(&doc);
    assert!((12..=20).contains(&es.len()), "{} entries", es.len());
    let mut names: Vec<&str> = es
        .iter()
        .map(|e| e["name"].as_str().expect("name"))
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), es.len(), "entry names are unique");

    let mut lens = Vec::new();
    for e in es {
        let name = &e["name"];
        let ids = ids(e);
        assert_eq!(ids.first(), Some(&101), "{name}: starts with [CLS]");
        assert_eq!(ids.last(), Some(&102), "{name}: ends with [SEP]");
        let v = vector(e);
        assert_eq!(v.len(), DIM, "{name}: dims");
        assert!(v.iter().all(|x| x.is_finite()), "{name}: finite");
        let norm = v
            .iter()
            .map(|x| f64::from(*x) * f64::from(*x))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "{name}: unit norm, got {norm}");
        lens.push((ids.len(), e["text"].as_str().expect("text")));
    }

    // Coverage: empty input, under 16, both sides of the 16- and 32-lane SIMD
    // widths, past 32, and a truncated input longer than 512 tokens.
    let has = |n: usize| lens.iter().any(|(l, _)| *l == n);
    for n in [2, 15, 16, 17, 31, 32, 33, 512] {
        assert!(has(n), "corpus has a {n}-token entry");
    }
    assert!(
        lens.iter().any(|(_, t)| t.is_empty()),
        "corpus has the empty string"
    );
    assert!(
        lens.iter()
            .any(|(l, t)| *l == 512 && t.split_whitespace().count() > 512),
        "the 512-token entry is a truncated longer input"
    );
    assert!(
        lens.iter().any(|(l, _)| (34..512).contains(l)),
        "corpus has a mid-length entry"
    );
    let non_ascii = lens.iter().filter(|(_, t)| !t.is_ascii()).count();
    assert!(non_ascii >= 3, "corpus has unicode entries ({non_ascii})");
}

/// Re-embeds the corpus with the current dependency stack and compares with the
/// candle 0.10.2 fixture. Needs the pinned weights; CI's bge-golden job
/// downloads them, checks the sha256, and runs this with `--ignored`.
#[cfg(feature = "transformer")]
#[test]
#[ignore = "needs CITRATE_BGE_MODEL_DIR with the pinned bge-base-en-v1.5 weights; run by CI's bge-golden job"]
fn bge_embeddings_match_candle_0_10_2_golden() {
    use mem_index::{Embedder, TransformerEmbedder};

    let dir = std::env::var("CITRATE_BGE_MODEL_DIR").unwrap_or_default();
    assert!(
        !dir.is_empty(),
        "set CITRATE_BGE_MODEL_DIR to the pinned bge-base-en-v1.5 directory \
         (runtime-deps bge-base-en-v1.5.tar.gz, sha256 cea05ebe...)"
    );
    let e = TransformerEmbedder::bge_base().expect("load pinned bge-base-en-v1.5");
    assert_eq!(e.dim(), DIM);

    let doc = fixture();
    let mut failures = Vec::new();
    let (mut worst_abs, mut worst_cos) = (0f32, 1f64);
    for entry in entries(&doc) {
        let name = entry["name"].as_str().expect("name");
        let text = entry["text"].as_str().expect("text");
        let want_ids = ids(entry);
        let got_ids = e.token_ids(text).expect("tokenize");
        if got_ids != want_ids {
            failures.push(format!(
                "{name}: token ids differ ({} vs {} ids)",
                got_ids.len(),
                want_ids.len()
            ));
        }

        let want = vector(entry);
        let got = e.embed(text).expect("embed").data;
        assert_eq!(got.len(), want.len(), "{name}: dims");
        // f32::max skips NaN, so check finiteness before folding the diff.
        assert!(
            got.iter().all(|x| x.is_finite()),
            "{name}: non-finite component"
        );
        let mad = want
            .iter()
            .zip(&got)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        let dot: f64 = want
            .iter()
            .zip(&got)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        let na = want
            .iter()
            .map(|a| f64::from(*a).powi(2))
            .sum::<f64>()
            .sqrt();
        let nb = got
            .iter()
            .map(|b| f64::from(*b).powi(2))
            .sum::<f64>()
            .sqrt();
        let cos = dot / (na * nb);
        eprintln!(
            "{name:<22} tokens {:>3}  max|diff| {mad:.3e}  1-cos {:.3e}",
            want_ids.len(),
            1.0 - cos
        );
        worst_abs = worst_abs.max(mad);
        worst_cos = worst_cos.min(cos);
        if cos.is_nan() || cos < MIN_COSINE {
            failures.push(format!("{name}: cosine {cos:.9} < {MIN_COSINE}"));
        }
        if mad > MAX_ABS_DIFF {
            failures.push(format!("{name}: max abs diff {mad:.3e} > {MAX_ABS_DIFF:e}"));
        }
    }
    eprintln!("max abs diff {worst_abs:.3e}, min cosine {worst_cos:.9}");
    assert!(
        failures.is_empty(),
        "bge embeddings drifted from the candle 0.10.2 fixture (stored indexes would need a re-embed):\n{}",
        failures.join("\n")
    );
}
