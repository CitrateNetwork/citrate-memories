//! Precomputed node embeddings shipped with a corpus (format 2, optional).
//!
//! Embedding the whole release corpus with BGE on a member's CPU takes most of
//! two hours (about 1.7 nodes per second measured on an Apple Silicon laptop,
//! 10,630 nodes), and the memory daemon cannot run while the importer holds the
//! store. So the release build may embed every node once, with the same pinned
//! model the app bundles, and ship the vectors beside each tenant:
//!
//! - `tenants/<tenant>.vectors.f16`: for each node in the tenant file's order,
//!   `dim` little-endian IEEE half floats (half the size of f32; BGE vectors are
//!   unit-length, so the rounding error is about 1e-3 per component and the
//!   importer re-normalises each vector);
//! - the tenant's manifest entry records the file, its sha256, the model id, the
//!   dimension and the sha256 of the model weights that produced it.
//!
//! The importer reuses the vectors only when the store's embedder has the same
//! model id and dimension and the caller proves the same weights (their sha256);
//! otherwise it embeds as before. Vectors are never part of node identity.

use mem_core::VersionedVector;
use serde::{Deserialize, Serialize};

use crate::{CorpusError, TENANTS_DIR};

/// The only encoding format 2 defines.
pub const ENCODING_F16LE: &str = "f16le";

/// Largest dimension the importer accepts.
pub const MAX_DIM: usize = 4096;

/// A tenant's vectors file, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorsEntry {
    /// Path relative to the corpus directory ([`vectors_file`]).
    pub file: String,
    pub sha256: String,
    pub encoding: String,
    /// `Embedder::model_id` of the model that produced the vectors.
    pub model: String,
    pub dim: usize,
    /// sha256 of the model weights file (e.g. BGE `model.safetensors`).
    pub weights_sha256: String,
}

/// Vectors file path inside a corpus directory.
pub fn vectors_file(tenant: &str) -> String {
    format!("{TENANTS_DIR}/{tenant}.vectors.f16")
}

/// Encode vectors (all of length `dim`) as consecutive f16 little-endian values.
pub fn encode(vectors: &[Vec<f32>], dim: usize) -> Result<Vec<u8>, CorpusError> {
    let mut out = Vec::with_capacity(vectors.len() * dim * 2);
    for v in vectors {
        if v.len() != dim {
            return Err(CorpusError::Embed(format!(
                "vector of length {} where {dim} was expected",
                v.len()
            )));
        }
        for x in v {
            out.extend_from_slice(&half::f16::from_f32(*x).to_le_bytes());
        }
    }
    Ok(out)
}

/// The `index`-th vector of an f16 file, re-normalised to unit length (a zero
/// vector stays zero), stamped with `model`.
pub fn vector_at(bytes: &[u8], dim: usize, index: usize, model: &str) -> Option<VersionedVector> {
    let start = index.checked_mul(dim)?.checked_mul(2)?;
    let end = start.checked_add(dim.checked_mul(2)?)?;
    let raw = bytes.get(start..end)?;
    let mut data: Vec<f32> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| half::f16::from_le_bytes(*c).to_f32())
        .collect();
    if data.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let norm: f32 = data.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in data.iter_mut() {
            *x /= norm;
        }
    }
    Some(VersionedVector {
        model: model.to_string(),
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: Vec<f32>) -> Vec<f32> {
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.into_iter().map(|x| x / n).collect()
    }

    #[test]
    fn round_trip_is_close_and_unit_length() {
        let a = unit(
            (0..768)
                .map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0)
                .collect(),
        );
        let b = unit((0..768).map(|i| ((i * 13 % 7) as f32) - 3.0).collect());
        let bytes = encode(&[a.clone(), b.clone()], 768).unwrap();
        assert_eq!(bytes.len(), 2 * 768 * 2);
        for (i, want) in [a, b].iter().enumerate() {
            let got = vector_at(&bytes, 768, i, "m").unwrap();
            assert_eq!(got.model, "m");
            let cos: f32 = got.data.iter().zip(want).map(|(x, y)| x * y).sum();
            assert!(cos > 0.9999, "cosine to the original {cos}");
            let norm: f32 = got.data.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-5, "re-normalised, got {norm}");
        }
    }

    #[test]
    fn zero_vector_stays_zero_and_out_of_range_is_none() {
        let bytes = encode(&[vec![0.0; 4]], 4).unwrap();
        assert_eq!(vector_at(&bytes, 4, 0, "m").unwrap().data, vec![0.0; 4]);
        assert!(vector_at(&bytes, 4, 1, "m").is_none());
        assert!(vector_at(&bytes[..7], 4, 0, "m").is_none());
        assert!(vector_at(&bytes, usize::MAX, 1, "m").is_none());
        assert!(vector_at(&bytes, usize::MAX, 0, "m").is_none());
    }

    #[test]
    fn non_finite_values_are_refused() {
        let mut bytes = encode(&[vec![1.0, 0.0]], 2).unwrap();
        bytes[0..2].copy_from_slice(&half::f16::INFINITY.to_le_bytes());
        assert!(vector_at(&bytes, 2, 0, "m").is_none());
    }

    #[test]
    fn encode_refuses_a_wrong_length() {
        assert!(encode(&[vec![1.0, 2.0, 3.0]], 2).is_err());
    }

    #[test]
    fn layout_is_little_endian_half_floats() {
        let bytes = encode(&[vec![1.0, -2.0]], 2).unwrap();
        // 1.0 = 0x3C00, -2.0 = 0xC000 in IEEE binary16.
        assert_eq!(bytes, vec![0x00, 0x3C, 0x00, 0xC0]);
    }
}
