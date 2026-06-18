/**
 * Memrizz — Constellation engine (lightweight 2.5D canvas), ported 1:1 from the
 * prototype's constellation.js. Orbit camera, perspective projection, baked glow
 * sprites, additive nodes, depth fog, four morphing layouts,
 * time-morph, fly-to, hit-testing, lasso. No WebGL — tuned for ~900 nodes.
 *
 * Decoupled from the prototype's global `MEM`: it takes an injected {@link Graph}
 * (built from the gateway scene by graph.ts). The projection + layout math are
 * exported as pure functions for unit tests; the class uses them.
 */
import { TRUST, laneColor } from "@/lib/data/taxonomy";
import { ccKey, hexRGB, tenantColors } from "./graph";
import type {
  Encoding, EngineFilters, EngineNode, EngineOptions, Graph, LayoutMode, Vec3,
} from "./types";

const TAU = Math.PI * 2;
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const clamp = (v: number, a: number, b: number) => (v < a ? a : v > b ? b : v);

export interface Camera {
  yaw: number; pitch: number; dist: number; zoom: number;
  tyaw: number; tpitch: number; tdist: number; tzoom: number;
}

export interface Projection {
  sx: number;
  sy: number;
  depth: number;
  scale: number;
}

export const EDGE_STYLE: Record<string, { color: string; a: number; w: number; dash: number[] | null }> = {
  TemporalNext: { color: "120,150,90", a: 0.16, w: 1, dash: null },
  Supersedes: { color: "255,189,16", a: 0.34, w: 1.4, dash: null },
  DependsOn: { color: "95,168,230", a: 0.22, w: 1.1, dash: null },
  AnalogousTo: { color: "181,140,255", a: 0.3, w: 1.1, dash: [3, 5] },
  Contradicts: { color: "210,60,40", a: 0.42, w: 1.4, dash: [2, 3] },
  References: { color: "120,140,120", a: 0.1, w: 1, dash: null },
};

/** Pure perspective projection (yaw/pitch orbit). Returns null when behind camera. */
export function projectPoint(
  cam: Camera,
  focal: number,
  scaleBase: number,
  center: { x: number; y: number },
  p: Vec3,
): Projection | null {
  const cy = Math.cos(cam.yaw), sy = Math.sin(cam.yaw);
  const cp = Math.cos(cam.pitch), sp = Math.sin(cam.pitch);
  const x1 = p.x * cy - p.z * sy;
  const z1 = p.x * sy + p.z * cy;
  const y1 = p.y * cp - z1 * sp;
  const z2 = p.y * sp + z1 * cp;
  const depth = z2 + cam.dist;
  if (depth <= 0.2) return null;
  const f = (focal / depth) * cam.zoom * scaleBase;
  return { sx: center.x + x1 * f, sy: center.y - y1 * f, depth, scale: f / 40 };
}

/** Pure layout: target position for one node in a given mode. */
export function layoutTarget(mode: LayoutMode, n: EngineNode, graph: Graph): Vec3 {
  const LG = 2.55, TIME_H = 6.4, TR_G = 1.7;
  const TN = graph.tenants.length;
  if (mode === "lattice") {
    const jx = (n.seed - 0.5) * 0.78 + (n.tenantIdx - TN / 2) * 0.04;
    const jz = n.sx * 0.5;
    return { x: (n.laneIdx - 2.5) * LG + jx, y: (n.tf - 0.5) * TIME_H, z: (n.trustIdx - 1.5) * TR_G + jz };
  }
  if (mode === "galaxy") {
    const cc = graph.clusterCenter[ccKey(n.lane, n.tenantIdx)] ?? { x: 0, y: 0, z: 0 };
    return { x: cc.x * 6.4 + n.sx * 0.9, y: cc.y * 5.6 + n.sy * 0.9, z: cc.z * 6.0 + n.sz * 0.9 };
  }
  if (mode === "islands") {
    const i = n.tenantIdx;
    const ang = (i / Math.max(1, TN)) * TAU, rad = 6.6;
    const c = { x: Math.cos(ang) * rad, y: (i % 2 ? 0.7 : -0.7) + Math.sin(ang * 2) * 0.5, z: Math.sin(ang) * rad };
    const a = n.seed * TAU, r = 0.5 + n.sx * 0.9 + n.sizeW * 0.3;
    return { x: c.x + Math.cos(a) * r, y: c.y + (n.tf - 0.5) * 2.2 + n.sy * 0.5, z: c.z + Math.sin(a) * r };
  }
  // river
  const t = n.tf;
  return {
    x: Math.sin(t * Math.PI * 2.4) * 4.2 + (n.laneIdx - 2.5) * 0.5,
    y: (n.laneIdx - 2.5) * 0.95 + n.sy * 0.4,
    z: (t - 0.5) * 13 + n.sx * 0.6,
  };
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

interface EngineState {
  mode: LayoutMode; encoding: Encoding; bloom: number;
  field: string; timeT: number; motion: boolean; reduced: boolean;
  filters: EngineFilters; selectedId: string | null; hoverId: string | null; focusRadius: number;
}

export class ConstellationEngine {
  cv: HTMLCanvasElement;
  ctx: CanvasRenderingContext2D;
  opts: EngineOptions;
  dpr: number;
  graph: Graph;
  nodes: EngineNode[];
  edges: Graph["edges"];
  tenantColor: Record<string, string>;
  state: EngineState;
  cam: Camera;
  focal = 9;
  center = { x: 0, y: 0 };
  spriteCache: Record<string, HTMLCanvasElement> = {};
  haloSprite: HTMLCanvasElement;
  fly: { p: number; dur: number; fromYaw: number; toYaw: number; fromPitch: number; toPitch: number; fromDist: number; toDist: number } | null = null;
  _t = 0;
  cited: Set<string> | null = null;
  focusHops = 1;
  analogues: { from: string; to: string[]; born: number } | null = null;
  mini: CanvasRenderingContext2D | null = null;
  miniCv: HTMLCanvasElement | null = null;
  marquee: Set<string> | null = null;
  lassoMode = false;
  w = 2; h = 2;
  _scaleBase = 1;
  _proj: (Projection | null)[] = [];
  _order: number[] | null = null;
  _neighSet: Set<string> | null = null;
  _focusEase = 0;
  _light = false;
  _neb: HTMLCanvasElement | null = null;
  _nebField = "";
  _dragging = false;
  _last = 0;
  _lastFrame = 0;
  _frameN = 0;
  private _raf = 0;
  private _watch: ReturnType<typeof setInterval> | null = null;
  private _lasso: { x0: number; y0: number; x1: number; y1: number; add: boolean } | null = null;
  private _lx = 0; private _ly = 0; private _moved = 0;

  constructor(canvas: HTMLCanvasElement, graph: Graph, opts: EngineOptions = {}) {
    this.cv = canvas;
    this.ctx = canvas.getContext("2d")!;
    this.opts = opts;
    this.dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.graph = graph;
    this.nodes = graph.nodes;
    this.edges = graph.edges;
    this.tenantColor = tenantColors(graph.tenants);
    this.state = {
      mode: "lattice", encoding: "kind", bloom: 0.7,
      field: "#0a1810", timeT: 1, motion: true, reduced: false,
      filters: { tenants: null, lanes: null, trusts: null, statuses: null, showQuarantined: true },
      selectedId: null, hoverId: null, focusRadius: 0,
    };
    this.cam = { yaw: 0.5, pitch: -0.32, dist: 15.2, zoom: 1, tyaw: 0.5, tpitch: -0.32, tdist: 15.2, tzoom: 1 };
    this.haloSprite = this._bakeHalo();

    this._initPositions();
    this._computeLayout("lattice", true);
    this._bindEvents();
    this._resize();
    window.addEventListener("resize", this._resize);
    this._loop = this._loop.bind(this);
    this._raf = requestAnimationFrame(this._loop);
    try { this._update(0.016); this._render(); } catch { /* first paint best-effort */ }
    this._lastFrame = performance.now();
    this._watch = setInterval(() => {
      if (performance.now() - this._lastFrame > 200) this._loop(performance.now());
    }, 120);
    opts.onReady?.(this);
  }

  // ---------- sprite baking ----------
  private _bakeHalo(): HTMLCanvasElement {
    const s = 128, c = document.createElement("canvas");
    c.width = c.height = s;
    const x = c.getContext("2d")!;
    const g = x.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
    g.addColorStop(0, "rgba(255,255,255,0.9)");
    g.addColorStop(0.18, "rgba(255,255,255,0.45)");
    g.addColorStop(0.5, "rgba(255,255,255,0.12)");
    g.addColorStop(1, "rgba(255,255,255,0)");
    x.fillStyle = g;
    x.fillRect(0, 0, s, s);
    return c;
  }
  private _glow(color: string): HTMLCanvasElement {
    if (this.spriteCache[color]) return this.spriteCache[color];
    const [r, g, b] = hexRGB(color);
    const s = 64, c = document.createElement("canvas");
    c.width = c.height = s;
    const x = c.getContext("2d")!;
    const grad = x.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
    grad.addColorStop(0, `rgba(255,255,255,0.62)`);
    grad.addColorStop(0.22, `rgba(${Math.min(255, r + 55)},${Math.min(255, g + 55)},${Math.min(255, b + 55)},0.52)`);
    grad.addColorStop(0.5, `rgba(${r},${g},${b},0.26)`);
    grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
    x.fillStyle = grad;
    x.fillRect(0, 0, s, s);
    this.spriteCache[color] = c;
    return c;
  }

  // ---------- layouts ----------
  private _initPositions() {
    this.nodes.forEach((n) => { n.pos = { x: 0, y: 0, z: 0 }; n.tpos = { x: 0, y: 0, z: 0 }; n._set = false; });
  }
  private _computeLayout(mode: LayoutMode, instant: boolean) {
    this.nodes.forEach((n) => {
      n.tpos = layoutTarget(mode, n, this.graph);
      if (instant || !n._set) { n.pos = { ...n.tpos }; n._set = true; }
    });
  }

  setState(p: Partial<EngineState>) {
    const prevMode = this.state.mode;
    Object.assign(this.state, p);
    if (p.filters) this.state.filters = Object.assign({}, this.state.filters, p.filters);
    if (p.mode && p.mode !== prevMode) this._computeLayout(p.mode, false);
  }

  // ---------- filter / effective state ----------
  private _passes(n: EngineNode): boolean {
    const f = this.state.filters;
    if (f.tenants && !f.tenants.has(n.tenant)) return false;
    if (f.lanes && !f.lanes.has(n.lane)) return false;
    if (f.trusts && !f.trusts.has(n.trust)) return false;
    if (f.statuses && !f.statuses.has(n.status)) return false;
    if (!f.showQuarantined && n.trust === "InferredAdvisory") return false;
    return true;
  }
  private _cutoffMs() { return this.graph.t0 + this.state.timeT * this.graph.span; }
  private _effStatus(n: EngineNode, cutoff: number): string {
    if (n.when > cutoff) return "future";
    if (n.status === "Superseded" && n.supersededBy) {
      const sup = this.graph.byId[n.supersededBy];
      if (sup && sup.when > cutoff) return "Active";
    }
    return n.status;
  }

  // ---------- color ----------
  private _nodeColor(n: EngineNode): string {
    const e = this.state.encoding;
    if (e === "trust") return TRUST[n.trustIdx]?.ring ?? "#cfe9b0";
    if (e === "tenant") return this.tenantColor[n.tenant] ?? "#8ecc09";
    return laneColor(n.lane);
  }

  private _project(p: Vec3): Projection | null {
    return projectPoint(this.cam, this.focal, this._scaleBase, this.center, p);
  }

  // ---------- main loop ----------
  private _loop(ts: number) {
    this._lastFrame = performance.now();
    this._raf = requestAnimationFrame(this._loop);
    const dt = Math.min(0.05, (ts - (this._last || ts)) / 1000);
    this._last = ts;
    this._t += dt;
    this._update(dt);
    this._render();
  }

  private _update(dt: number) {
    const c = this.cam, st = this.state;
    if (this.fly) {
      this.fly.p = clamp(this.fly.p + dt / this.fly.dur, 0, 1);
      const e = 1 - Math.pow(1 - this.fly.p, 3);
      c.tyaw = lerp(this.fly.fromYaw, this.fly.toYaw, e);
      c.tpitch = lerp(this.fly.fromPitch, this.fly.toPitch, e);
      c.tdist = lerp(this.fly.fromDist, this.fly.toDist, e);
      if (this.fly.p >= 1) this.fly = null;
    } else if (st.motion && !st.reduced && !this._dragging) {
      c.tyaw += dt * 0.045;
    }
    c.yaw = lerp(c.yaw, c.tyaw, 0.12);
    c.pitch = lerp(c.pitch, c.tpitch, 0.12);
    c.dist = lerp(c.dist, c.tdist, 0.1);
    c.zoom = lerp(c.zoom, c.tzoom, 0.14);
    const N = this.nodes;
    for (let i = 0; i < N.length; i++) {
      const p = N[i].pos, t = N[i].tpos;
      p.x += (t.x - p.x) * 0.09; p.y += (t.y - p.y) * 0.09; p.z += (t.z - p.z) * 0.09;
    }
    this._focusEase = lerp(this._focusEase, st.selectedId ? 1 : 0, 0.1);
  }

  private _render() {
    const ctx = this.ctx, st = this.state;
    this._drawBackground(ctx, this.w, this.h);
    const cutoff = this._cutoffMs();
    const sel = st.selectedId ? this.graph.byId[st.selectedId] : null;
    const hov = st.hoverId ? this.graph.byId[st.hoverId] : null;
    const N = this.nodes;
    const P = this._proj.length === N.length ? this._proj : (this._proj = new Array(N.length));
    const neigh = this._neighSet;
    for (let i = 0; i < N.length; i++) {
      const n = N[i];
      const pr = this._project(n.pos);
      P[i] = pr;
      if (!pr) continue;
      n._eff = this._effStatus(n, cutoff);
      n._vis = this._passes(n);
    }

    ctx.globalCompositeOperation = "source-over";
    this._drawEdges(ctx, sel);

    const fb = hexRGB(st.field);
    const light = (this._light = (0.299 * fb[0] + 0.587 * fb[1] + 0.114 * fb[2]) / 255 > 0.55);
    ctx.globalCompositeOperation = light ? "source-over" : "lighter";
    const order = this._order && this._order.length === N.length ? this._order : (this._order = N.map((_, i) => i));
    order.sort((a, b) => (P[b] ? P[b]!.depth : 0) - (P[a] ? P[a]!.depth : 0));
    const bloomK = st.bloom;
    let bloomBudget = st.bloom < 0.4 ? 36 : 150;

    for (let k = 0; k < order.length; k++) {
      const i = order[k], n = N[i], pr = P[i];
      if (!pr || n._eff === "future") continue;
      const depthA = clamp((this.cam.dist + 7 - pr.depth) / 13, 0.12, 1);
      let baseA = depthA * 0.72;
      const color = this._nodeColor(n);
      let tint = color;
      if (n._eff === "Superseded") { baseA *= 0.4; tint = "#7d8a74"; }
      else if (n._eff === "Archived") baseA *= 0.28;
      if (n.trust === "InferredAdvisory") baseA *= 0.5 + 0.32 * (0.5 + 0.5 * Math.sin(this._t * 5 + n.seed * 30));
      else if (n.trust === "AgentAsserted") baseA *= 0.82;
      if (!n._vis) baseA *= 0.07;
      if (sel && this._focusEase > 0.01) {
        const isFocus = n.id === sel.id || (neigh && neigh.has(n.id));
        if (!isFocus) baseA *= lerp(1, 0.12, this._focusEase);
      }
      const r = (2.5 + n.sizeW * 4.7) * pr.scale * (sel && n.id === sel.id ? 1.5 : 1);
      if (r < 0.25) continue;

      if (light) {
        ctx.globalAlpha = clamp(baseA, 0, 1);
        ctx.fillStyle = tint;
        ctx.beginPath(); ctx.arc(pr.sx, pr.sy, Math.max(1, r * 0.62), 0, TAU); ctx.fill();
        if (n.belnap && n._eff === "Active") { ctx.fillStyle = "#d2381f"; ctx.globalAlpha = clamp(baseA * 0.7, 0, 1); ctx.beginPath(); ctx.arc(pr.sx, pr.sy, Math.max(1, r * 0.4), 0, TAU); ctx.fill(); }
        if (sel && n.id === sel.id) { ctx.globalAlpha = 1; ctx.strokeStyle = "#0e0f0c"; ctx.lineWidth = 1.5; ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r * 1.3 + 4, 0, TAU); ctx.stroke(); }
        n._sx = pr.sx; n._sy = pr.sy; n._r = r;
        continue;
      }

      const wantBloom = n.trust === "DerivedDeterministic" || n.trust === "HumanConfirmed" || n.id === sel?.id || n.id === hov?.id;
      if (wantBloom && bloomBudget > 0 && baseA > 0.2 && bloomK > 0.05) {
        bloomBudget--;
        const hr = r * (2.6 + bloomK * 1.6);
        ctx.globalAlpha = clamp(baseA * (0.06 + bloomK * 0.14) * (n.trust === "HumanConfirmed" ? 1.25 : 1), 0, 0.42);
        ctx.drawImage(this._glow(color), pr.sx - hr, pr.sy - hr, hr * 2, hr * 2);
      }
      ctx.globalAlpha = clamp(baseA, 0, 1);
      ctx.drawImage(this._glow(tint), pr.sx - r, pr.sy - r, r * 2, r * 2);

      if (this.cited?.has(n.id)) {
        ctx.globalCompositeOperation = "source-over";
        ctx.globalAlpha = 0.4 + 0.45 * (0.5 + 0.5 * Math.sin(this._t * 4 + n.seed * 5));
        ctx.strokeStyle = "#ffbd10"; ctx.lineWidth = 1.5;
        ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r * 1.5 + 5, 0, TAU); ctx.stroke();
        ctx.globalCompositeOperation = "lighter";
      }
      if (n.belnap && n._eff === "Active") {
        const gx = Math.sin(this._t * 22 + n.seed * 10) * r * 0.5;
        ctx.globalAlpha = clamp(baseA * 0.5, 0, 0.6);
        ctx.drawImage(this._glow("#e23a28"), pr.sx - r * 1.2 + gx, pr.sy - r * 1.2, r * 2.4, r * 2.4);
      }
      if (sel && n.id === sel.id) {
        ctx.globalCompositeOperation = "source-over";
        ctx.globalAlpha = 0.95; ctx.strokeStyle = "#ffffff"; ctx.lineWidth = 1.5;
        ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r * 1.7 + 4, 0, TAU); ctx.stroke();
        ctx.globalCompositeOperation = "lighter";
      }
      n._sx = pr.sx; n._sy = pr.sy; n._r = r;
    }

    this._drawAnalogues(ctx);
    ctx.globalCompositeOperation = "source-over";
    this._drawLabels(ctx, sel, hov);
    if (this.mini && (this._frameN = (this._frameN + 1) % 4) === 0) this._drawMinimap();
    this._drawMarquee(ctx);
    ctx.globalAlpha = 1;
  }

  private _drawBackground(ctx: CanvasRenderingContext2D, W: number, H: number) {
    const st = this.state;
    ctx.globalCompositeOperation = "source-over";
    ctx.fillStyle = st.field;
    ctx.fillRect(0, 0, W, H);
    if (!this._neb || this._nebField !== st.field) this._bakeNebula();
    const px = (this.cam.yaw % TAU) * 40;
    ctx.globalAlpha = 1;
    if (this._neb) ctx.drawImage(this._neb, -px * 0.2 - 60, -40, W + 200, H + 120);
  }
  private _bakeNebula() {
    const st = this.state;
    const c = document.createElement("canvas");
    c.width = this.w + 200; c.height = this.h + 120;
    const x = c.getContext("2d")!;
    const dark = st.field === "#f4f1ea";
    const blobs: [number, number, string][] = dark
      ? [[0.28, 0.3, "rgba(142,204,9,0.05)"], [0.72, 0.62, "rgba(255,189,16,0.05)"], [0.5, 0.5, "rgba(120,120,110,0.04)"]]
      : [[0.26, 0.32, "rgba(26,80,52,0.55)"], [0.74, 0.66, "rgba(20,60,72,0.4)"], [0.55, 0.2, "rgba(60,60,30,0.25)"]];
    blobs.forEach(([fx, fy, col]) => {
      const g = x.createRadialGradient(fx * c.width, fy * c.height, 0, fx * c.width, fy * c.height, c.width * 0.5);
      g.addColorStop(0, col); g.addColorStop(1, "rgba(0,0,0,0)");
      x.fillStyle = g; x.fillRect(0, 0, c.width, c.height);
    });
    if (!dark) {
      let s = 99;
      const rnd = () => (s = (s * 16807) % 2147483647) / 2147483647;
      for (let i = 0; i < 220; i++) {
        const a = 0.04 + rnd() * 0.18, rr = rnd() * 1.3;
        x.fillStyle = `rgba(205,231,214,${a})`;
        x.beginPath(); x.arc(rnd() * c.width, rnd() * c.height, rr, 0, TAU); x.fill();
      }
    }
    this._neb = c; this._nebField = st.field;
  }

  private _drawEdges(ctx: CanvasRenderingContext2D, sel: EngineNode | null) {
    const P = this._proj, st = this.state;
    const reduce = st.bloom < 0.4;
    for (let i = 0; i < this.edges.length; i++) {
      const e = this.edges[i];
      const a = this.nodes[e.ai], b = this.nodes[e.bi];
      if (a._eff === "future" || b._eff === "future") continue;
      const pa = P[e.ai], pb = P[e.bi];
      if (!pa || !pb) continue;
      if (e.quarantined && !st.filters.showQuarantined) continue;
      const stl = EDGE_STYLE[e.kind] || EDGE_STYLE.References;
      let alpha = stl.a;
      let focusEdge = false;
      if (sel && this._focusEase > 0.05) {
        focusEdge = e.a === sel.id || e.b === sel.id;
        alpha *= focusEdge ? 1.7 : lerp(1, 0.1, this._focusEase);
      }
      if (!a._vis || !b._vis) alpha *= 0.12;
      if (reduce && !focusEdge && stl.a < 0.2) continue;
      const depthA = clamp((this.cam.dist + 9 - (pa.depth + pb.depth) / 2) / 16, 0.1, 1);
      ctx.globalAlpha = clamp(alpha * depthA, 0, 0.9);
      ctx.strokeStyle = `rgb(${stl.color})`;
      ctx.lineWidth = stl.w * (focusEdge ? 1.6 : 1);
      if (e.quarantined) { ctx.setLineDash([2, 4]); ctx.globalAlpha *= 0.7 + 0.3 * Math.sin(this._t * 4 + e.ai); }
      else if (stl.dash) ctx.setLineDash(stl.dash);
      else ctx.setLineDash([]);
      ctx.beginPath(); ctx.moveTo(pa.sx, pa.sy); ctx.lineTo(pb.sx, pb.sy); ctx.stroke();
    }
    ctx.setLineDash([]);
  }

  private _drawLabels(ctx: CanvasRenderingContext2D, sel: EngineNode | null, hov: EngineNode | null) {
    const list: [EngineNode, boolean][] = [];
    if (sel && sel._sx != null && sel._eff !== "future") list.push([sel, true]);
    if (hov && hov !== sel && hov._sx != null && hov._eff !== "future") list.push([hov, false]);
    ctx.textBaseline = "middle";
    const light = this._light;
    list.forEach(([n, strong]) => {
      const x = n._sx!, y = n._sy!, r = n._r || 4;
      const label = n.title.length > 38 ? n.title.slice(0, 36) + "…" : n.title;
      const sub = n.kind + " · " + n.tenant;
      ctx.font = "500 12px Geist, system-ui, sans-serif";
      const w = Math.max(ctx.measureText(label).width, ctx.measureText(sub).width) + 18;
      const lx = x + r + 10, ly = y;
      ctx.globalAlpha = strong ? 0.96 : 0.9;
      ctx.fillStyle = light ? "rgba(250,248,243,0.95)" : "rgba(8,16,11,0.86)";
      roundRect(ctx, lx, ly - 19, w, 38, 6); ctx.fill();
      ctx.strokeStyle = light ? "rgba(14,15,12,0.16)" : "rgba(205,231,214,0.18)"; ctx.lineWidth = 1; ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.fillStyle = light ? "#0e0f0c" : "#f1efe8";
      ctx.font = "500 12px Geist, system-ui, sans-serif";
      ctx.fillText(label, lx + 9, ly - 6);
      ctx.fillStyle = this._nodeColor(n);
      ctx.font = "500 10px 'Geist Mono', monospace";
      ctx.fillText(sub.toUpperCase(), lx + 9, ly + 8);
    });
  }

  // ---------- interaction ----------
  private _bindEvents() {
    const cv = this.cv;
    cv.addEventListener("pointerdown", this._onDown);
    window.addEventListener("pointermove", this._onMove);
    window.addEventListener("pointerup", this._onUp);
    cv.addEventListener("wheel", this._onWheel, { passive: false });
    cv.addEventListener("pointerleave", () => { if (!this._dragging) this._setHover(null); });
  }
  private _onDown = (ev: PointerEvent) => {
    this.fly = null;
    const rect = this.cv.getBoundingClientRect();
    const mx = ev.clientX - rect.left, my = ev.clientY - rect.top;
    if (ev.shiftKey || this.lassoMode) {
      this._lasso = { x0: mx, y0: my, x1: mx, y1: my, add: ev.metaKey || ev.ctrlKey };
      this._dragging = false;
      this.cv.setPointerCapture?.(ev.pointerId);
      return;
    }
    this._dragging = true;
    this._lx = ev.clientX; this._ly = ev.clientY; this._moved = 0;
    this.cv.setPointerCapture?.(ev.pointerId);
  };
  private _onMove = (ev: PointerEvent) => {
    const rect = this.cv.getBoundingClientRect();
    const mx = ev.clientX - rect.left, my = ev.clientY - rect.top;
    if (this._lasso) { this._lasso.x1 = mx; this._lasso.y1 = my; this.cv.style.cursor = "crosshair"; return; }
    if (this._dragging) {
      const dx = ev.clientX - this._lx, dy = ev.clientY - this._ly;
      this._moved += Math.abs(dx) + Math.abs(dy);
      this.cam.tyaw += dx * 0.005; this.cam.yaw += dx * 0.005;
      this.cam.tpitch = clamp(this.cam.tpitch - dy * 0.005, -1.45, 1.45);
      this.cam.pitch = clamp(this.cam.pitch - dy * 0.005, -1.45, 1.45);
      this._lx = ev.clientX; this._ly = ev.clientY;
    } else {
      this._hitTest(mx, my, ev.clientX, ev.clientY);
    }
  };
  private _onUp = (ev: PointerEvent) => {
    if (this._lasso) {
      this._commitLasso();
      this._lasso = null;
      this.cv.style.cursor = this.lassoMode ? "crosshair" : "grab";
      return;
    }
    if (this._dragging && this._moved < 6) {
      const rect = this.cv.getBoundingClientRect();
      this._hitTest(ev.clientX - rect.left, ev.clientY - rect.top, ev.clientX, ev.clientY, true);
    }
    this._dragging = false;
  };
  private _onWheel = (ev: WheelEvent) => {
    ev.preventDefault();
    this.cam.tdist = clamp(this.cam.tdist * (1 + Math.sign(ev.deltaY) * 0.08), 5.5, 34);
  };

  setLassoMode(on: boolean) { this.lassoMode = on; this.cv.style.cursor = on ? "crosshair" : "grab"; }
  private _commitLasso() {
    const L = this._lasso, P = this._proj; if (!L || !P.length) return;
    const xa = Math.min(L.x0, L.x1) * this.dpr, xb = Math.max(L.x0, L.x1) * this.dpr;
    const ya = Math.min(L.y0, L.y1) * this.dpr, yb = Math.max(L.y0, L.y1) * this.dpr;
    if (xb - xa < 4 && yb - ya < 4) return;
    const set = L.add && this.marquee ? new Set(this.marquee) : new Set<string>();
    const cutoff = this._cutoffMs();
    for (let i = 0; i < this.nodes.length; i++) {
      const n = this.nodes[i], pr = P[i];
      if (!pr || n.when > cutoff || n._vis === false) continue;
      if (pr.sx >= xa && pr.sx <= xb && pr.sy >= ya && pr.sy <= yb) set.add(n.id);
    }
    this.marquee = set.size ? set : null;
    this.opts.onMarquee?.(this.marquee ? [...this.marquee] : []);
  }
  clearMarquee() { this.marquee = null; this.opts.onMarquee?.([]); }
  setMarquee(ids: string[]) { this.marquee = ids?.length ? new Set(ids) : null; this.opts.onMarquee?.(ids || []); }

  private _hitTest(mx: number, my: number, cx: number, cy: number, click?: boolean) {
    const P = this._proj; if (!P.length) return;
    let best: EngineNode | null = null, bestD = 1e9;
    for (let i = 0; i < this.nodes.length; i++) {
      const n = this.nodes[i], pr = P[i];
      if (!pr || n._eff === "future" || n._vis === false) continue;
      const dx = pr.sx - mx, dy = pr.sy - my, d = dx * dx + dy * dy;
      const hitR = Math.max(7, (n._r || 4) * 1.6);
      if (d < hitR * hitR && d < bestD) { bestD = d; best = n; }
    }
    if (click) this.select(best ? best.id : null);
    else this._setHover(best ? best.id : null, cx, cy);
    this.cv.style.cursor = best ? "pointer" : this._dragging ? "grabbing" : "grab";
  }
  private _setHover(id: string | null, cx = 0, cy = 0) {
    if (this.state.hoverId !== id) {
      this.state.hoverId = id;
      this.opts.onHover?.(id ? this.graph.byId[id] : null, cx, cy);
    } else if (id) {
      this.opts.onHover?.(this.graph.byId[id], cx, cy);
    }
  }
  select(id: string | null) {
    this.state.selectedId = id;
    this.analogues = null;
    this._neighSet = id ? this._computeNeigh(id, this.focusHops || 1) : null;
    if (id) this.flyTo(id);
    this.opts.onSelect?.(id ? this.graph.byId[id] : null);
  }
  private _computeNeigh(id: string, hops: number): Set<string> {
    const set = new Set([id]);
    let frontier = [id];
    for (let h = 0; h < hops; h++) {
      const next: string[] = [];
      frontier.forEach((nid) => (this.graph.adj[nid] || []).forEach((a) => { if (!set.has(a.o)) { set.add(a.o); next.push(a.o); } }));
      frontier = next;
    }
    return set;
  }
  setFocusHops(n: number) {
    this.focusHops = n;
    if (this.state.selectedId && !this.analogues) this._neighSet = this._computeNeigh(this.state.selectedId, n);
  }
  flyTo(id: string) {
    const n = this.graph.byId[id]; if (!n) return;
    const p = n.pos;
    const tyaw = Math.atan2(p.x, p.z);
    const horiz = Math.sqrt(p.x * p.x + p.z * p.z);
    const tpitch = clamp(-Math.atan2(p.y, horiz + 0.001) * 0.6 - 0.1, -1.2, 1.2);
    this.fly = {
      p: 0, dur: 0.9,
      fromYaw: this.cam.yaw, toYaw: -tyaw + 0.5,
      fromPitch: this.cam.pitch, toPitch: tpitch,
      fromDist: this.cam.dist, toDist: clamp(8.5, 6, 12),
    };
    this.cam.tdist = 9.2;
  }
  /** Beam in cross-repo analogues — explicit AnalogousTo edges, else same-kind
   *  cross-tenant nodes by degree (ported from the prototype). */
  showAnalogues(id: string) {
    const n = this.graph.byId[id];
    if (!n) return;
    let tos = (this.graph.adj[id] || [])
      .filter((a) => this.graph.edges[a.e]?.kind === "AnalogousTo")
      .map((a) => a.o);
    if (tos.length < 2) {
      tos = this.nodes
        .filter((m) => m.kind === n.kind && m.tenant !== n.tenant && m.id !== id)
        .sort((a, b) => b.deg - a.deg)
        .slice(0, 4)
        .map((m) => m.id);
    } else {
      tos = tos.slice(0, 5);
    }
    this.analogues = { from: id, to: tos, born: this._t };
    this.state.selectedId = id;
    this._neighSet = new Set([id, ...tos]);
    this.flyTo(id);
    this.opts.onSelect?.(n);
  }
  setCited(ids: string[]) {
    this.cited = ids?.length ? new Set(ids) : null;
    if (ids?.length) this.flyTo(ids[0]);
  }
  resetView() {
    this.cam.tyaw = 0.5; this.cam.tpitch = -0.32; this.cam.tdist = 15.2; this.cam.tzoom = 1;
  }
  setMinimap(canvas: HTMLCanvasElement | null) {
    this.miniCv = canvas;
    this.mini = canvas ? canvas.getContext("2d") : null;
  }
  private _drawMarquee(ctx: CanvasRenderingContext2D) {
    ctx.globalCompositeOperation = "source-over";
    if (this.marquee?.size) {
      const P = this._proj;
      ctx.strokeStyle = "rgba(255,189,16,0.9)"; ctx.lineWidth = 1.4;
      this.marquee.forEach((id) => {
        const n = this.graph.byId[id], pr = P[n.idx];
        if (!pr || n._eff === "future") return;
        const r = (n._r || 4) * 1.5 + 4;
        ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r, 0, TAU); ctx.stroke();
      });
    }
    if (this._lasso) {
      const L = this._lasso, d = this.dpr;
      const x = Math.min(L.x0, L.x1) * d, y = Math.min(L.y0, L.y1) * d;
      const w = Math.abs(L.x1 - L.x0) * d, h = Math.abs(L.y1 - L.y0) * d;
      ctx.fillStyle = "rgba(255,189,16,0.08)"; ctx.fillRect(x, y, w, h);
      ctx.strokeStyle = "rgba(255,189,16,0.85)"; ctx.lineWidth = 1.2; ctx.setLineDash([5, 4]);
      ctx.strokeRect(x, y, w, h); ctx.setLineDash([]);
    }
  }
  private _drawMinimap() {
    const ctx = this.mini, mc = this.miniCv; if (!ctx || !mc) return;
    const W = mc.width, H = mc.height, N = this.nodes;
    ctx.clearRect(0, 0, W, H);
    let minx = 1e9, maxx = -1e9, minz = 1e9, maxz = -1e9;
    for (const n of N) { const p = n.pos; if (p.x < minx) minx = p.x; if (p.x > maxx) maxx = p.x; if (p.z < minz) minz = p.z; if (p.z > maxz) maxz = p.z; }
    const pad = 9, s = Math.min((W - 2 * pad) / ((maxx - minx) || 1), (H - 2 * pad) / ((maxz - minz) || 1));
    const ox = pad + (W - 2 * pad - (maxx - minx) * s) / 2, oz = pad + (H - 2 * pad - (maxz - minz) * s) / 2;
    const map = (p: Vec3) => ({ x: ox + (p.x - minx) * s, y: oz + (p.z - minz) * s });
    const cutoff = this._cutoffMs();
    for (let i = 0; i < N.length; i += 2) { const n = N[i]; if (n.when > cutoff) continue; const m = map(n.pos); ctx.fillStyle = this._nodeColor(n); ctx.globalAlpha = 0.55; ctx.fillRect(m.x, m.y, 1.5, 1.5); }
    ctx.globalAlpha = 1;
    const sel = this.state.selectedId ? this.graph.byId[this.state.selectedId] : null;
    if (sel) { const m = map(sel.pos); ctx.strokeStyle = "#fff"; ctx.lineWidth = 1.2; ctx.beginPath(); ctx.arc(m.x, m.y, 4.5, 0, TAU); ctx.stroke(); }
    const cxp = W / 2, cyp = H / 2;
    ctx.strokeStyle = "rgba(142,204,9,0.8)"; ctx.lineWidth = 1.5;
    ctx.beginPath(); ctx.moveTo(cxp, cyp); ctx.lineTo(cxp + Math.sin(this.cam.yaw) * 15, cyp + Math.cos(this.cam.yaw) * 15); ctx.stroke();
    ctx.fillStyle = "rgba(142,204,9,0.8)"; ctx.beginPath(); ctx.arc(cxp, cyp, 2, 0, TAU); ctx.fill();
  }
  private _drawAnalogues(ctx: CanvasRenderingContext2D) {
    if (!this.analogues) return;
    const P = this._proj, from = this.graph.byId[this.analogues.from], pa = from && P[from.idx];
    if (!pa) return;
    const age = this._t - this.analogues.born;
    ctx.globalCompositeOperation = "lighter";
    this.analogues.to.forEach((tid, k) => {
      const tn = this.graph.byId[tid], pb = tn && P[tn.idx]; if (!pb) return;
      const prog = clamp((age - k * 0.14) / 0.55, 0, 1); if (prog <= 0) return;
      const mx = (pa.sx + pb.sx) / 2, my = (pa.sy + pb.sy) / 2;
      const dx = pb.sx - pa.sx, dy = pb.sy - pa.sy, len = Math.hypot(dx, dy) || 1;
      const off = Math.min(150, len * 0.32);
      const cx = mx - (dy / len) * off, cy = my + (dx / len) * off;
      const bez = (t: number) => ({ x: (1 - t) * (1 - t) * pa.sx + 2 * (1 - t) * t * cx + t * t * pb.sx, y: (1 - t) * (1 - t) * pa.sy + 2 * (1 - t) * t * cy + t * t * pb.sy });
      ctx.strokeStyle = "rgba(181,140,255," + 0.55 * prog + ")";
      ctx.lineWidth = 1.4; ctx.setLineDash([4, 7]); ctx.lineDashOffset = -this._t * 34;
      ctx.beginPath();
      const steps = 26, lim = Math.floor(steps * prog);
      for (let sct = 0; sct <= lim; sct++) { const b = bez(sct / steps); if (sct === 0) ctx.moveTo(b.x, b.y); else ctx.lineTo(b.x, b.y); }
      ctx.stroke(); ctx.setLineDash([]);
      const tt = ((this._t * 0.35 + k * 0.2) % 1) * prog, b = bez(tt);
      ctx.globalAlpha = 0.7 * prog; ctx.drawImage(this._glow("#b58cff"), b.x - 6, b.y - 6, 12, 12);
      ctx.globalAlpha = 1;
    });
    ctx.globalCompositeOperation = "source-over";
  }

  private _resize = () => {
    const parent = this.cv.parentElement;
    if (!parent) return;
    const r = parent.getBoundingClientRect();
    this.w = Math.max(2, r.width * this.dpr); this.h = Math.max(2, r.height * this.dpr);
    this.cv.width = this.w; this.cv.height = this.h;
    this.cv.style.width = r.width + "px"; this.cv.style.height = r.height + "px";
    this.center = { x: this.w / 2, y: this.h / 2 };
    this._scaleBase = Math.min(this.w, this.h) / 13;
    this._neb = null;
  };
  dispose() {
    cancelAnimationFrame(this._raf);
    if (this._watch) clearInterval(this._watch);
    window.removeEventListener("resize", this._resize);
    window.removeEventListener("pointermove", this._onMove);
    window.removeEventListener("pointerup", this._onUp);
  }
}
