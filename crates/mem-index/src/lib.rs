//! `mem-index` — the vector layer for citrate-memories.
//!
//! Two pieces: an [`Embedder`] (text -> versioned vector) and a [`VectorIndex`]
//! (model-bound nearest-neighbour search). Both are deliberately small traits so
//! v2 can swap the v1 baselines (feature-hashing embedder, brute-force index) for
//! a transformer embedder and an HNSW index without touching callers.

pub mod embed;
pub mod hnsw;
pub mod index;

pub use embed::{EmbedError, Embedder, HashingEmbedder};
pub use hnsw::HnswIndex;
pub use index::{BruteForceIndex, IndexError, Neighbor, VectorIndex};

#[cfg(feature = "transformer")]
pub mod transformer;
#[cfg(feature = "transformer")]
pub use transformer::TransformerEmbedder;
