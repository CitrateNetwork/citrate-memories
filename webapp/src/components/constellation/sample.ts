/**
 * A small DETERMINISTIC sample scene, used ONLY as a dev fallback when the gateway
 * is unreachable (so the constellation isn't a blank field with no backend). It is
 * clearly labelled "sample data" in the UI and is NOT org memory — the real scene
 * comes from `GET /api/orgs/[org]/layout`. Deterministic (seeded) so it's stable.
 */
import { LANES, TENANTS, TRUST } from "@/lib/data/taxonomy";
import type { Scene, SceneEdge, SceneNode } from "@/lib/gateway/types";

const KIND_BY_LANE: Record<string, string[]> = {
  code: ["Commit", "Pr"],
  docs: ["Doc", "Narrative", "Handoff"],
  specs: ["Adr", "Sprint", "WorkPackage", "Rationale"],
  config: ["ManifestChange", "PinBump", "DriftEvent"],
  audit: ["Audit", "Finding", "Benchmark", "Blocker"],
  claims: ["Claim", "AgentAction", "AnalogyHypothesis"],
};
const EDGE_KINDS = ["TemporalNext", "DependsOn", "Supersedes", "AnalogousTo", "Contradicts", "References"];

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function sampleScene(n = 200): Scene {
  const rand = mulberry32(0x6d6e656d); // "mnem"
  const pick = <T>(arr: T[]) => arr[(rand() * arr.length) | 0];
  const nodes: SceneNode[] = [];
  for (let i = 0; i < n; i++) {
    const lane = pick(LANES);
    const kind = pick(KIND_BY_LANE[lane.id]);
    const trust = pick(TRUST);
    const repo = pick(TENANTS);
    const status = rand() < 0.78 ? "active" : rand() < 0.6 ? "superseded" : "archived";
    nodes.push({
      id: `sample-${i.toString(16).padStart(4, "0")}`,
      pos: [(rand() - 0.5) * 12, (rand() - 0.5) * 9, (rand() - 0.5) * 12],
      projected: true,
      repo,
      kind,
      lane: lane.id,
      plane: trust.id === "DerivedDeterministic" ? "derived" : "asserted",
      trust: trust.id,
      status,
      contradicted: rand() < 0.05,
      degree: 0,
      title: `${kind.toLowerCase()} in ${repo}`,
    });
  }
  const edges: SceneEdge[] = [];
  const deg: Record<string, number> = {};
  for (let i = 0; i < n * 1.6; i++) {
    const a = nodes[(rand() * n) | 0], b = nodes[(rand() * n) | 0];
    if (a.id === b.id) continue;
    const kind = pick(EDGE_KINDS);
    edges.push({ from: a.id, to: b.id, kind, quarantined: kind === "AnalogousTo" && rand() < 0.6 });
    deg[a.id] = (deg[a.id] ?? 0) + 1;
    deg[b.id] = (deg[b.id] ?? 0) + 1;
  }
  for (const node of nodes) node.degree = deg[node.id] ?? 0;
  return { nodes, edges };
}
