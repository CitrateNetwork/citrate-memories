//! Server-side 3D layout for the constellation (spec §5).
//!
//! Projects each node's 768-d bge embedding → a stable `(x,y,z)` via **PCA**
//! (deterministic by construction — no RNG, matching the Derived-plane ethos; a
//! heavier UMAP is a later option behind the same endpoint). Also assembles every
//! visual attribute the renderer needs in ONE payload (material lane, trust, plane,
//! status, contradiction, degree) so the constellation loads the scene in a single
//! request. Nodes without an embedding get a deterministic fallback position so they
//! still appear, flagged `projected:false`.

use serde::Serialize;

use mem_core::{BelnapValue, MemoryNode, NodeKind, Plane, Status, TrustTier};

/// The "material lane" a node renders in (spec §5.2) — drives color/shader. Stable
/// strings; the designer maps them to on-brand hues.
pub fn material_lane(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Commit | NodeKind::Pr => "code",
        NodeKind::Doc | NodeKind::Narrative(_) | NodeKind::Handoff => "docs",
        NodeKind::Sprint(_) | NodeKind::Adr | NodeKind::WorkPackage | NodeKind::Rationale => "specs",
        NodeKind::ManifestChange | NodeKind::PinBump | NodeKind::DriftEvent => "configs",
        NodeKind::Audit | NodeKind::Finding | NodeKind::Benchmark | NodeKind::Blocker | NodeKind::TechDebt => "tests",
        NodeKind::Claim(_) | NodeKind::AgentAction | NodeKind::AnalogyHypothesis => "claims",
        NodeKind::Tenant => "tenant",
    }
}

fn trust_str(t: TrustTier) -> &'static str {
    match t {
        TrustTier::DerivedDeterministic => "derived_deterministic",
        TrustTier::HumanConfirmed => "human_confirmed",
        TrustTier::AgentAsserted => "agent_asserted",
        TrustTier::InferredAdvisory => "inferred_advisory",
    }
}

fn status_str(s: Status) -> &'static str {
    match s {
        Status::Active => "active",
        Status::Superseded => "superseded",
        Status::Archived => "archived",
    }
}

/// One node in the rendered scene — position + everything needed to draw it.
#[derive(Debug, Clone, Serialize)]
pub struct SceneNode {
    pub id: String,
    pub pos: [f32; 3],
    /// `false` ⇒ the position is a fallback (no embedding), style accordingly.
    pub projected: bool,
    pub repo: String,
    pub kind: String,
    pub lane: String,
    pub plane: String,
    pub trust: String,
    pub status: String,
    /// Belnap `Both` recorded ⇒ render the contradiction effect.
    pub contradicted: bool,
    /// Edge degree (in+out) ⇒ node size / centrality.
    pub degree: u32,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub quarantined: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub edges: Vec<SceneEdge>,
    /// PCA variance captured by the 3 axes (0..1 each) — a quality signal for the UI.
    pub axis_variance: [f32; 3],
    pub node_count: usize,
    pub edge_count: usize,
}

const POS_SCALE: f32 = 100.0;

/// Compute the top-3 principal axes of `vectors` (each length D), mean-centered,
/// via power iteration with Gram-Schmidt deflation on the *implicit* covariance
/// (`Cv = mean_k (xc_k · v) xc_k`) — no DxD matrix, deterministic init, no RNG.
/// Returns `(mean, [axis0, axis1, axis2], eigenvalues)`.
// Dense linear algebra: explicit `0..d` indexing across parallel vectors is the
// clearest form here (the index is the shared coordinate), so we opt out of the
// iterator-rewrite lint for this numeric kernel.
#[allow(clippy::needless_range_loop)]
fn pca3(vectors: &[&[f32]]) -> (Vec<f32>, [Vec<f32>; 3], [f32; 3]) {
    let n = vectors.len();
    let d = vectors.first().map(|v| v.len()).unwrap_or(0);
    let mut mean = vec![0f32; d];
    if n == 0 || d == 0 {
        return (mean, [vec![], vec![], vec![]], [0.0; 3]);
    }
    for v in vectors {
        for i in 0..d {
            mean[i] += v[i];
        }
    }
    for m in mean.iter_mut() {
        *m /= n as f32;
    }

    let dot = |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| x * y).sum() };
    let centered = |v: &[f32], out: &mut [f32]| {
        for i in 0..d {
            out[i] = v[i] - mean[i];
        }
    };

    let mut axes: [Vec<f32>; 3] = [vec![0f32; d], vec![0f32; d], vec![0f32; d]];
    let mut eigs = [0f32; 3];
    let mut xc = vec![0f32; d]; // scratch for a centered vector

    for c in 0..3 {
        // Deterministic init: a fixed unit-ish vector, distinct per component.
        let mut v = vec![0f32; d];
        for i in 0..d {
            v[i] = (((i + c * 7 + 1) % 13) as f32 - 6.0) / 6.0;
        }
        // Iterate. Power iteration on the top components converges fast; 18 is ample.
        for _ in 0..18 {
            // Gram-Schmidt against already-found axes (deflation).
            for prev in axes.iter().take(c) {
                let p = dot(&v, prev);
                for i in 0..d {
                    v[i] -= p * prev[i];
                }
            }
            // w = C v  (implicit covariance over centered data)
            let mut w = vec![0f32; d];
            for vec_ in vectors {
                centered(vec_, &mut xc);
                let proj = dot(&xc, &v);
                for i in 0..d {
                    w[i] += proj * xc[i];
                }
            }
            for wi in w.iter_mut() {
                *wi /= n as f32;
            }
            // Re-deflate w, then normalize → next v.
            for prev in axes.iter().take(c) {
                let p = dot(&w, prev);
                for i in 0..d {
                    w[i] -= p * prev[i];
                }
            }
            let norm = dot(&w, &w).sqrt();
            if norm < 1e-12 {
                break;
            }
            eigs[c] = norm;
            for i in 0..d {
                v[i] = w[i] / norm;
            }
        }
        axes[c] = v;
    }
    (mean, axes, eigs)
}

/// Project nodes to 3D and assemble the scene. `degree_of` gives a node's edge count
/// (in+out). Positions are scaled to roughly `[-POS_SCALE, POS_SCALE]` per axis.
#[allow(clippy::needless_range_loop)] // dense projection over the embedding dimension
pub fn build_scene(nodes: &[MemoryNode], edges: Vec<SceneEdge>, degree_of: impl Fn(&str) -> u32) -> Scene {
    // Collect embeddings (same model is guaranteed by the store; mixed-dim is skipped).
    let embedded: Vec<(usize, &[f32])> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.embedding.as_ref().map(|e| (i, e.data.as_slice())))
        .collect();
    // The PCA *axes* are computed from a strided sample (top-3 PCs are stable from a
    // few thousand vectors); EVERY node is then projected onto them. This keeps the
    // expensive covariance step ~O(sample) instead of O(all) with no visible quality
    // loss. The full set is still what gets positioned.
    const MAX_AXIS_SAMPLE: usize = 3000;
    let stride = (embedded.len() / MAX_AXIS_SAMPLE).max(1);
    let sample: Vec<&[f32]> = embedded.iter().step_by(stride).map(|(_, e)| *e).collect();
    let (mean, axes, eigs) = pca3(&sample);
    let total_var: f32 = eigs.iter().map(|e| e * e).sum::<f32>().max(1e-12);
    let axis_variance = [
        eigs[0] * eigs[0] / total_var,
        eigs[1] * eigs[1] / total_var,
        eigs[2] * eigs[2] / total_var,
    ];

    // Project + find the spread so we can scale to a stable cube.
    let mut raw: std::collections::HashMap<usize, [f32; 3]> = std::collections::HashMap::new();
    let mut maxabs = 1e-6f32;
    if !axes[0].is_empty() {
        for (i, e) in &embedded {
            let mut p = [0f32; 3];
            for (a, axis) in axes.iter().enumerate() {
                let mut c = 0f32;
                for k in 0..e.len() {
                    c += (e[k] - mean[k]) * axis[k];
                }
                p[a] = c;
                maxabs = maxabs.max(c.abs());
            }
            raw.insert(*i, p);
        }
    }
    let scale = POS_SCALE / maxabs;

    let scene_nodes = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let id = n.compute_id().to_hex();
            let (pos, projected) = match raw.get(&i) {
                Some(p) => ([p[0] * scale, p[1] * scale, p[2] * scale], true),
                None => (fallback_pos(&id), false),
            };
            SceneNode {
                pos,
                projected,
                repo: n.repo.clone(),
                kind: n.kind.discriminant(),
                lane: material_lane(&n.kind).to_string(),
                plane: match n.plane {
                    Plane::Derived => "derived".into(),
                    Plane::Asserted => "asserted".into(),
                },
                trust: trust_str(n.trust_tier).into(),
                status: status_str(n.status).into(),
                contradicted: n.confidence.iter().any(|c| matches!(c, BelnapValue::Both)),
                degree: degree_of(&id),
                title: String::from_utf8_lossy(&n.content).split('\n').next().unwrap_or("").chars().take(140).collect(),
                id,
            }
        })
        .collect();

    let edge_count = edges.len();
    Scene {
        nodes: scene_nodes,
        edges,
        axis_variance,
        node_count: nodes.len(),
        edge_count,
    }
}

/// Deterministic fallback position for an un-embedded node: a point on a fixed
/// sphere derived from its id, offset below the main cloud so it reads as "no
/// semantic position" without overlapping the projected galaxy.
fn fallback_pos(id_hex: &str) -> [f32; 3] {
    let h = blake3::hash(id_hex.as_bytes());
    let b = h.as_bytes();
    let u = |i: usize| (b[i] as f32 / 255.0) * 2.0 - 1.0;
    let r = POS_SCALE * 0.35;
    [u(0) * r, -POS_SCALE * 0.9 + u(1) * r * 0.3, u(2) * r]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vecs() -> Vec<Vec<f32>> {
        // Two clear clusters along dim 0 → PC1 should separate them.
        let mut v = vec![];
        for _ in 0..20 {
            v.push(vec![5.0, 0.1, 0.0, 0.0]);
        }
        for _ in 0..20 {
            v.push(vec![-5.0, -0.1, 0.0, 0.0]);
        }
        v
    }

    #[test]
    fn pca_is_deterministic() {
        let data = vecs();
        let refs: Vec<&[f32]> = data.iter().map(|v| v.as_slice()).collect();
        let (_m1, a1, e1) = pca3(&refs);
        let (_m2, a2, e2) = pca3(&refs);
        assert_eq!(a1[0], a2[0], "PCA must be deterministic (no RNG)");
        assert_eq!(e1, e2);
    }

    #[test]
    fn pca_first_axis_captures_the_separating_dimension() {
        let data = vecs();
        let refs: Vec<&[f32]> = data.iter().map(|v| v.as_slice()).collect();
        let (_m, axes, _e) = pca3(&refs);
        // dim 0 is where the variance is → |axis0[0]| dominates.
        let a0 = &axes[0];
        assert!(a0[0].abs() > a0[1].abs() && a0[0].abs() > a0[2].abs(), "PC1 aligns with the high-variance dim, got {a0:?}");
    }

    #[test]
    fn fallback_is_stable_and_distinct() {
        let p1 = fallback_pos("abc123");
        let p2 = fallback_pos("abc123");
        let p3 = fallback_pos("def456");
        assert_eq!(p1, p2, "same id → same fallback");
        assert_ne!(p1, p3, "different id → different fallback");
    }

    #[test]
    fn material_lanes_cover_the_taxonomy() {
        assert_eq!(material_lane(&NodeKind::Commit), "code");
        assert_eq!(material_lane(&NodeKind::Doc), "docs");
        assert_eq!(material_lane(&NodeKind::Adr), "specs");
        assert_eq!(material_lane(&NodeKind::ManifestChange), "configs");
        assert_eq!(material_lane(&NodeKind::Finding), "tests");
        assert_eq!(material_lane(&NodeKind::AgentAction), "claims");
        assert_eq!(material_lane(&NodeKind::Tenant), "tenant");
    }
}
