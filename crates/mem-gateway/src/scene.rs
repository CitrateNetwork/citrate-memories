//! The constellation "scene" the memrizz webapp renders.
//!
//! CONTRACT (matched to the deployed webapp's constellation component): the
//! client consumes the gateway response directly as `{nodes, edges}` (its mock
//! generator produces the same shape). Each node MUST carry a 3-D `pos` array —
//! the renderer does `node.pos.map(...)` and reads `pos[0..2]` — plus `degree`,
//! `trust`, `plane` ("derived"/"asserted"), `repo`, `kind`, `status`, `title`,
//! `lane`, `projected`, `contradicted`. Edges are `{from, to, kind, quarantined}`.
//!
//! Coordinates come from a deterministic PCA-3D projection of the bge embeddings
//! (the renderer re-normalises by max-abs, so any consistent scale is fine). When
//! too few nodes carry embeddings we fall back to a stable ring.

use serde::Serialize;

/// One node handed to the projector before coordinates are assigned.
pub struct NodeInput {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub repo: String,
    /// "derived" | "asserted" (lowercase — the renderer compares literally).
    pub plane: String,
    /// Trust-tier id: DerivedDeterministic | HumanConfirmed | AgentAsserted | InferredAdvisory.
    pub trust: String,
    /// Active | Superseded | Archived.
    pub status: String,
    /// bge embedding if the node carries one (drives PCA).
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneNode {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub repo: String,
    pub plane: String,
    pub trust: String,
    /// The constellation groups nodes into trust-tier lanes; lane == trust tier.
    pub lane: String,
    pub status: String,
    /// 3-D coordinate the renderer maps over. REQUIRED — its absence crashes the client.
    pub pos: [f32; 3],
    /// Edge count touching this node (in + out, among the scene's edges).
    pub degree: u32,
    pub contradicted: bool,
    /// Always true here — these positions are server-projected (not client-laid).
    pub projected: bool,
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
    pub org: String,
    pub projection: &'static str,
    pub node_count: usize,
    pub edge_count: usize,
    pub nodes: Vec<SceneNode>,
    pub edges: Vec<SceneEdge>,
}

const TITLE_MAX: usize = 140;
/// Minimum embedded nodes before PCA is worth it; below this we ring-lay.
const PCA_MIN: usize = 3;
const POWER_ITERS: usize = 64;

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Build a scene from already-fetched nodes + edges. Pure + deterministic.
/// `edges` are `(from, to, kind, quarantined)` with ids referring to `nodes`.
pub fn build_scene(
    org: &str,
    nodes: Vec<NodeInput>,
    edges: Vec<(String, String, String, bool)>,
) -> Scene {
    let dim = embedding_dim(&nodes);
    let embedded = nodes.iter().filter(|n| has_dim(n, dim)).count();
    let use_pca = dim.is_some() && embedded >= PCA_MIN;

    let coords: Vec<[f32; 3]> = if use_pca {
        pca_coords(&nodes, dim.unwrap_or(0))
    } else {
        ring_coords(nodes.len())
    };
    let projection = if use_pca { "pca-3d" } else { "ring" };

    let ids: std::collections::HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let kept: Vec<(String, String, String, bool)> = edges
        .into_iter()
        .filter(|(f, t, _, _)| ids.contains(f.as_str()) && ids.contains(t.as_str()))
        .collect();

    // degree (in+out) and contradiction flag, derived from the kept edges.
    let mut degree: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut contradicted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (f, t, kind, _) in &kept {
        *degree.entry(f.as_str()).or_insert(0) += 1;
        *degree.entry(t.as_str()).or_insert(0) += 1;
        if kind == "Contradicts" || kind == "Refutes" {
            contradicted.insert(f.as_str());
            contradicted.insert(t.as_str());
        }
    }

    let scene_edges: Vec<SceneEdge> = kept
        .iter()
        .map(|(from, to, kind, q)| SceneEdge {
            from: from.clone(),
            to: to.clone(),
            kind: kind.clone(),
            quarantined: *q,
        })
        .collect();

    let scene_nodes: Vec<SceneNode> = nodes
        .into_iter()
        .zip(coords)
        .map(|(n, pos)| {
            let deg = degree.get(n.id.as_str()).copied().unwrap_or(0);
            let contra = contradicted.contains(n.id.as_str());
            SceneNode {
                lane: n.trust.clone(),
                title: truncate(&n.title, TITLE_MAX),
                kind: n.kind,
                repo: n.repo,
                plane: n.plane,
                trust: n.trust,
                status: n.status,
                pos,
                degree: deg,
                contradicted: contra,
                projected: true,
                id: n.id,
            }
        })
        .collect();

    Scene {
        org: org.to_string(),
        projection,
        node_count: scene_nodes.len(),
        edge_count: scene_edges.len(),
        nodes: scene_nodes,
        edges: scene_edges,
    }
}

fn embedding_dim(nodes: &[NodeInput]) -> Option<usize> {
    nodes
        .iter()
        .find_map(|n| n.embedding.as_ref().map(|v| v.len()))
        .filter(|d| *d > 0)
}

fn has_dim(n: &NodeInput, dim: Option<usize>) -> bool {
    match (n.embedding.as_ref(), dim) {
        (Some(v), Some(d)) => v.len() == d,
        _ => false,
    }
}

/// Deterministic 3-D ring layout, used when PCA isn't viable.
fn ring_coords(n: usize) -> Vec<[f32; 3]> {
    if n == 0 {
        return Vec::new();
    }
    let two_pi = std::f32::consts::TAU;
    (0..n)
        .map(|i| {
            let theta = two_pi * (i as f32) / (n as f32);
            // spread z slightly so a ring isn't perfectly coplanar
            let z = (i as f32) / (n as f32) - 0.5;
            [theta.cos(), theta.sin(), z]
        })
        .collect()
}

/// PCA-3D via covariance power iteration with deflation (3 components). Nodes
/// lacking a same-dim embedding ring-place by index so the scene stays complete.
fn pca_coords(nodes: &[NodeInput], dim: usize) -> Vec<[f32; 3]> {
    let rows: Vec<&Vec<f32>> = nodes
        .iter()
        .filter_map(|n| n.embedding.as_ref())
        .filter(|v| v.len() == dim)
        .collect();
    let mut mean = vec![0f32; dim];
    for r in &rows {
        for (m, v) in mean.iter_mut().zip(r.iter()) {
            *m += *v;
        }
    }
    let inv = 1.0 / (rows.len() as f32);
    for m in &mut mean {
        *m *= inv;
    }

    let pc1 = principal_component(&rows, &mean, dim, &[]);
    let pc2 = principal_component(&rows, &mean, dim, &[&pc1]);
    let pc3 = principal_component(&rows, &mean, dim, &[&pc1, &pc2]);

    let ring = ring_coords(nodes.len());
    let mut raw: Vec<[f32; 3]> = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        match n.embedding.as_ref() {
            Some(v) if v.len() == dim => raw.push([
                dot_centered(v, &mean, &pc1),
                dot_centered(v, &mean, &pc2),
                dot_centered(v, &mean, &pc3),
            ]),
            _ => raw.push(ring[i]),
        }
    }
    normalize(&mut raw);
    raw
}

fn dot_centered(v: &[f32], mean: &[f32], axis: &[f32]) -> f32 {
    v.iter()
        .zip(mean)
        .zip(axis)
        .map(|((vi, mi), ai)| (vi - mi) * ai)
        .sum()
}

/// One principal component by power iteration on the covariance operator,
/// applied implicitly. Any `deflate` components are projected out each step.
fn principal_component(
    rows: &[&Vec<f32>],
    mean: &[f32],
    dim: usize,
    deflate: &[&Vec<f32>],
) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for (i, vi) in v.iter_mut().enumerate() {
        *vi = if i % 2 == 0 { 1.0 } else { -1.0 };
    }
    for d in deflate {
        project_out(&mut v, d);
    }
    normalize_vec(&mut v);

    for _ in 0..POWER_ITERS {
        let mut next = vec![0f32; dim];
        for r in rows {
            let mut c = 0f32;
            for k in 0..dim {
                c += (r[k] - mean[k]) * v[k];
            }
            for k in 0..dim {
                next[k] += c * (r[k] - mean[k]);
            }
        }
        for d in deflate {
            project_out(&mut next, d);
        }
        if normalize_vec(&mut next) {
            v = next;
        } else {
            break;
        }
    }
    v
}

fn project_out(v: &mut [f32], axis: &[f32]) {
    let proj: f32 = v.iter().zip(axis).map(|(a, b)| a * b).sum();
    for (vi, ai) in v.iter_mut().zip(axis) {
        *vi -= proj * ai;
    }
}

fn normalize_vec(v: &mut [f32]) -> bool {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return false;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    true
}

/// Scale points into roughly [-1, 1] per axis (max-abs).
fn normalize(points: &mut [[f32; 3]]) {
    let mut max = [0f32; 3];
    for p in points.iter() {
        for k in 0..3 {
            max[k] = max[k].max(p[k].abs());
        }
    }
    let s: [f32; 3] = [
        if max[0] > f32::EPSILON { 1.0 / max[0] } else { 1.0 },
        if max[1] > f32::EPSILON { 1.0 / max[1] } else { 1.0 },
        if max[2] > f32::EPSILON { 1.0 / max[2] } else { 1.0 },
    ];
    for p in points.iter_mut() {
        for k in 0..3 {
            p[k] *= s[k];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &str, emb: Option<Vec<f32>>) -> NodeInput {
        NodeInput {
            id: id.into(),
            title: "t".into(),
            kind: "doc".into(),
            repo: "r".into(),
            plane: "derived".into(),
            trust: "DerivedDeterministic".into(),
            status: "Active".into(),
            embedding: emb,
        }
    }

    #[test]
    fn empty_scene_is_empty() {
        let s = build_scene("o", vec![], vec![]);
        assert_eq!(s.node_count, 0);
        assert_eq!(s.projection, "ring");
    }

    #[test]
    fn ring_when_no_embeddings_has_3d_pos() {
        let s = build_scene("o", vec![n("a", None), n("b", None), n("c", None)], vec![]);
        assert_eq!(s.node_count, 3);
        assert_eq!(s.projection, "ring");
        for nd in &s.nodes {
            assert!(nd.pos.iter().all(|c| c.is_finite()));
            assert!(nd.projected);
        }
    }

    #[test]
    fn pca_3d_degree_and_edges() {
        let nodes = vec![
            n("a", Some(vec![1.0, 0.0, 0.0])),
            n("b", Some(vec![0.0, 1.0, 0.0])),
            n("c", Some(vec![0.0, 0.0, 1.0])),
            n("d", Some(vec![1.0, 1.0, 0.0])),
        ];
        let edges = vec![
            ("a".into(), "b".into(), "TemporalNext".into(), false),
            ("a".into(), "c".into(), "Contradicts".into(), false),
            ("a".into(), "zzz".into(), "DependsOn".into(), false), // dangling -> dropped
        ];
        let s = build_scene("o", nodes, edges);
        assert_eq!(s.node_count, 4);
        assert_eq!(s.projection, "pca-3d");
        assert_eq!(s.edge_count, 2, "dangling edge filtered");
        let a = s.nodes.iter().find(|x| x.id == "a").unwrap();
        assert_eq!(a.degree, 2, "a touches two kept edges");
        assert!(a.contradicted, "a is endpoint of a Contradicts edge");
        for nd in &s.nodes {
            assert!(nd.pos.iter().all(|c| c.is_finite() && c.abs() <= 1.0001));
        }
    }
}
