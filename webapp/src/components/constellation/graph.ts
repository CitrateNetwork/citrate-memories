/**
 * Engine graph construction. The gateway `/layout` scene (server-positioned nodes
 * + edges) is mapped into the engine's node shape; the four client-side layout
 * modes (lattice/galaxy/islands/river) are then computed from each node's derived
 * indices + semantic jitter — exactly as the prototype did (planset §1.4).
 *
 * KNOWN DEGRADATION: SceneNode carries no `valid_from` yet, so `tf`/`when` are
 * derived deterministically from the node id (stable, but not real chronology).
 * When the gateway adds time to the scene, map it here — nothing else changes.
 */
import { LANE_IDX, TRUST_IDX, T0 as TAX_T0, span as TAX_SPAN } from "@/lib/data/taxonomy";
import type { Scene, SceneNode } from "@/lib/gateway/types";
import type { EngineEdge, EngineNode, Graph, Vec3 } from "./types";

export function hexRGB(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

export function hslHex(h: number, s: number, l: number): string {
  s /= 100;
  l /= 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  const to = (x: number) => Math.round(255 * x).toString(16).padStart(2, "0");
  return "#" + to(f(0)) + to(f(8)) + to(f(4));
}

/** Distinct muted hue per tenant (matches the prototype's TENANT_COLORS). */
export function tenantColors(tenants: string[]): Record<string, string> {
  const out: Record<string, string> = {};
  tenants.forEach((t, i) => {
    const h = ((i * 360) / Math.max(1, tenants.length) + 18) % 360;
    out[t] = hslHex(h, 52, 62);
  });
  return out;
}

/** Deterministic 0..1 from a string id (FNV-1a) — replaces the prototype's seed. */
export function seedFromId(id: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return ((h >>> 0) % 100000) / 100000;
}

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export const ccKey = (lane: string, tenantIdx: number) => `${lane}|${tenantIdx}`;

/** Galaxy cluster centers, one per lane×tenant — deterministic (seeded). */
function buildClusterCenters(lanes: string[], tenants: string[]): Record<string, Vec3> {
  const rand = mulberry32(0x6d656d); // "mem"
  const gauss = () => (rand() + rand() + rand() - 1.5) / 1.5;
  const out: Record<string, Vec3> = {};
  lanes.forEach((lane, li) => {
    tenants.forEach((_t, ti) => {
      const ang = rand() * Math.PI * 2;
      const rad = 0.45 + rand() * 0.55;
      out[ccKey(lane, ti)] = {
        x: Math.cos(ang) * rad + (li - 2.5) * 0.18,
        y: Math.sin(ang) * rad,
        z: gauss() * 0.7 + (ti - tenants.length / 2) * 0.06,
      };
    });
  });
  return out;
}

const LANE_KEYS = ["code", "docs", "specs", "config", "audit", "claims"];

/** Map a gateway scene into an engine graph. */
export function buildGraphFromScene(scene: Scene): Graph {
  const tenants = [...new Set(scene.nodes.map((n) => n.repo))].sort();
  const tenantIdx: Record<string, number> = Object.fromEntries(tenants.map((t, i) => [t, i]));
  const maxDeg = Math.max(1, ...scene.nodes.map((n) => n.degree));
  // normalize server positions into ~[-1,1] for use as within-cluster jitter
  const posScale =
    Math.max(1e-3, ...scene.nodes.flatMap((n) => n.pos.map((c) => Math.abs(c)))) || 1;

  const nodes: EngineNode[] = scene.nodes.map((s: SceneNode, idx) => {
    const seed = seedFromId(s.id);
    const tf = seed; // see KNOWN DEGRADATION above
    return {
      id: s.id,
      idx,
      kind: s.kind,
      lane: s.lane,
      laneIdx: LANE_IDX[s.lane] ?? 5,
      plane: s.plane,
      trust: s.trust,
      trustIdx: TRUST_IDX[s.trust] ?? 2,
      status: s.status.charAt(0).toUpperCase() + s.status.slice(1),
      tenant: s.repo,
      tenantIdx: tenantIdx[s.repo] ?? 0,
      title: s.title,
      tf,
      when: TAX_T0 + tf * TAX_SPAN,
      belnap: s.contradicted,
      deg: s.degree,
      sizeW: 0.45 + Math.pow(s.degree / maxDeg, 0.55),
      seed,
      sx: s.pos[0] / posScale,
      sy: s.pos[1] / posScale,
      sz: s.pos[2] / posScale,
      pos: { x: 0, y: 0, z: 0 },
      tpos: { x: 0, y: 0, z: 0 },
      _set: false,
    };
  });

  const byId: Record<string, EngineNode> = Object.fromEntries(nodes.map((n) => [n.id, n]));
  const adj: Record<string, { o: string; e: number }[]> = {};
  const edges: EngineEdge[] = [];
  scene.edges.forEach((e, ei) => {
    const a = byId[e.from];
    const b = byId[e.to];
    if (!a || !b) return;
    const idx = edges.length;
    edges.push({ a: e.from, b: e.to, ai: a.idx, bi: b.idx, kind: e.kind, quarantined: e.quarantined, e: idx });
    (adj[e.from] ??= []).push({ o: e.to, e: ei });
    (adj[e.to] ??= []).push({ o: e.from, e: ei });
  });

  return {
    nodes,
    edges,
    byId,
    adj,
    tenants,
    t0: TAX_T0,
    span: TAX_SPAN,
    clusterCenter: buildClusterCenters(LANE_KEYS, tenants),
  };
}
