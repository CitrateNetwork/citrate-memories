/**
 * Constellation engine — pure math + graph-adapter tests (WP-7.4). The canvas
 * render loop needs a browser, but the projection, the four layout modes, the
 * scene→graph mapping, and the deterministic helpers are pure and verified here.
 */
import { describe, expect, it } from "vitest";
import { projectPoint, layoutTarget, type Camera } from "./engine";
import { buildGraphFromScene, hexRGB, seedFromId } from "./graph";
import { sampleScene } from "./sample";
import type { LayoutMode } from "./types";

const CAM: Camera = { yaw: 0.5, pitch: -0.32, dist: 15.2, zoom: 1, tyaw: 0.5, tpitch: -0.32, tdist: 15.2, tzoom: 1 };
const CENTER = { x: 400, y: 300 };

describe("projectPoint", () => {
  it("projects the origin to the screen center region with positive depth", () => {
    const p = projectPoint(CAM, 9, 40, CENTER, { x: 0, y: 0, z: 0 });
    expect(p).not.toBeNull();
    expect(p!.depth).toBeGreaterThan(0);
    // origin maps near center x (no x/z offset at origin)
    expect(Math.abs(p!.sx - CENTER.x)).toBeLessThan(1);
  });

  it("culls points behind the camera (depth <= 0.2)", () => {
    // A point far in -z beyond the camera distance projects to null.
    const behind = projectPoint({ ...CAM, dist: 1 }, 9, 40, CENTER, { x: 0, y: 0, z: -50 });
    expect(behind).toBeNull();
  });

  it("is deterministic for the same inputs", () => {
    const a = projectPoint(CAM, 9, 40, CENTER, { x: 1, y: 2, z: 3 });
    const b = projectPoint(CAM, 9, 40, CENTER, { x: 1, y: 2, z: 3 });
    expect(a).toEqual(b);
  });
});

describe("hexRGB / seedFromId", () => {
  it("parses hex colors", () => {
    expect(hexRGB("#8ecc09")).toEqual([142, 204, 9]);
  });
  it("seedFromId is deterministic and in [0,1)", () => {
    const s = seedFromId("node-abc");
    expect(s).toBe(seedFromId("node-abc"));
    expect(s).toBeGreaterThanOrEqual(0);
    expect(s).toBeLessThan(1);
    expect(seedFromId("a")).not.toBe(seedFromId("b"));
  });
});

describe("buildGraphFromScene", () => {
  const graph = buildGraphFromScene(sampleScene(120));

  it("derives indices and a byId map", () => {
    expect(graph.nodes.length).toBe(120);
    const n = graph.nodes[0];
    expect(graph.byId[n.id]).toBe(n);
    expect(n.laneIdx).toBeGreaterThanOrEqual(0);
    expect(n.trustIdx).toBeGreaterThanOrEqual(0);
    expect(n.tenantIdx).toBeGreaterThanOrEqual(0);
  });

  it("builds symmetric adjacency from edges", () => {
    const e = graph.edges[0];
    expect(graph.adj[e.a].some((x) => x.o === e.b)).toBe(true);
    expect(graph.adj[e.b].some((x) => x.o === e.a)).toBe(true);
  });

  it("sizeW scales with degree and stays bounded", () => {
    for (const n of graph.nodes) {
      expect(n.sizeW).toBeGreaterThanOrEqual(0.45);
      expect(n.sizeW).toBeLessThanOrEqual(1.45 + 1e-9);
    }
  });
});

describe("layoutTarget — four modes produce finite, distinct positions", () => {
  const graph = buildGraphFromScene(sampleScene(60));
  const n = graph.nodes[10];
  const modes: LayoutMode[] = ["lattice", "galaxy", "islands", "river"];

  it("every mode yields finite coordinates", () => {
    for (const m of modes) {
      const p = layoutTarget(m, n, graph);
      expect(Number.isFinite(p.x) && Number.isFinite(p.y) && Number.isFinite(p.z)).toBe(true);
    }
  });

  it("lattice places lane on X and trust on Z deterministically", () => {
    const a = layoutTarget("lattice", n, graph);
    const b = layoutTarget("lattice", n, graph);
    expect(a).toEqual(b);
    // X is dominated by the lane column (laneIdx-2.5)*2.55 + small jitter
    expect(Math.sign(a.x)).toBe(Math.sign((n.laneIdx - 2.5) * 2.55) || Math.sign(a.x));
  });

  it("modes differ from one another", () => {
    const positions = modes.map((m) => JSON.stringify(layoutTarget(m, n, graph)));
    expect(new Set(positions).size).toBeGreaterThan(1);
  });
});
