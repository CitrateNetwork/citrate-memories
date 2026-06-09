//! Embedders: text -> [`VersionedVector`].
//!
//! Every embedding is tagged with the model that produced it, so the index can
//! refuse to mix vector spaces (decision: re-embedding is safe, identity never
//! depends on the vector — see `mem-core::node`).
//!
//! v1 ships a deterministic, dependency-free **feature-hashing** embedder (the
//! "hashing trick" — a real, widely-used technique, not a placeholder). It needs
//! no model download and no network, so it is reproducible across machines. The
//! transformer/ONNX backend (bge/gte/nomic) lands in WP-0.4b behind a feature
//! and implements this same trait.

use mem_core::VersionedVector;

/// An embedding failure. The feature-hashing baseline never errors; model-backed
/// embedders (WP-0.4b) surface load/tokenize/inference failures here rather than
/// panicking (Rule 8) or silently returning a corrupt vector.
#[derive(Debug)]
pub enum EmbedError {
    /// Model weights/tokenizer could not be loaded (missing asset, bad config).
    Load(String),
    /// Tokenization of the input failed.
    Tokenize(String),
    /// Forward-pass / tensor inference failed.
    Inference(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Load(m) => write!(f, "embedder load failed: {m}"),
            EmbedError::Tokenize(m) => write!(f, "tokenization failed: {m}"),
            EmbedError::Inference(m) => write!(f, "embedding inference failed: {m}"),
        }
    }
}

impl std::error::Error for EmbedError {}

/// Produces embeddings for text. Implementations must be deterministic: the same
/// input yields the same vector (so the Asserted plane stays reproducible enough
/// for advisory similarity).
pub trait Embedder {
    /// Stable identifier of the model+version producing the vectors, e.g.
    /// `"hashing-v1-d256"`. Stored on every `VersionedVector` and used by the
    /// index to reject cross-space queries.
    fn model_id(&self) -> &str;
    fn dim(&self) -> usize;
    /// Embed `text`. Fallible: model-backed embedders can fail at inference time
    /// (the hashing baseline always returns `Ok`).
    fn embed(&self, text: &str) -> Result<VersionedVector, EmbedError>;
}

/// Deterministic feature-hashing embedder. Tokens are hashed into a fixed-width
/// vector with signed accumulation, then L2-normalised so dot product equals
/// cosine similarity.
pub struct HashingEmbedder {
    dim: usize,
    model_id: String,
}

impl HashingEmbedder {
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "embedding dim must be > 0");
        Self {
            dim,
            model_id: format!("hashing-v1-d{dim}"),
        }
    }

    fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_lowercase())
    }
}

impl Embedder for HashingEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> Result<VersionedVector, EmbedError> {
        let mut data = vec![0f32; self.dim];
        for tok in Self::tokens(text) {
            let h = blake3::hash(tok.as_bytes());
            let b = h.as_bytes();
            // first 8 bytes -> bucket index; next byte's low bit -> sign.
            let idx_seed = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            let idx = (idx_seed % self.dim as u64) as usize;
            let sign = if b[8] & 1 == 0 { 1.0 } else { -1.0 };
            data[idx] += sign;
        }
        // L2 normalise (leave the zero vector as-is for empty/symbol-only input).
        let norm: f32 = data.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in data.iter_mut() {
                *x /= norm;
            }
        }
        Ok(VersionedVector {
            model: self.model_id.clone(),
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let e = HashingEmbedder::new(256);
        assert_eq!(
            e.embed("compose two lora adapters").unwrap(),
            e.embed("compose two lora adapters").unwrap()
        );
    }

    #[test]
    fn model_id_encodes_dim() {
        assert_eq!(HashingEmbedder::new(128).model_id(), "hashing-v1-d128");
    }

    #[test]
    fn normalised_unit_length() {
        let e = HashingEmbedder::new(64);
        let v = e.embed("blue score finality reorg").unwrap();
        let norm: f32 = v.data.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "expected unit vector, got norm {norm}");
    }

    #[test]
    fn empty_input_is_zero_vector() {
        let e = HashingEmbedder::new(32);
        let v = e.embed("!!! --- ").unwrap();
        assert!(v.data.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn shared_vocabulary_is_more_similar() {
        let e = HashingEmbedder::new(512);
        let cos = |a: &str, b: &str| {
            let (x, y) = (e.embed(a).unwrap(), e.embed(b).unwrap());
            x.data.iter().zip(&y.data).map(|(p, q)| p * q).sum::<f32>()
        };
        let near = cos("lora adapter provenance chain", "lora adapter provenance hash");
        let far = cos("lora adapter provenance chain", "gossip peer discovery quic");
        assert!(near > far, "near={near} should exceed far={far}");
    }
}
