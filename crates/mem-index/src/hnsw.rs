//! HNSW approximate nearest-neighbour index (WP-0.4b) — sub-linear queries at
//! scale behind the same [`VectorIndex`] trait as the exact baseline.
//!
//! Hierarchical Navigable Small World (Malkov & Yashunin 2016): a stack of
//! proximity graphs, sparser toward the top. A query greedily descends the
//! sparse layers to land near its neighbourhood, then runs a beam search
//! (width `ef`) on the dense bottom layer — `O(log n)`-ish hops instead of a
//! full scan. Neighbour selection uses the paper's diversity heuristic
//! (Algorithm 4 + keepPrunedConnections): plain top-M wires clusters of
//! near-duplicates into cliques and disconnects them from the rest of the
//! graph — and real memory corpora (commit messages, sprint docs) are exactly
//! that clustered.
//!
//! **Deterministic by construction** (the Derived plane must rebuild
//! byte-identically): the usual RNG for level assignment is replaced by a
//! blake3 hash of the node id, so the same nodes inserted in the same order
//! always produce the same graph — and store iteration is key-sorted, so the
//! order itself is deterministic too.
//!
//! Vectors are L2-normalised on `add`, making dot product equal cosine
//! similarity (the zero vector stays zero and scores 0 against everything,
//! matching the brute-force baseline).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use mem_core::{ContentHash, VersionedVector};

use crate::index::{IndexError, Neighbor, VectorIndex};

/// f32 with a total order (via `total_cmp`) so similarities can live in heaps.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sim(f32);

impl Eq for Sim {}
impl PartialOrd for Sim {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Sim {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

struct HnswNode {
    id: ContentHash,
    /// L2-normalised embedding (dot == cosine).
    vec: Vec<f32>,
    /// Out-links per layer, `links[l]` for layers `0..=level`.
    links: Vec<Vec<u32>>,
}

/// Approximate cosine kNN. Binds to the first vector's model and dim; rejects
/// mismatches (never silently compares across vector spaces).
pub struct HnswIndex {
    model: Option<String>,
    dim: Option<usize>,
    /// Max out-degree per layer above 0 (layer 0 allows `2 * m`).
    m: usize,
    /// Beam width while inserting.
    ef_construction: usize,
    /// Beam width while querying (raised to `k` if smaller).
    ef_search: usize,
    /// 1/ln(m): the level-assignment decay from the paper.
    level_norm: f64,
    nodes: Vec<HnswNode>,
    /// Index of the node on the highest level (the global entry point).
    entry: Option<u32>,
}

impl Default for HnswIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl HnswIndex {
    /// Paper-typical defaults: M=16, efConstruction=128, efSearch=64.
    pub fn new() -> Self {
        Self::with_params(16, 128, 64)
    }

    /// `m` ≥ 2 (out-degree), `ef_construction`/`ef_search` ≥ 1 (beam widths).
    pub fn with_params(m: usize, ef_construction: usize, ef_search: usize) -> Self {
        let m = m.max(2);
        Self {
            model: None,
            dim: None,
            m,
            ef_construction: ef_construction.max(1),
            ef_search: ef_search.max(1),
            level_norm: 1.0 / (m as f64).ln(),
            nodes: Vec::new(),
            entry: None,
        }
    }

    /// The model this index is bound to (None until the first `add`).
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn check(&self, v: &VersionedVector) -> Result<(), IndexError> {
        if let Some(m) = &self.model {
            if m != &v.model {
                return Err(IndexError::ModelMismatch { expected: m.clone(), got: v.model.clone() });
            }
        }
        if let Some(d) = self.dim {
            if d != v.data.len() {
                return Err(IndexError::DimMismatch { expected: d, got: v.data.len() });
            }
        }
        Ok(())
    }

    /// Deterministic stand-in for the paper's `-ln(rand()) * mL`: the uniform
    /// draw comes from a blake3 hash of the node id, so the same node always
    /// lands on the same level.
    fn level_for(&self, id: &ContentHash) -> usize {
        let h = blake3::hash(id.as_bytes());
        let b = h.as_bytes();
        let x = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        // (0, 1]: never 0, so ln is finite.
        let u = (x as f64 + 1.0) / (u64::MAX as f64 + 2.0);
        let l = (-u.ln() * self.level_norm).floor() as usize;
        l.min(32)
    }

    fn max_degree(&self, layer: usize) -> usize {
        if layer == 0 {
            self.m * 2
        } else {
            self.m
        }
    }

    fn similarity(&self, a: u32, q: &[f32]) -> f32 {
        let v = &self.nodes[a as usize].vec;
        v.iter().zip(q).map(|(x, y)| x * y).sum()
    }

    /// Greedy hill-climb on one layer: move to the best-scoring link until no
    /// link improves. The descent step for layers above the target.
    fn greedy_closest(&self, mut ep: u32, q: &[f32], layer: usize) -> u32 {
        let mut best = self.similarity(ep, q);
        loop {
            let mut improved = false;
            for &nb in &self.nodes[ep as usize].links[layer] {
                let s = self.similarity(nb, q);
                if s > best {
                    best = s;
                    ep = nb;
                    improved = true;
                }
            }
            if !improved {
                return ep;
            }
        }
    }

    /// Beam search on one layer (Algorithm 2): expand the closest unexpanded
    /// candidate while it can still beat the worst of the best-`ef` found.
    /// Returns the best-`ef` as (similarity, node) sorted descending.
    fn search_layer(&self, ep: u32, q: &[f32], ef: usize, layer: usize) -> Vec<(Sim, u32)> {
        let mut visited = vec![false; self.nodes.len()];
        visited[ep as usize] = true;
        let entry_sim = Sim(self.similarity(ep, q));

        // Unexpanded frontier, best first.
        let mut candidates: BinaryHeap<(Sim, u32)> = BinaryHeap::from([(entry_sim, ep)]);
        // Running best-ef, worst first (so the floor is peekable).
        let mut best: BinaryHeap<Reverse<(Sim, u32)>> = BinaryHeap::from([Reverse((entry_sim, ep))]);

        while let Some((sim, node)) = candidates.pop() {
            let floor = best.peek().map(|Reverse((s, _))| *s).unwrap_or(Sim(f32::NEG_INFINITY));
            if sim < floor && best.len() >= ef {
                break;
            }
            for &nb in &self.nodes[node as usize].links[layer] {
                if visited[nb as usize] {
                    continue;
                }
                visited[nb as usize] = true;
                let s = Sim(self.similarity(nb, q));
                let floor = best.peek().map(|Reverse((f, _))| *f).unwrap_or(Sim(f32::NEG_INFINITY));
                if best.len() < ef || s > floor {
                    candidates.push((s, nb));
                    best.push(Reverse((s, nb)));
                    if best.len() > ef {
                        best.pop();
                    }
                }
            }
        }

        let mut out: Vec<(Sim, u32)> = best.into_iter().map(|Reverse(p)| p).collect();
        out.sort_by(|a, b| b.cmp(a));
        out
    }

    /// Diversity-aware neighbour selection (the paper's Algorithm 4, with
    /// keepPrunedConnections). Walking candidates best-first, keep one only if
    /// it is closer to `base` than to every neighbour already kept — so links
    /// bridge *between* clusters instead of forming cliques of near-duplicates
    /// (plain top-M disconnects clustered corpora). Skipped candidates back-fill
    /// any remaining capacity.
    /// `candidates` are (similarity-to-base, node), sorted descending.
    fn select_diverse(&self, candidates: &[(Sim, u32)], cap: usize) -> Vec<u32> {
        let mut kept: Vec<(Sim, u32)> = Vec::with_capacity(cap);
        let mut skipped: Vec<u32> = Vec::new();
        for &(sim_to_base, c) in candidates {
            if kept.len() >= cap {
                break;
            }
            let cv = &self.nodes[c as usize].vec;
            let diverse = kept.iter().all(|&(_, s)| {
                let sim_to_kept: f32 = self.nodes[s as usize].vec.iter().zip(cv).map(|(x, y)| x * y).sum();
                sim_to_base.0 > sim_to_kept
            });
            if diverse {
                kept.push((sim_to_base, c));
            } else {
                skipped.push(c);
            }
        }
        let mut out: Vec<u32> = kept.into_iter().map(|(_, c)| c).collect();
        for c in skipped {
            if out.len() >= cap {
                break;
            }
            out.push(c);
        }
        out
    }

    /// Re-select a node's links down to `max_degree` with the diversity heuristic.
    fn prune(&mut self, node: u32, layer: usize) {
        let cap = self.max_degree(layer);
        if self.nodes[node as usize].links[layer].len() <= cap {
            return;
        }
        let q = self.nodes[node as usize].vec.clone();
        let mut scored: Vec<(Sim, u32)> = self.nodes[node as usize].links[layer]
            .iter()
            .map(|&nb| (Sim(self.similarity(nb, &q)), nb))
            .collect();
        scored.sort_by(|a, b| b.cmp(a));
        self.nodes[node as usize].links[layer] = self.select_diverse(&scored, cap);
    }
}

fn normalized(data: &[f32]) -> Vec<f32> {
    let norm: f32 = data.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        data.iter().map(|x| x / norm).collect()
    } else {
        data.to_vec()
    }
}

impl VectorIndex for HnswIndex {
    fn add(&mut self, id: ContentHash, v: &VersionedVector) -> Result<(), IndexError> {
        self.check(v)?;
        self.model.get_or_insert_with(|| v.model.clone());
        self.dim.get_or_insert(v.data.len());

        let level = self.level_for(&id);
        let new_idx = self.nodes.len() as u32;
        self.nodes.push(HnswNode { id, vec: normalized(&v.data), links: vec![Vec::new(); level + 1] });

        let Some(entry) = self.entry else {
            self.entry = Some(new_idx);
            return Ok(());
        };
        let q = self.nodes[new_idx as usize].vec.clone();
        let top = self.nodes[entry as usize].links.len() - 1;

        // Descend the layers above the new node's level greedily.
        let mut ep = entry;
        for layer in ((level + 1)..=top).rev() {
            ep = self.greedy_closest(ep, &q, layer);
        }

        // On each shared layer: beam-search, link to M diverse neighbours, prune.
        for layer in (0..=level.min(top)).rev() {
            let found = self.search_layer(ep, &q, self.ef_construction, layer);
            if let Some(&(_, best)) = found.first() {
                ep = best;
            }
            for nb in self.select_diverse(&found, self.m) {
                self.nodes[new_idx as usize].links[layer].push(nb);
                self.nodes[nb as usize].links[layer].push(new_idx);
                self.prune(nb, layer);
            }
        }

        if level > top {
            self.entry = Some(new_idx);
        }
        Ok(())
    }

    fn search(&self, q: &VersionedVector, k: usize) -> Result<Vec<Neighbor>, IndexError> {
        self.check(q)?;
        let Some(entry) = self.entry else {
            return Ok(Vec::new());
        };
        let qn = normalized(&q.data);

        // Greedy descent to layer 1, then a beam search on the bottom layer.
        let mut ep = entry;
        let top = self.nodes[entry as usize].links.len() - 1;
        for layer in (1..=top).rev() {
            ep = self.greedy_closest(ep, &qn, layer);
        }
        let ef = self.ef_search.max(k);
        let found = self.search_layer(ep, &qn, ef, 0);

        Ok(found
            .into_iter()
            .take(k)
            .map(|(sim, idx)| Neighbor { id: self.nodes[idx as usize].id, score: sim.0 })
            .collect())
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{Embedder, HashingEmbedder};
    use crate::index::BruteForceIndex;
    use mem_core::IdBuilder;

    fn id(s: &str) -> ContentHash {
        IdBuilder::new("test").field_str(1, s).finish()
    }

    /// A deterministic corpus with real lexical clusters (so similarity is
    /// meaningful, not uniform noise).
    fn corpus(n: usize) -> Vec<String> {
        let topics = [
            "ghostdag blue score consensus tip selection",
            "lora adapter provenance composition registry",
            "rocksdb column family atomic batch storage",
            "siwe wallet login session nonce identity",
            "mcp json rpc tool call capability grant",
            "embedding vector cosine similarity index",
            "sprint handoff blocker work package status",
        ];
        let extras = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta"];
        (0..n)
            .map(|i| {
                // Repeat the off-topic filler a varying number of times so norms —
                // and hence similarities — genuinely differ across items (a corpus
                // of analytically-tied scores only tests float noise, not ranking).
                let filler = format!(" {}", extras[(i / 7) % extras.len()]).repeat(1 + i % 5);
                format!("{}{} v{}", topics[i % topics.len()], filler, i)
            })
            .collect()
    }

    fn build_pair(texts: &[String]) -> (HnswIndex, BruteForceIndex) {
        let e = HashingEmbedder::new(256);
        let mut hnsw = HnswIndex::new();
        let mut brute = BruteForceIndex::new();
        for t in texts {
            let v = e.embed(t).unwrap();
            hnsw.add(id(t), &v).unwrap();
            brute.add(id(t), &v).unwrap();
        }
        (hnsw, brute)
    }

    #[test]
    fn matches_brute_force_on_clustered_corpus() {
        let texts = corpus(300);
        let (hnsw, brute) = build_pair(&texts);
        let e = HashingEmbedder::new(256);
        assert_eq!(hnsw.len(), 300);

        for query in ["ghostdag consensus tip", "lora adapter registry", "rocksdb atomic batch"] {
            let q = e.embed(query).unwrap();
            let h = hnsw.search(&q, 10).unwrap();
            let b = brute.search(&q, 10).unwrap();
            // Top-1 must agree with exact search up to ties: hash collisions can
            // make two corpus items embed identically, and ANN order among
            // identical vectors is undefined.
            assert!((h[0].score - b[0].score).abs() < 1e-5, "top score must match exact search for {query:?}");
            let b_top: std::collections::HashSet<_> =
                b.iter().filter(|n| (n.score - b[0].score).abs() < 1e-5).map(|n| n.id).collect();
            assert!(b_top.contains(&h[0].id), "top-1 must be one of the exact-search co-leaders for {query:?}");
            let bset: std::collections::HashSet<_> = b.iter().map(|n| n.id).collect();
            let overlap = h.iter().filter(|n| bset.contains(&n.id)).count();
            assert!(overlap >= 8, "top-10 overlap with exact search was {overlap}/10 for {query:?}");
        }
    }

    #[test]
    fn every_item_retrieves_itself_top1() {
        let texts = corpus(120);
        let (hnsw, _) = build_pair(&texts);
        let e = HashingEmbedder::new(256);
        for t in &texts {
            let hits = hnsw.search(&e.embed(t).unwrap(), 5).unwrap();
            assert!((hits[0].score - 1.0).abs() < 1e-5, "self-similarity should be 1.0 for {t:?}");
            // Up to ties (identical vectors from hash collisions), the item itself
            // must be among the perfect-score hits.
            assert!(hits.iter().any(|h| h.id == id(t)), "self-retrieval failed for {t:?}");
        }
    }

    #[test]
    fn rebuild_is_deterministic() {
        let texts = corpus(150);
        let (a, _) = build_pair(&texts);
        let (b, _) = build_pair(&texts);
        let e = HashingEmbedder::new(256);
        let q = e.embed("capability grant tool call").unwrap();
        assert_eq!(a.search(&q, 10).unwrap(), b.search(&q, 10).unwrap());
    }

    #[test]
    fn rejects_cross_model_add_and_query() {
        let small = HashingEmbedder::new(64);
        let big = HashingEmbedder::new(128);
        let mut idx = HnswIndex::new();
        idx.add(id("a"), &small.embed("hello world").unwrap()).unwrap();
        assert!(matches!(
            idx.add(id("b"), &big.embed("y").unwrap()),
            Err(IndexError::ModelMismatch { .. })
        ));
        assert!(matches!(
            idx.search(&big.embed("hello world").unwrap(), 1),
            Err(IndexError::ModelMismatch { .. })
        ));
    }

    #[test]
    fn empty_index_returns_no_neighbors() {
        let e = HashingEmbedder::new(32);
        let idx = HnswIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.search(&e.embed("anything").unwrap(), 5).unwrap(), vec![]);
    }
}
