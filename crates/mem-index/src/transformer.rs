//! Transformer embedder (WP-0.4b): a real BERT sentence-embedding model run
//! locally on CPU via [`candle`], behind the `transformer` feature.
//!
//! This is the search-quality upgrade over the feature-hashing baseline: instead
//! of token-overlap, it produces dense semantic embeddings, so paraphrases and
//! synonyms land near each other. It implements the same [`Embedder`] trait, and
//! reports a distinct [`model_id`](Embedder::model_id) (e.g. `"bge-base-en-v1.5"`),
//! so the index's model-version guard keeps these vectors from ever being compared
//! against hashing-baseline vectors — the swap is safe by construction.
//!
//! Default model: **BAAI/bge-base-en-v1.5** (768-d). Weights and tokenizer are
//! fetched once from the HuggingFace Hub and cached in `~/.cache/huggingface`;
//! every subsequent load and all inference are fully local (decision #5).
//!
//! Pooling follows bge's design: the `[CLS]` token representation, L2-normalised
//! so a dot product equals cosine similarity.

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::{Tokenizer, TruncationParams};

use mem_core::VersionedVector;

use crate::embed::{EmbedError, Embedder};

/// Default sentence-embedding model: BAAI/bge-base-en-v1.5 (768-d).
pub const DEFAULT_MODEL: &str = "BAAI/bge-base-en-v1.5";
/// `model_id` reported for the default model (the bge repo basename).
pub const DEFAULT_MODEL_ID: &str = "bge-base-en-v1.5";
/// bge-base accepts up to 512 tokens; longer inputs are truncated.
const MAX_TOKENS: usize = 512;

/// A BERT sentence embedder. Loads weights once; `embed` runs a CPU forward pass.
pub struct TransformerEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    model_id: String,
    dim: usize,
}

impl TransformerEmbedder {
    /// Load the default model ([`DEFAULT_MODEL`]) from the HuggingFace Hub cache,
    /// downloading on first use.
    pub fn bge_base() -> Result<Self, EmbedError> {
        Self::load(DEFAULT_MODEL, DEFAULT_MODEL_ID, "main")
    }

    /// Load an arbitrary BERT-architecture model. `repo` is the HF repo id (e.g.
    /// `"BAAI/bge-base-en-v1.5"`), `model_id` is the stable tag stamped on every
    /// vector (and enforced by the index guard), and `revision` pins the repo.
    pub fn load(repo: &str, model_id: &str, revision: &str) -> Result<Self, EmbedError> {
        let device = Device::Cpu;
        let api = Api::new().map_err(|e| EmbedError::Load(format!("hf-hub init: {e}")))?;
        let repo_handle = api.repo(Repo::with_revision(
            repo.to_string(),
            RepoType::Model,
            revision.to_string(),
        ));

        let get = |file: &str| -> Result<std::path::PathBuf, EmbedError> {
            repo_handle
                .get(file)
                .map_err(|e| EmbedError::Load(format!("fetch {file} from {repo}: {e}")))
        };
        let config_path = get("config.json")?;
        let tokenizer_path = get("tokenizer.json")?;
        let weights_path = get("model.safetensors")?;

        let config_json = std::fs::read_to_string(&config_path)
            .map_err(|e| EmbedError::Load(format!("read config.json: {e}")))?;
        let config: Config = serde_json::from_str(&config_json)
            .map_err(|e| EmbedError::Load(format!("parse config.json: {e}")))?;
        let dim = config.hidden_size;

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| EmbedError::Load(format!("load tokenizer: {e}")))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| EmbedError::Load(format!("set truncation: {e}")))?;

        // SAFETY: mmap of a trusted, locally-cached safetensors file. The bytes are
        // not mutated for the lifetime of the mapping (read-only weights).
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)
                .map_err(|e| EmbedError::Load(format!("map safetensors: {e}")))?
        };
        let model =
            BertModel::load(vb, &config).map_err(|e| EmbedError::Load(format!("build model: {e}")))?;

        Ok(Self {
            model,
            tokenizer,
            device,
            model_id: model_id.to_string(),
            dim,
        })
    }

    /// Tokenize, run the forward pass, CLS-pool, and L2-normalise.
    fn embed_inner(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| EmbedError::Tokenize(e.to_string()))?;
        let ids = encoding.get_ids();
        if ids.is_empty() {
            // No tokens (e.g. empty/whitespace input): a zero vector is the
            // well-defined "no signal" embedding (matches the hashing baseline).
            return Ok(vec![0.0; self.dim]);
        }

        let to_inf = |e: candle_core::Error| EmbedError::Inference(e.to_string());
        // [1, seq]
        let input_ids = Tensor::new(ids, &self.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(to_inf)?;
        let token_type_ids = input_ids.zeros_like().map_err(to_inf)?;
        let attention_mask = input_ids.ones_like().map_err(to_inf)?;

        // [1, seq, hidden]
        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(to_inf)?;
        // CLS pooling: take the first token's hidden state -> [hidden].
        let cls = hidden.i((0, 0)).map_err(to_inf)?;
        let mut data = cls.to_vec1::<f32>().map_err(to_inf)?;

        let norm: f32 = data.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in data.iter_mut() {
                *x /= norm;
            }
        }
        Ok(data)
    }
}

impl Embedder for TransformerEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> Result<VersionedVector, EmbedError> {
        let data = self.embed_inner(text)?;
        Ok(VersionedVector {
            model: self.model_id.clone(),
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BruteForceIndex, VectorIndex};
    use mem_core::IdBuilder;

    fn id(s: &str) -> mem_core::ContentHash {
        IdBuilder::new("test").field_str(1, s).finish()
    }

    fn cos(a: &VersionedVector, b: &VersionedVector) -> f32 {
        a.data.iter().zip(&b.data).map(|(x, y)| x * y).sum()
    }

    /// One opt-in end-to-end test: downloads bge-base (~440MB) on first run, then
    /// runs real CPU inference. Loads the model exactly once and asserts every
    /// property sequentially, so there is no parallel race on the HF download lock
    /// and no `--test-threads=1` requirement. Run with:
    ///   cargo test -p mem-index --features transformer --release -- --ignored
    #[test]
    #[ignore = "downloads bge-base + runs CPU inference"]
    fn bge_base_end_to_end() {
        let e = TransformerEmbedder::bge_base().expect("load bge-base");

        // Shape + identity + normalisation.
        assert_eq!(e.model_id(), DEFAULT_MODEL_ID);
        assert_eq!(e.dim(), 768);
        let v = e.embed("ghostdag blue score finality").unwrap();
        assert_eq!(v.data.len(), 768);
        assert_eq!(v.model, DEFAULT_MODEL_ID);
        let norm: f32 = v.data.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "expected unit vector, got {norm}");

        // Determinism (the trait's contract).
        assert_eq!(
            e.embed("compose two lora adapters").unwrap(),
            e.embed("compose two lora adapters").unwrap()
        );

        // The WP-0.4b win: a paraphrase with NO shared content words ranks above an
        // unrelated sentence. The hashing baseline (token overlap) cannot do this.
        let anchor = e.embed("the validator finalized the block").unwrap();
        let paraphrase = e.embed("a node confirmed the ledger entry").unwrap();
        let unrelated = e.embed("she planted tomatoes in the garden").unwrap();
        let (near, far) = (cos(&anchor, &paraphrase), cos(&anchor, &unrelated));
        assert!(near > far, "paraphrase {near} should outrank unrelated {far}");

        // Integrates with the model-version-guarded index.
        let mut idx = BruteForceIndex::new();
        idx.add(id("a"), &e.embed("consensus reorg depth and finality").unwrap()).unwrap();
        idx.add(id("b"), &e.embed("baking sourdough bread at home").unwrap()).unwrap();
        idx.add(id("c"), &e.embed("chain reorganization and block finalization").unwrap()).unwrap();
        let hits = idx.search(&e.embed("how deep can a reorg go before finality").unwrap(), 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|n| n.id != id("b")), "the cooking node must not be top-2");
        assert_eq!(idx.model(), Some(DEFAULT_MODEL_ID));
    }
}
