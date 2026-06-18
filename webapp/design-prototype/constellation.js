/* =============================================================
   Memrizz — Constellation engine (lightweight 2.5D)
   Canvas renderer: orbit camera, perspective projection, baked
   glow sprites (fast), additive nodes, particle edge-flow, depth
   fog, four morphing layouts, time-morph, fly-to, hit-testing.
   No WebGL — tuned to stay smooth at ~900 nodes / ~1500 edges.
   window.Constellation
   ============================================================= */
(function () {
  const MEM = window.MEM;
  const TAU = Math.PI * 2;
  const lerp = (a, b, t) => a + (b - a) * t;
  const clamp = (v, a, b) => (v < a ? a : v > b ? b : v);

  // edge kind -> style
  const EDGE_STYLE = {
    TemporalNext: { color: "120,150,90",  a: 0.16, w: 1,   dash: null,    flow: 0.5 },
    Supersedes:   { color: "255,189,16",  a: 0.34, w: 1.4, dash: null,    flow: 1.0 },
    DependsOn:    { color: "95,168,230",  a: 0.22, w: 1.1, dash: null,    flow: 0.7 },
    AnalogousTo:  { color: "181,140,255", a: 0.30, w: 1.1, dash: [3, 5],  flow: 0.6 },
    Contradicts:  { color: "210,60,40",   a: 0.42, w: 1.4, dash: [2, 3],  flow: 0.9 },
    References:   { color: "120,140,120", a: 0.10, w: 1,   dash: null,    flow: 0.3 },
  };

  // tenant colors (distinct hues, muted to sit on dark)
  const TENANT_COLORS = {};
  MEM.TENANTS.forEach((t, i) => {
    const h = (i * 360 / MEM.TENANTS.length + 18) % 360;
    TENANT_COLORS[t] = hslHex(h, 52, 62);
  });
  function hslHex(h, s, l) {
    s /= 100; l /= 100;
    const k = (n) => (n + h / 30) % 12;
    const a = s * Math.min(l, 1 - l);
    const f = (n) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
    const to = (x) => Math.round(255 * x).toString(16).padStart(2, "0");
    return "#" + to(f(0)) + to(f(8)) + to(f(4));
  }
  function hexRGB(hex) {
    const h = hex.replace("#", "");
    return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
  }

  class Constellation {
    constructor(canvas, opts) {
      this.cv = canvas;
      this.ctx = canvas.getContext("2d");
      this.opts = opts || {};
      this.dpr = Math.min(window.devicePixelRatio || 1, 2);

      this.state = {
        mode: "lattice", encoding: "kind", bloom: 0.7, particles: 0.6,
        field: "#0a1810", timeT: 1, motion: true, reduced: false,
        filters: { tenants: null, lanes: null, trusts: null, statuses: null, showQuarantined: true },
        selectedId: null, hoverId: null, focusRadius: 0,
      };

      // camera
      this.cam = { yaw: 0.5, pitch: -0.32, dist: 15.2, zoom: 1, tyaw: 0.5, tpitch: -0.32, tdist: 15.2, tzoom: 1 };
      this.focal = 9;
      this.center = { x: 0, y: 0 };

      this.nodes = MEM.nodes;
      this.edges = MEM.edges;
      this.spriteCache = {};
      this.haloSprite = this._bakeHalo();
      this.fly = null;
      this._t = 0;
      this.cited = null;
      this.focusHops = 1;
      this.analogues = null;
      this.mini = null;
      this._dragging = false;
      this._raf = null;

      this._initPositions();
      this._computeLayout("lattice", true);
      this._bindEvents();
      this._resize();
      window.addEventListener("resize", this._resize);
      this._loop = this._loop.bind(this);
      this._raf = requestAnimationFrame(this._loop);
      // immediate first paint (don't wait on rAF, which can be paused when hidden)
      try { this._update(0.016); this._render(); } catch (e) {}
      // watchdog: if rAF is throttled/paused, pump via timer so it still animates
      this._lastFrame = performance.now();
      this._watch = setInterval(() => {
        if (performance.now() - this._lastFrame > 200) { this._loop(performance.now()); }
      }, 120);
      if (this.opts.onReady) this.opts.onReady(this);
    }

    // ---------- sprite baking ----------
    _bakeHalo() {
      const s = 128, c = document.createElement("canvas"); c.width = c.height = s;
      const x = c.getContext("2d"); const g = x.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
      g.addColorStop(0, "rgba(255,255,255,0.9)");
      g.addColorStop(0.18, "rgba(255,255,255,0.45)");
      g.addColorStop(0.5, "rgba(255,255,255,0.12)");
      g.addColorStop(1, "rgba(255,255,255,0)");
      x.fillStyle = g; x.fillRect(0, 0, s, s);
      return c;
    }
    _glow(color) {
      if (this.spriteCache[color]) return this.spriteCache[color];
      const [r, g, b] = hexRGB(color);
      const s = 64, c = document.createElement("canvas"); c.width = c.height = s;
      const x = c.getContext("2d");
      const grad = x.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
      grad.addColorStop(0, `rgba(255,255,255,0.62)`);
      grad.addColorStop(0.22, `rgba(${Math.min(255, r + 55)},${Math.min(255, g + 55)},${Math.min(255, b + 55)},0.52)`);
      grad.addColorStop(0.5, `rgba(${r},${g},${b},0.26)`);
      grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
      x.fillStyle = grad; x.fillRect(0, 0, s, s);
      this.spriteCache[color] = c;
      return c;
    }

    // ---------- layouts ----------
    _initPositions() {
      this.nodes.forEach((n) => { n.pos = { x: 0, y: 0, z: 0 }; n.tpos = { x: 0, y: 0, z: 0 }; n._set = false; });
    }
    _computeLayout(mode, instant) {
      const N = this.nodes, M = MEM;
      const LG = 2.55, TIME_H = 6.4, TR_G = 1.7;
      if (mode === "lattice") {
        N.forEach((n) => {
          const jx = (n.seed - 0.5) * 0.78 + (n.tenantIdx - M.TENANTS.length / 2) * 0.04;
          const jz = (n.sx) * 0.5;
          n.tpos = {
            x: (n.laneIdx - 2.5) * LG + jx,
            y: (n.tf - 0.5) * TIME_H,
            z: (n.trustIdx - 1.5) * TR_G + jz,
          };
        });
      } else if (mode === "galaxy") {
        N.forEach((n) => {
          const cc = M.clusterCenter[M.ccKey(n.lane, n.tenantIdx)];
          n.tpos = {
            x: cc.x * 6.4 + n.sx * 0.9,
            y: cc.y * 5.6 + n.sy * 0.9,
            z: cc.z * 6.0 + n.sz * 0.9,
          };
        });
      } else if (mode === "islands") {
        const TN = M.TENANTS.length;
        const centers = M.TENANTS.map((t, i) => {
          const ang = (i / TN) * TAU;
          const rad = 6.6;
          return { x: Math.cos(ang) * rad, y: (i % 2 ? 0.7 : -0.7) + Math.sin(ang * 2) * 0.5, z: Math.sin(ang) * rad };
        });
        N.forEach((n) => {
          const c = centers[n.tenantIdx];
          const a = n.seed * TAU, r = 0.5 + n.sx * 0.9 + n.sizeW * 0.3;
          n.tpos = { x: c.x + Math.cos(a) * r, y: c.y + (n.tf - 0.5) * 2.2 + n.sy * 0.5, z: c.z + Math.sin(a) * r };
        });
      } else if (mode === "river") {
        N.forEach((n) => {
          const t = n.tf;
          n.tpos = {
            x: Math.sin(t * Math.PI * 2.4) * 4.2 + (n.laneIdx - 2.5) * 0.5,
            y: (n.laneIdx - 2.5) * 0.95 + n.sy * 0.4,
            z: (t - 0.5) * 13 + n.sx * 0.6,
          };
        });
      }
      N.forEach((n) => {
        if (instant || !n._set) { n.pos = { ...n.tpos }; n._set = true; }
      });
    }

    setState(p) {
      const prevMode = this.state.mode;
      Object.assign(this.state, p);
      if (p.filters) this.state.filters = Object.assign({}, this.state.filters, p.filters);
      if (p.mode && p.mode !== prevMode) this._computeLayout(p.mode, false);
      this._dirtyFilter = true;
    }

    // ---------- filter / effective state ----------
    _passes(n) {
      const f = this.state.filters;
      if (f.tenants && !f.tenants.has(n.tenant)) return false;
      if (f.lanes && !f.lanes.has(n.lane)) return false;
      if (f.trusts && !f.trusts.has(n.trust)) return false;
      if (f.statuses && !f.statuses.has(n.status)) return false;
      if (!f.showQuarantined && n.trust === "InferredAdvisory") return false;
      return true;
    }
    _cutoffMs() { return MEM.T0 + this.state.timeT * MEM.span; }
    _effStatus(n, cutoff) {
      // node not yet born
      if (n.when > cutoff) return "future";
      if (n.status === "Superseded" && n.supersededBy) {
        const sup = MEM.byId[n.supersededBy];
        if (sup && sup.when > cutoff) return "Active"; // supersession hasn't happened yet
      }
      return n.status;
    }

    // ---------- color ----------
    _nodeColor(n) {
      const e = this.state.encoding;
      if (e === "trust") return MEM.TRUST[n.trustIdx].ring;
      if (e === "tenant") return TENANT_COLORS[n.tenant];
      return MEM.laneColor(n.lane);
    }

    // ---------- projection ----------
    _project(p) {
      const c = this.cam;
      const cy = Math.cos(c.yaw), sy = Math.sin(c.yaw);
      const cp = Math.cos(c.pitch), sp = Math.sin(c.pitch);
      const x1 = p.x * cy - p.z * sy;
      const z1 = p.x * sy + p.z * cy;
      const y1 = p.y * cp - z1 * sp;
      const z2 = p.y * sp + z1 * cp;
      const depth = z2 + c.dist;
      if (depth <= 0.2) return null;
      const f = (this.focal / depth) * c.zoom * this._scaleBase;
      return { sx: this.center.x + x1 * f, sy: this.center.y - y1 * f, depth, scale: f / 40 };
    }

    // ---------- main loop ----------
    _loop(ts) {
      this._lastFrame = performance.now();
      this._raf = requestAnimationFrame(this._loop);
      const dt = Math.min(0.05, (ts - (this._last || ts)) / 1000); this._last = ts;
      this._t += dt;
      this._update(dt);
      this._render();
    }

    _update(dt) {
      const c = this.cam, st = this.state;
      // fly-to
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
      // ease camera
      c.yaw = lerp(c.yaw, c.tyaw, 0.12);
      c.pitch = lerp(c.pitch, c.tpitch, 0.12);
      c.dist = lerp(c.dist, c.tdist, 0.1);
      c.zoom = lerp(c.zoom, c.tzoom, 0.14);
      // morph positions
      const N = this.nodes;
      for (let i = 0; i < N.length; i++) {
        const n = N[i], p = n.pos, t = n.tpos;
        p.x += (t.x - p.x) * 0.09; p.y += (t.y - p.y) * 0.09; p.z += (t.z - p.z) * 0.09;
      }
      // focus radius ease
      this._focusEase = lerp(this._focusEase || 0, st.selectedId ? 1 : 0, 0.1);
    }

    _render() {
      const ctx = this.ctx, st = this.state;
      const W = this.w, H = this.h;
      // background field + nebula + stars
      this._drawBackground(ctx, W, H);

      const cutoff = this._cutoffMs();
      const sel = st.selectedId ? MEM.byId[st.selectedId] : null;
      const hov = st.hoverId ? MEM.byId[st.hoverId] : null;

      // project all nodes
      const N = this.nodes, P = this._proj || (this._proj = new Array(N.length));
      const neigh = this._neighSet;
      for (let i = 0; i < N.length; i++) {
        const n = N[i];
        const pr = this._project(n.pos);
        P[i] = pr;
        if (!pr) continue;
        const es = this._effStatus(n, cutoff);
        n._eff = es;
        n._vis = this._passes(n);
      }

      // ---- edges (source-over, faint) ----
      ctx.globalCompositeOperation = "source-over";
      this._drawEdges(ctx, cutoff, sel, neigh);

      // ---- nodes ----
      const fb = hexRGB(st.field);
      const light = this._light = (0.299 * fb[0] + 0.587 * fb[1] + 0.114 * fb[2]) / 255 > 0.55;
      ctx.globalCompositeOperation = light ? "source-over" : "lighter";
      const order = this._order || (this._order = N.map((_, i) => i));
      order.sort((a, b) => (P[b] ? P[b].depth : 0) - (P[a] ? P[a].depth : 0));
      const bloomK = st.bloom;
      let bloomBudget = st.bloom < 0.4 ? 36 : 150;

      for (let k = 0; k < order.length; k++) {
        const i = order[k], n = N[i], pr = P[i];
        if (!pr) continue;
        if (n._eff === "future") continue;
        // alpha from depth fog
        let depthA = clamp((this.cam.dist + 7 - pr.depth) / 13, 0.12, 1);
        let baseA = depthA * 0.72;
        const color = this._nodeColor(n);
        // status dimming
        let sizeMul = 1, tint = color;
        if (n._eff === "Superseded") { baseA *= 0.4; tint = "#7d8a74"; }
        else if (n._eff === "Archived") { baseA *= 0.28; }
        // trust translucency / flicker
        if (n.trust === "InferredAdvisory") {
          baseA *= 0.5 + 0.32 * (0.5 + 0.5 * Math.sin(this._t * 5 + n.seed * 30));
        } else if (n.trust === "AgentAsserted") baseA *= 0.82;
        // filter dim
        if (!n._vis) baseA *= 0.07;
        // focus: dim non-neighbors
        if (sel && this._focusEase > 0.01) {
          const isFocus = n.id === sel.id || (neigh && neigh.has(n.id));
          if (!isFocus) baseA *= lerp(1, 0.12, this._focusEase);
        }
        const r = (2.5 + n.sizeW * 4.7) * pr.scale * (sel && n.id === sel.id ? 1.5 : 1);
        if (r < 0.25) continue;

        // ---- light field: solid dots, no additive ----
        if (light) {
          ctx.globalAlpha = clamp(baseA, 0, 1);
          ctx.fillStyle = tint;
          ctx.beginPath(); ctx.arc(pr.sx, pr.sy, Math.max(1, r * 0.62), 0, TAU); ctx.fill();
          if (n.belnap && n._eff === "Active") { ctx.fillStyle = "#d2381f"; ctx.globalAlpha = clamp(baseA * 0.7, 0, 1); ctx.beginPath(); ctx.arc(pr.sx, pr.sy, Math.max(1, r * 0.4), 0, TAU); ctx.fill(); }
          if (sel && n.id === sel.id) { ctx.globalAlpha = 1; ctx.strokeStyle = "#0e0f0c"; ctx.lineWidth = 1.5; ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r * 1.3 + 4, 0, TAU); ctx.stroke(); }
          n._sx = pr.sx; n._sy = pr.sy; n._r = r;
          continue;
        }

        // bloom halo for high-trust / selected / hovered
        const wantBloom = (n.trust === "DerivedDeterministic" || n.trust === "HumanConfirmed" || n.id === (sel && sel.id) || n.id === (hov && hov.id));
        if (wantBloom && bloomBudget > 0 && baseA > 0.2 && bloomK > 0.05) {
          bloomBudget--;
          const hr = r * (2.6 + bloomK * 1.6);
          ctx.globalAlpha = clamp(baseA * (0.06 + bloomK * 0.14) * (n.trust === "HumanConfirmed" ? 1.25 : 1), 0, 0.42);
          const sp = this._glow(color);
          ctx.drawImage(sp, pr.sx - hr, pr.sy - hr, hr * 2, hr * 2);
        }
        // core glow
        ctx.globalAlpha = clamp(baseA, 0, 1);
        const sprite = this._glow(tint);
        ctx.drawImage(sprite, pr.sx - r, pr.sy - r, r * 2, r * 2);

        // cited-by-an-answer pulse (yellow)
        if (this.cited && this.cited.has(n.id)) {
          ctx.globalCompositeOperation = "source-over";
          ctx.globalAlpha = 0.4 + 0.45 * (0.5 + 0.5 * Math.sin(this._t * 4 + n.seed * 5));
          ctx.strokeStyle = "#ffbd10"; ctx.lineWidth = 1.5;
          ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r * 1.5 + 5, 0, TAU); ctx.stroke();
          ctx.globalCompositeOperation = "lighter";
        }

        // contradiction glitch
        if (n.belnap && n._eff === "Active") {
          const gx = Math.sin(this._t * 22 + n.seed * 10) * r * 0.5;
          ctx.globalAlpha = clamp(baseA * 0.5, 0, 0.6);
          const rs = this._glow("#e23a28");
          ctx.drawImage(rs, pr.sx - r * 1.2 + gx, pr.sy - r * 1.2, r * 2.4, r * 2.4);
        }
        // selection ring (white pip)
        if (sel && n.id === sel.id) {
          ctx.globalCompositeOperation = "source-over";
          ctx.globalAlpha = 0.95;
          ctx.strokeStyle = "#ffffff"; ctx.lineWidth = 1.5;
          ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r * 1.7 + 4, 0, TAU); ctx.stroke();
          // confirmed gold ring
          ctx.globalCompositeOperation = "lighter";
        }
        n._sx = pr.sx; n._sy = pr.sy; n._r = r;
      }

      // particles along edges
      this._drawParticles(ctx, cutoff, sel, neigh);

      // analogy beams
      this._drawAnalogues(ctx);

      // labels (hover + selected + a few hubs)
      ctx.globalCompositeOperation = "source-over";
      this._drawLabels(ctx, sel, hov);

      // minimap (throttled)
      if (this.mini && (this._frameN = (this._frameN || 0) + 1) % 4 === 0) this._drawMinimap();

      // marquee rings + lasso rectangle
      this._drawMarquee(ctx);

      ctx.globalAlpha = 1;
    }

    _drawBackground(ctx, W, H) {
      const st = this.state;
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = st.field;
      ctx.fillRect(0, 0, W, H);
      // nebula blobs (baked offset by yaw for parallax)
      if (!this._neb || this._nebField !== st.field) this._bakeNebula();
      const px = (this.cam.yaw % TAU) * 40;
      ctx.globalAlpha = 1;
      ctx.drawImage(this._neb, -px * 0.2 - 60, -40, W + 200, H + 120);
    }
    _bakeNebula() {
      const st = this.state;
      const c = document.createElement("canvas"); c.width = this.w + 200; c.height = this.h + 120;
      const x = c.getContext("2d");
      const base = hexRGB(st.field);
      const dark = st.field === "#f4f1ea";
      const blobs = dark
        ? [[0.28, 0.3, "rgba(142,204,9,0.05)"], [0.72, 0.62, "rgba(255,189,16,0.05)"], [0.5, 0.5, "rgba(120,120,110,0.04)"]]
        : [[0.26, 0.32, "rgba(26,80,52,0.55)"], [0.74, 0.66, "rgba(20,60,72,0.4)"], [0.55, 0.2, "rgba(60,60,30,0.25)"]];
      blobs.forEach(([fx, fy, col]) => {
        const g = x.createRadialGradient(fx * c.width, fy * c.height, 0, fx * c.width, fy * c.height, c.width * 0.5);
        g.addColorStop(0, col); g.addColorStop(1, "rgba(0,0,0,0)");
        x.fillStyle = g; x.fillRect(0, 0, c.width, c.height);
      });
      // far starfield
      if (!dark) { } else {
        const r = (function () { let s = 99; return () => (s = (s * 16807) % 2147483647) / 2147483647; })();
        for (let i = 0; i < 220; i++) {
          const a = 0.04 + r() * 0.18, rr = r() * 1.3;
          x.fillStyle = `rgba(205,231,214,${a})`;
          x.beginPath(); x.arc(r() * c.width, r() * c.height, rr, 0, TAU); x.fill();
        }
      }
      // faint lattice dots
      x.fillStyle = dark ? "rgba(14,15,12,0.06)" : "rgba(205,231,214,0.05)";
      this._neb = c; this._nebField = st.field;
    }

    _drawEdges(ctx, cutoff, sel, neigh) {
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
        // focus emphasis
        let focusEdge = false;
        if (sel && this._focusEase > 0.05) {
          focusEdge = (e.a === sel.id || e.b === sel.id);
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

    _drawParticles(ctx, cutoff, sel, neigh) {
      const st = this.state;
      if (st.particles < 0.02) return;
      const P = this._proj;
      const budget = Math.round(lerp(60, 520, st.particles) * (st.bloom < 0.4 ? 0.4 : 1));
      ctx.globalCompositeOperation = "lighter";
      const step = Math.max(1, Math.round(this.edges.length / budget));
      const sprite = this.haloSprite;
      for (let i = 0; i < this.edges.length; i += step) {
        const e = this.edges[i];
        const a = this.nodes[e.ai], b = this.nodes[e.bi];
        if (a._eff === "future" || b._eff === "future") continue;
        const stl = EDGE_STYLE[e.kind]; if (!stl) continue;
        let emph = 1;
        if (sel && this._focusEase > 0.05) {
          if (e.a === sel.id || e.b === sel.id) emph = 2.2; else continue;
        } else {
          if (!a._vis || !b._vis) continue;
        }
        const pa = P[e.ai], pb = P[e.bi]; if (!pa || !pb) continue;
        const speed = stl.flow * (e.quarantined ? 0.5 : 1);
        const ph = (this._t * speed * 0.35 + e.ai * 0.013) % 1;
        const t = e.kind === "Supersedes" ? ph : (ph + Math.sin(e.ai) * 0.5) % 1;
        const sx = lerp(pa.sx, pb.sx, t), sy = lerp(pa.sy, pb.sy, t);
        const [r, g, bl] = stl.color.split(",").map(Number);
        const sz = (e.kind === "Supersedes" || e.kind === "Contradicts" ? 7 : 4.5) * emph;
        ctx.globalAlpha = clamp(0.5 * emph, 0, 0.85);
        // tint via temporary: draw white halo then we accept additive color from edges; cheap tint:
        ctx.drawImage(this._glow("#" + [r, g, bl].map((v) => v.toString(16).padStart(2, "0")).join("")), sx - sz, sy - sz, sz * 2, sz * 2);
      }
    }

    _drawLabels(ctx, sel, hov) {
      const list = [];
      if (sel && sel._sx != null && sel._eff !== "future") list.push([sel, true]);
      if (hov && hov !== sel && hov._sx != null && hov._eff !== "future") list.push([hov, false]);
      ctx.font = "500 12px Geist, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      const light = this._light;
      list.forEach(([n, strong]) => {
        const x = n._sx, y = n._sy, r = n._r || 4;
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
    _bindEvents() {
      const cv = this.cv;
      this._onDown = (ev) => {
        this.fly = null;
        const rect = cv.getBoundingClientRect();
        const mx = ev.clientX - rect.left, my = ev.clientY - rect.top;
        // lasso: shift-drag, or lassoMode armed
        if (ev.shiftKey || this.lassoMode) {
          this._lasso = { x0: mx, y0: my, x1: mx, y1: my, add: ev.metaKey || ev.ctrlKey };
          this._dragging = false;
          cv.setPointerCapture && cv.setPointerCapture(ev.pointerId);
          return;
        }
        this._dragging = true;
        this._lx = ev.clientX; this._ly = ev.clientY; this._moved = 0;
        cv.setPointerCapture && cv.setPointerCapture(ev.pointerId);
      };
      this._onMove = (ev) => {
        const rect = cv.getBoundingClientRect();
        const mx = ev.clientX - rect.left, my = ev.clientY - rect.top;
        if (this._lasso) {
          this._lasso.x1 = mx; this._lasso.y1 = my;
          this.cv.style.cursor = "crosshair";
          return;
        }
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
      this._onUp = (ev) => {
        if (this._lasso) {
          this._commitLasso();
          this._lasso = null;
          this.cv.style.cursor = this.lassoMode ? "crosshair" : "grab";
          return;
        }
        if (this._dragging && this._moved < 6) {
          const rect = cv.getBoundingClientRect();
          this._hitTest(ev.clientX - rect.left, ev.clientY - rect.top, ev.clientX, ev.clientY, true);
        }
        this._dragging = false;
      };
      this._onWheel = (ev) => {
        ev.preventDefault();
        this.cam.tdist = clamp(this.cam.tdist * (1 + Math.sign(ev.deltaY) * 0.08), 5.5, 34);
      };
      cv.addEventListener("pointerdown", this._onDown);
      window.addEventListener("pointermove", this._onMove);
      window.addEventListener("pointerup", this._onUp);
      cv.addEventListener("wheel", this._onWheel, { passive: false });
      cv.addEventListener("pointerleave", () => { if (!this._dragging) this._setHover(null); });
    }
    setLassoMode(on) { this.lassoMode = on; this.cv.style.cursor = on ? "crosshair" : "grab"; }
    _commitLasso() {
      const L = this._lasso, P = this._proj; if (!P) return;
      const xa = Math.min(L.x0, L.x1) * this.dpr, xb = Math.max(L.x0, L.x1) * this.dpr;
      const ya = Math.min(L.y0, L.y1) * this.dpr, yb = Math.max(L.y0, L.y1) * this.dpr;
      if (xb - xa < 4 && yb - ya < 4) { return; }
      const set = L.add && this.marquee ? new Set(this.marquee) : new Set();
      const cutoff = this._cutoffMs();
      for (let i = 0; i < this.nodes.length; i++) {
        const n = this.nodes[i], pr = P[i];
        if (!pr || n.when > cutoff || n._vis === false) continue;
        if (pr.sx >= xa && pr.sx <= xb && pr.sy >= ya && pr.sy <= yb) set.add(n.id);
      }
      this.marquee = set.size ? set : null;
      if (this.opts.onMarquee) this.opts.onMarquee(this.marquee ? [...this.marquee] : []);
    }
    clearMarquee() { this.marquee = null; if (this.opts.onMarquee) this.opts.onMarquee([]); }
    setMarquee(ids) { this.marquee = ids && ids.length ? new Set(ids) : null; if (this.opts.onMarquee) this.opts.onMarquee(ids || []); }
    _hitTest(mx, my, cx, cy, click) {
      const P = this._proj; if (!P) return;
      let best = null, bestD = 1e9;
      for (let i = 0; i < this.nodes.length; i++) {
        const n = this.nodes[i], pr = P[i];
        if (!pr || n._eff === "future") continue;
        if (n._vis === false) continue;
        const dx = pr.sx - mx, dy = pr.sy - my, d = dx * dx + dy * dy;
        const hitR = Math.max(7, (n._r || 4) * 1.6); // px
        if (d < hitR * hitR && d < bestD) { bestD = d; best = n; }
      }
      if (click) { this.select(best ? best.id : null); }
      else { this._setHover(best ? best.id : null, cx, cy); }
      this.cv.style.cursor = best ? "pointer" : (this._dragging ? "grabbing" : "grab");
    }
    _setHover(id, cx, cy) {
      if (this.state.hoverId !== id) { this.state.hoverId = id; if (this.opts.onHover) this.opts.onHover(id ? MEM.byId[id] : null, cx, cy); }
      else if (this.opts.onHover && id) this.opts.onHover(MEM.byId[id], cx, cy);
    }
    select(id) {
      this.state.selectedId = id;
      this.analogues = null;
      this._neighSet = id ? this._computeNeigh(id, this.focusHops || 1) : null;
      if (id) this.flyTo(id);
      if (this.opts.onSelect) this.opts.onSelect(id ? MEM.byId[id] : null);
    }
    _computeNeigh(id, hops) {
      const set = new Set([id]);
      let frontier = [id];
      for (let h = 0; h < hops; h++) {
        const next = [];
        frontier.forEach((nid) => (MEM.adj[nid] || []).forEach((a) => { if (!set.has(a.o)) { set.add(a.o); next.push(a.o); } }));
        frontier = next;
      }
      return set;
    }
    setFocusHops(n) {
      this.focusHops = n;
      if (this.state.selectedId && !this.analogues) this._neighSet = this._computeNeigh(this.state.selectedId, n);
    }
    showAnalogues(id) {
      const n = MEM.byId[id]; if (!n) return;
      let tos = (MEM.adj[id] || []).filter((a) => MEM.edges[a.e].kind === "AnalogousTo").map((a) => a.o);
      if (tos.length < 2) {
        tos = MEM.nodes.filter((m) => m.kind === n.kind && m.tenant !== n.tenant && m.id !== id)
          .sort((a, b) => b.deg - a.deg).slice(0, 4).map((m) => m.id);
      } else tos = tos.slice(0, 5);
      this.analogues = { from: id, to: tos, born: this._t };
      this.state.selectedId = id;
      this._neighSet = new Set([id, ...tos]);
      this.flyTo(id);
      if (this.opts.onSelect) this.opts.onSelect(n);
      return tos.map((t) => MEM.byId[t]);
    }
    flyTo(id) {
      const n = MEM.byId[id]; if (!n) return;
      // aim camera so node sits near center: solve yaw/pitch to face node direction
      const p = n.pos;
      const tyaw = Math.atan2(p.x, p.z) + 0; // rough
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
    setMinimap(canvas) { this.miniCv = canvas; this.mini = canvas ? canvas.getContext("2d") : null; }
    _drawMarquee(ctx) {
      ctx.globalCompositeOperation = "source-over";
      // rings on marquee'd nodes
      if (this.marquee && this.marquee.size) {
        const P = this._proj;
        ctx.strokeStyle = "rgba(255,189,16,0.9)"; ctx.lineWidth = 1.4;
        this.marquee.forEach((id) => {
          const n = MEM.byId[id], pr = P[n.idx];
          if (!pr || n._eff === "future") return;
          const r = (n._r || 4) * 1.5 + 4;
          ctx.beginPath(); ctx.arc(pr.sx, pr.sy, r, 0, TAU); ctx.stroke();
        });
      }
      // active lasso rectangle
      if (this._lasso) {
        const L = this._lasso, d = this.dpr;
        const x = Math.min(L.x0, L.x1) * d, y = Math.min(L.y0, L.y1) * d;
        const w = Math.abs(L.x1 - L.x0) * d, h = Math.abs(L.y1 - L.y0) * d;
        ctx.fillStyle = "rgba(255,189,16,0.08)"; ctx.fillRect(x, y, w, h);
        ctx.strokeStyle = "rgba(255,189,16,0.85)"; ctx.lineWidth = 1.2; ctx.setLineDash([5, 4]);
        ctx.strokeRect(x, y, w, h); ctx.setLineDash([]);
      }
    }
    _drawMinimap() {
      const ctx = this.mini; if (!ctx) return;
      const W = this.miniCv.width, H = this.miniCv.height, N = this.nodes;
      ctx.clearRect(0, 0, W, H);
      let minx = 1e9, maxx = -1e9, minz = 1e9, maxz = -1e9;
      for (const n of N) { const p = n.pos; if (p.x < minx) minx = p.x; if (p.x > maxx) maxx = p.x; if (p.z < minz) minz = p.z; if (p.z > maxz) maxz = p.z; }
      const pad = 9, s = Math.min((W - 2 * pad) / ((maxx - minx) || 1), (H - 2 * pad) / ((maxz - minz) || 1));
      const ox = pad + (W - 2 * pad - (maxx - minx) * s) / 2, oz = pad + (H - 2 * pad - (maxz - minz) * s) / 2;
      const map = (p) => ({ x: ox + (p.x - minx) * s, y: oz + (p.z - minz) * s });
      const cutoff = this._cutoffMs();
      for (let i = 0; i < N.length; i += 2) { const n = N[i]; if (n.when > cutoff) continue; const m = map(n.pos); ctx.fillStyle = this._nodeColor(n); ctx.globalAlpha = 0.55; ctx.fillRect(m.x, m.y, 1.5, 1.5); }
      ctx.globalAlpha = 1;
      const sel = this.state.selectedId && MEM.byId[this.state.selectedId];
      if (sel) { const m = map(sel.pos); ctx.strokeStyle = "#fff"; ctx.lineWidth = 1.2; ctx.beginPath(); ctx.arc(m.x, m.y, 4.5, 0, TAU); ctx.stroke(); }
      const cxp = W / 2, cyp = H / 2;
      ctx.strokeStyle = "rgba(142,204,9,0.8)"; ctx.lineWidth = 1.5;
      ctx.beginPath(); ctx.moveTo(cxp, cyp); ctx.lineTo(cxp + Math.sin(this.cam.yaw) * 15, cyp + Math.cos(this.cam.yaw) * 15); ctx.stroke();
      ctx.fillStyle = "rgba(142,204,9,0.8)"; ctx.beginPath(); ctx.arc(cxp, cyp, 2, 0, TAU); ctx.fill();
    }
    _drawAnalogues(ctx) {
      if (!this.analogues) return;
      const P = this._proj, from = MEM.byId[this.analogues.from], pa = P[from.idx];
      if (!pa) return;
      const age = this._t - this.analogues.born;
      ctx.globalCompositeOperation = "lighter";
      this.analogues.to.forEach((tid, k) => {
        const tn = MEM.byId[tid], pb = P[tn.idx]; if (!pb) return;
        const prog = clamp((age - k * 0.14) / 0.55, 0, 1); if (prog <= 0) return;
        const mx = (pa.sx + pb.sx) / 2, my = (pa.sy + pb.sy) / 2;
        const dx = pb.sx - pa.sx, dy = pb.sy - pa.sy, len = Math.hypot(dx, dy) || 1;
        const off = Math.min(150, len * 0.32);
        const cx = mx - dy / len * off, cy = my + dx / len * off;
        const bez = (t) => ({ x: (1 - t) * (1 - t) * pa.sx + 2 * (1 - t) * t * cx + t * t * pb.sx, y: (1 - t) * (1 - t) * pa.sy + 2 * (1 - t) * t * cy + t * t * pb.sy });
        ctx.strokeStyle = "rgba(181,140,255," + (0.55 * prog) + ")";
        ctx.lineWidth = 1.4; ctx.setLineDash([4, 7]); ctx.lineDashOffset = -this._t * 34;
        ctx.beginPath();
        const steps = 26, lim = Math.floor(steps * prog);
        for (let sct = 0; sct <= lim; sct++) { const b = bez(sct / steps); if (sct === 0) ctx.moveTo(b.x, b.y); else ctx.lineTo(b.x, b.y); }
        ctx.stroke(); ctx.setLineDash([]);
        const tt = ((this._t * 0.35 + k * 0.2) % 1) * prog, b = bez(tt), sp = this._glow("#b58cff");
        ctx.globalAlpha = 0.7 * prog; ctx.drawImage(sp, b.x - 6, b.y - 6, 12, 12);
        ctx.globalAlpha = 1;
      });
      ctx.globalCompositeOperation = "source-over";
    }
    setCited(ids) {
      this.cited = ids && ids.length ? new Set(ids) : null;
      if (ids && ids.length) this.flyTo(ids[0]);
    }
    resetView() {
      this.cam.tyaw = 0.5; this.cam.tpitch = -0.32; this.cam.tdist = 15.2; this.cam.tzoom = 1;
    }
    screenPosOf(id) {
      const i = MEM.byId[id] && MEM.byId[id].idx;
      const pr = this._proj && this._proj[i];
      if (!pr) return null;
      return { x: pr.sx / this.dpr, y: pr.sy / this.dpr };
    }

    _resize = () => {
      const r = this.cv.parentElement.getBoundingClientRect();
      this.w = Math.max(2, r.width * this.dpr); this.h = Math.max(2, r.height * this.dpr);
      this.cv.width = this.w; this.cv.height = this.h;
      this.cv.style.width = r.width + "px"; this.cv.style.height = r.height + "px";
      this.center = { x: this.w / 2, y: this.h / 2 };
      this._scaleBase = Math.min(this.w, this.h) / 13;
      this._neb = null;
    };
    dispose() {
      cancelAnimationFrame(this._raf);
      clearInterval(this._watch);
      window.removeEventListener("resize", this._resize);
      window.removeEventListener("pointermove", this._onMove);
      window.removeEventListener("pointerup", this._onUp);
    }
  }

  function roundRect(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y); ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r); ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r); ctx.closePath();
  }

  window.Constellation = Constellation;
  window.TENANT_COLORS = TENANT_COLORS;
})();
