//! Vector index: nearest-neighbour over node embeddings.
//!
//! v1 ships an exact brute-force cosine index (`O(n)` query) — correct and
//! simple, enough to wire up recall/analogy plumbing. The approximate HNSW
//! backend (sub-linear query at scale) lands in WP-0.4b behind the same
//! [`VectorIndex`] trait.
//!
//! Every index is bound to exactly one embedding model. Adding or querying with a
//! vector from a different model is rejected, never silently compared — mixing
//! vector spaces produces meaningless similarities (the embedding-migration trap
//! from the risk register).

use mem_core::{ContentHash, VersionedVector};

#[derive(Debug, PartialEq, Eq)]
pub enum IndexError {
    /// The vector's model differs from the index's bound model.
    ModelMismatch { expected: String, got: String },
    /// The vector's dimensionality differs from existing entries.
    DimMismatch { expected: usize, got: usize },
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::ModelMismatch { expected, got } => {
                write!(f, "vector model '{got}' does not match index model '{expected}'")
            }
            IndexError::DimMismatch { expected, got } => {
                write!(f, "vector dim {got} does not match index dim {expected}")
            }
        }
    }
}

impl std::error::Error for IndexError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Neighbor {
    pub id: ContentHash,
    /// Cosine similarity in [-1, 1] (higher = nearer).
    pub score: f32,
}

/// A nearest-neighbour index over node embeddings, bound to one embedding model.
pub trait VectorIndex {
    fn add(&mut self, id: ContentHash, v: &VersionedVector) -> Result<(), IndexError>;
    fn search(&self, q: &VersionedVector, k: usize) -> Result<Vec<Neighbor>, IndexError>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Exact cosine kNN. Binds to the first vector's model and dim; rejects mismatches.
#[derive(Default)]
pub struct BruteForceIndex {
    model: Option<String>,
    dim: Option<usize>,
    items: Vec<(ContentHash, Vec<f32>)>,
}

impl BruteForceIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// The model this index is bound to (None until the first `add`).
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn check(&self, v: &VersionedVector) -> Result<(), IndexError> {
        if let Some(m) = &self.model {
            if m != &v.model {
                return Err(IndexError::ModelMismatch {
                    expected: m.clone(),
                    got: v.model.clone(),
                });
            }
        }
        if let Some(d) = self.dim {
            if d != v.data.len() {
                return Err(IndexError::DimMismatch {
                    expected: d,
                    got: v.data.len(),
                });
            }
        }
        Ok(())
    }
}

impl VectorIndex for BruteForceIndex {
    fn add(&mut self, id: ContentHash, v: &VersionedVector) -> Result<(), IndexError> {
        self.check(v)?;
        self.model.get_or_insert_with(|| v.model.clone());
        self.dim.get_or_insert(v.data.len());
        self.items.push((id, v.data.clone()));
        Ok(())
    }

    fn search(&self, q: &VersionedVector, k: usize) -> Result<Vec<Neighbor>, IndexError> {
        self.check(q)?;
        let mut scored: Vec<Neighbor> = self
            .items
            .iter()
            .map(|(id, v)| Neighbor {
                id: *id,
                score: cosine(&q.data, v),
            })
            .collect();
        // Descending by score; total order via partial_cmp with NaN pushed last.
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        Ok(scored)
    }

    fn len(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{Embedder, HashingEmbedder};
    use mem_core::IdBuilder;

    fn id(s: &str) -> ContentHash {
        IdBuilder::new("test").field_str(1, s).finish()
    }

    #[test]
    fn finds_nearest_first() {
        let e = HashingEmbedder::new(512);
        let mut idx = BruteForceIndex::new();
        idx.add(id("a"), &e.embed("lora adapter provenance chain")).unwrap();
        idx.add(id("b"), &e.embed("gossip peer discovery quic")).unwrap();
        idx.add(id("c"), &e.embed("lora adapter provenance hash")).unwrap();
        assert_eq!(idx.len(), 3);

        let q = e.embed("lora adapter provenance");
        let hits = idx.search(&q, 2).unwrap();
        assert_eq!(hits.len(), 2);
        // a and c (lora/provenance) should outrank b (networking).
        assert!(hits.iter().all(|n| n.id != id("b")), "networking node should not be top-2");
        assert!(hits[0].score >= hits[1].score, "results sorted descending");
    }

    #[test]
    fn rejects_cross_model_query() {
        let small = HashingEmbedder::new(64);
        let big = HashingEmbedder::new(128);
        let mut idx = BruteForceIndex::new();
        idx.add(id("a"), &small.embed("hello world")).unwrap();
        // Different model id (and dim) -> rejected, not silently compared.
        let err = idx.search(&big.embed("hello world"), 1).unwrap_err();
        assert!(matches!(err, IndexError::ModelMismatch { .. }));
    }

    #[test]
    fn rejects_cross_model_add() {
        let small = HashingEmbedder::new(64);
        let big = HashingEmbedder::new(128);
        let mut idx = BruteForceIndex::new();
        idx.add(id("a"), &small.embed("x")).unwrap();
        assert!(idx.add(id("b"), &big.embed("y")).is_err());
    }

    #[test]
    fn empty_index_returns_no_neighbors() {
        let e = HashingEmbedder::new(32);
        let idx = BruteForceIndex::new();
        assert!(idx.is_empty());
        // Querying an empty (unbound) index is fine — no model bound yet.
        assert_eq!(idx.search(&e.embed("anything"), 5).unwrap(), vec![]);
    }
}
