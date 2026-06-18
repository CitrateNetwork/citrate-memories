/* =============================================================
   Memrizz — synthetic federation memory graph
   ~900 content-addressed nodes across the Citrate federation.
   Deterministic (seeded) so the constellation is stable across loads.
   Attaches everything to window.MEM.
   ============================================================= */
(function () {
  // ---- seeded RNG (mulberry32) ----
  function rng(seed) {
    let a = seed >>> 0;
    return function () {
      a |= 0; a = (a + 0x6D2B79F5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }
  const rand = rng(40204);
  const pick = (arr) => arr[(rand() * arr.length) | 0];
  const gauss = () => (rand() + rand() + rand() + rand() - 2) / 2; // ~N(0,~0.4)

  // ---- six material lanes (the structured X axis) ----
  const LANES = [
    { id: "code",   label: "Code",        color: "#8ecc09", desc: "the built thing" },
    { id: "docs",   label: "Docs",        color: "#ffc83a", desc: "the written word" },
    { id: "specs",  label: "Specs",       color: "#5fa8e6", desc: "the plan" },
    { id: "config", label: "Configs",     color: "#34c7b0", desc: "the wiring" },
    { id: "audit",  label: "Tests/Audit", color: "#f0743a", desc: "the checks" },
    { id: "claims", label: "Claims",      color: "#b58cff", desc: "what someone said" },
  ];
  const LANE_IDX = Object.fromEntries(LANES.map((l, i) => [l.id, i]));

  // kind -> lane + plane
  const KINDS = {
    Commit:           { lane: "code",   plane: "Derived"  },
    Pr:               { lane: "code",   plane: "Derived"  },
    Doc:              { lane: "docs",   plane: "Derived"  },
    Narrative:        { lane: "docs",   plane: "Derived"  },
    Handoff:          { lane: "docs",   plane: "Asserted" },
    Sprint:           { lane: "specs",  plane: "Derived"  },
    Adr:              { lane: "specs",  plane: "Derived"  },
    WorkPackage:      { lane: "specs",  plane: "Derived"  },
    Rationale:        { lane: "specs",  plane: "Asserted" },
    ManifestChange:   { lane: "config", plane: "Derived"  },
    PinBump:          { lane: "config", plane: "Derived"  },
    DriftEvent:       { lane: "config", plane: "Derived"  },
    Audit:            { lane: "audit",  plane: "Derived"  },
    Finding:          { lane: "audit",  plane: "Asserted" },
    Benchmark:        { lane: "audit",  plane: "Derived"  },
    Blocker:          { lane: "audit",  plane: "Asserted" },
    TechDebt:         { lane: "audit",  plane: "Asserted" },
    Claim:            { lane: "claims", plane: "Asserted" },
    AgentAction:      { lane: "claims", plane: "Asserted" },
    AnalogyHypothesis:{ lane: "claims", plane: "Asserted" },
  };
  const KIND_LIST = Object.keys(KINDS);

  // ---- trust tiers ----
  const TRUST = [
    { id: "DerivedDeterministic", label: "Derived — deterministic", short: "Derived",   ring: "#cfe9b0",
      tip: "Rebuilt from git/markdown. Trusted by construction — no signature needed." },
    { id: "HumanConfirmed",       label: "Human-confirmed",          short: "Confirmed",  ring: "#ffd24a",
      tip: "A person reviewed and confirmed this. Load-bearing." },
    { id: "AgentAsserted",        label: "Agent-asserted",           short: "Asserted",   ring: "#9fc0e8",
      tip: "Signed by an agent or teammate. Authentic, but a claim — not the record." },
    { id: "InferredAdvisory",     label: "Inferred — advisory",      short: "Proposed",   ring: "#b58cff",
      tip: "An AI guess, quarantined. Advisory only until a human confirms it." },
  ];
  const TRUST_IDX = Object.fromEntries(TRUST.map((t, i) => [t.id, i]));

  const STATUS = {
    Active:     { label: "Active" },
    Superseded: { label: "Superseded" },
    Archived:   { label: "Archived" },
  };

  // ---- the federation: repo-tenants ----
  const TENANTS = [
    "citrate-memories", "mem-gateway", "citrate-identity", "citrate-explorer",
    "lattice-vm", "mcp-orchestrator", "ghostdag-consensus", "citrate-inference",
    "salt-tokenomics", "belnap-logic", "federated-learning", "x402-precompiles",
    "citrate-dashboard", "citrate-docs",
  ];
  // dependency map (a -> depends on b), drives DependsOn bridges + islands
  const DEPENDS = [
    ["mem-gateway", "citrate-memories"], ["mem-gateway", "citrate-identity"],
    ["citrate-explorer", "mem-gateway"], ["citrate-dashboard", "mem-gateway"],
    ["citrate-inference", "lattice-vm"], ["mcp-orchestrator", "citrate-inference"],
    ["lattice-vm", "ghostdag-consensus"], ["x402-precompiles", "lattice-vm"],
    ["salt-tokenomics", "ghostdag-consensus"], ["federated-learning", "ghostdag-consensus"],
    ["belnap-logic", "citrate-memories"], ["citrate-memories", "federated-learning"],
    ["citrate-docs", "citrate-memories"], ["mcp-orchestrator", "mem-gateway"],
  ];

  // ---- titles, flavored by kind ----
  const TITLE_BITS = {
    Commit: ["fix single-writer lock contention", "add HNSW recall path", "wire bge embeddings", "patch checkpoint rotation", "harden OIDC verify", "stream graph deltas over SSE", "guard supersede cycle", "cache UMAP layout per watermark"],
    Pr: ["per-Org engine routing", "crypto-shred keyring path", "delegation cascade revoke", "memory-diff merge preview", "attenuation-aware scope picker", "live audit tail"],
    Doc: ["Front-end spec", "Gateway contract", "Two-plane memory model", "Trust tiers explained", "Belnap FOUR-valued logic", "Quarantine & confirm loop", "Onboarding for non-technical teammates"],
    Narrative: ["how we decided on per-Org isolation", "the path to OSS release", "why the audit is the system", "story of the 34-repo federation"],
    Handoff: ["session handoff — gateway M0", "agent handoff — layout endpoint", "review handoff — RBAC seam"],
    Sprint: ["MEM-S6 packaging", "MEM-S5 durability", "MEM-S4 federation map", "MEM-S6 front-end track"],
    Adr: ["SaaS, org-isolated multi-tenancy", "new mem-gateway crate", "Streamable-HTTP for BYOM", "fail-closed OIDC config", "point, don't copy (Rule 9)"],
    WorkPackage: ["WP-6.1 Tier-1 audit", "WP-6.3 packaging", "WP-6.4 OSS release", "WP-6.5 rolling checkpoints"],
    Rationale: ["why Org boundary is checked first", "why confirmation is a human act", "why we never lie about certainty"],
    ManifestChange: ["bump mem-query → 0.9.2", "pin three.js layout sidecar", "add mem-gateway to workspace", "drift: identity SSE version skew"],
    PinBump: ["pin bge-small → 1.5", "pin rocksdb → 8.11", "pin axum → 0.7"],
    DriftEvent: ["manifest drift: explorer auth seam", "drift: dashboard chatbot fix", "drift: gateway vs daemon lock"],
    Audit: ["SECREM-02 fail-closed sweep", "Rule-8 review — gateway", "audit chain integrity check", "BYOM endpoint token-gating"],
    Finding: ["unauthenticated control surface risk", "tokens in localStorage (FUA-04)", "undefined passed to verify (1.4)", "over-grant possible in UI"],
    Benchmark: ["60fps @ 10k nodes (M-series)", "recall p95 latency", "UMAP layout cache hit rate", "SSE delta throughput"],
    Blocker: ["F-5 identity-bound principals", "F-7 delegation-revocation cascade", "signing path for assert/confirm"],
    TechDebt: ["second RBAC system risk", "per-Org daemon idle eviction", "tile streaming for large stores"],
    Claim: ["this recall omits 2 superseded sources", "the index watermark is 3h stale", "this tenant has no readable scope", "contradiction resolved in favor of ADR-12"],
    AgentAction: ["proposed edge: Adr supersedes Adr", "asserted: gateway owns DB lock", "recall over federation tenant", "verify run on root anchor"],
    AnalogyHypothesis: ["explorer auth seam ≈ gateway auth seam", "checkpoint rotation ≈ CRDT merge", "quarantine ≈ git stash", "delegation tree ≈ capability chain"],
  };

  function titleFor(kind, tenant) {
    const bits = TITLE_BITS[kind] || ["memory node"];
    return bits[(rand() * bits.length) | 0];
  }

  // ---- time window: 2025-01-01 .. 2026-06-13 ----
  const T0 = Date.UTC(2025, 0, 1);
  const T1 = Date.UTC(2026, 5, 13);
  const span = T1 - T0;

  // ---- build nodes ----
  const nodes = [];
  let idc = 0;
  const perTenant = {};
  TENANTS.forEach((tenant, ti) => {
    const count = 42 + ((rand() * 34) | 0);
    perTenant[tenant] = [];
    for (let i = 0; i < count; i++) {
      const kind = pick(KIND_LIST);
      const meta = KINDS[kind];
      const lane = meta.lane;
      // time: weighted toward recent
      const tf = Math.min(0.999, Math.pow(rand(), 0.65));
      const when = T0 + tf * span;
      // trust assignment
      let trust;
      if (meta.plane === "Derived") {
        trust = rand() < 0.86 ? "DerivedDeterministic" : "HumanConfirmed";
      } else {
        const r = rand();
        if (kind === "AnalogyHypothesis" || kind === "AgentAction") trust = r < 0.7 ? "InferredAdvisory" : "AgentAsserted";
        else trust = r < 0.45 ? "HumanConfirmed" : r < 0.8 ? "AgentAsserted" : "InferredAdvisory";
      }
      // status
      let status = "Active", supersededBy = null;
      const sr = rand();
      if (sr < 0.11 && tf < 0.85) status = "Superseded";
      else if (sr < 0.155 && tf < 0.5) status = "Archived";

      const node = {
        id: "n" + (idc++),
        idx: idc - 1,
        kind, lane, laneIdx: LANE_IDX[lane],
        plane: meta.plane,
        trust, trustIdx: TRUST_IDX[trust],
        status, supersededBy,
        tenant, tenantIdx: ti,
        title: titleFor(kind, tenant),
        tf, when,
        belnap: false,
        deg: 0,
        seed: rand(),
        sx: gauss(), sy: gauss(), sz: gauss(), // semantic jitter (galaxy)
        hash: "ctr:" + (0x10000000 + ((rand() * 0xefffffff) | 0)).toString(16),
      };
      nodes.push(node);
      perTenant[tenant].push(node);
    }
  });

  // semantic cluster centers per (lane,tenant) for galaxy mode
  const clusterCenter = {};
  function ccKey(lane, ti) { return lane + ":" + ti; }
  LANES.forEach((l) => TENANTS.forEach((t, ti) => {
    const ang = rand() * Math.PI * 2;
    const rad = 0.45 + rand() * 0.55;
    clusterCenter[ccKey(l.id, ti)] = {
      x: Math.cos(ang) * rad + (LANE_IDX[l.id] - 2.5) * 0.18,
      y: Math.sin(ang) * rad,
      z: gauss() * 0.7 + (ti - TENANTS.length / 2) * 0.06,
    };
  }));

  // ---- edges ----
  const edges = [];
  const addEdge = (a, b, kind, quarantined) => {
    if (!a || !b || a === b) return;
    edges.push({ a: a.id, b: b.id, ai: a.idx, bi: b.idx, kind, quarantined: !!quarantined });
    a.deg++; b.deg++;
  };
  const byId = Object.fromEntries(nodes.map((n) => [n.id, n]));

  // TemporalNext spine within each tenant (sorted by time)
  TENANTS.forEach((t) => {
    const arr = perTenant[t].slice().sort((a, b) => a.tf - b.tf);
    for (let i = 1; i < arr.length; i++) {
      if (rand() < 0.82) addEdge(arr[i - 1], arr[i], "TemporalNext", false);
    }
    // a few extra structural edges within tenant
    for (let i = 0; i < arr.length; i++) {
      if (rand() < 0.25) addEdge(arr[i], arr[(rand() * arr.length) | 0], rand() < 0.5 ? "DependsOn" : "References", false);
    }
  });

  // Supersedes: link superseded nodes to a later node in same tenant+lane
  nodes.forEach((n) => {
    if (n.status === "Superseded") {
      const cands = perTenant[n.tenant].filter((m) => m.tf > n.tf && m.lane === n.lane && m.status === "Active");
      const sup = cands.length ? cands[(rand() * cands.length) | 0] : null;
      if (sup) { n.supersededBy = sup.id; addEdge(sup, n, "Supersedes", false); }
    }
  });

  // DependsOn bridges across tenants (hub nodes)
  const hub = {};
  TENANTS.forEach((t) => {
    const arr = perTenant[t].slice().sort((a, b) => b.deg - a.deg);
    hub[t] = arr[0];
  });
  DEPENDS.forEach(([a, b]) => {
    if (hub[a] && hub[b]) addEdge(hub[a], hub[b], "DependsOn", false);
    // a couple more node-level dependency beams
    for (let k = 0; k < 2; k++) {
      addEdge(pick(perTenant[a]), pick(perTenant[b]), "DependsOn", false);
    }
  });

  // AnalogousTo: cross-tenant dashed arcs between same-kind nodes
  for (let i = 0; i < 26; i++) {
    const a = pick(nodes);
    const cands = nodes.filter((m) => m.kind === a.kind && m.tenant !== a.tenant);
    if (cands.length) addEdge(a, pick(cands), "AnalogousTo", false);
  }

  // Quarantined proposed edges (advisory, pending)
  const proposals = [];
  for (let i = 0; i < 18; i++) {
    const a = pick(nodes), b = pick(nodes);
    if (a !== b && a.tenant === b.tenant) {
      addEdge(a, b, "Supersedes", true);
      proposals.push(edges[edges.length - 1]);
    }
  }

  // ---- contradictions (Belnap Both): a handful ----
  const contradictions = [];
  for (let i = 0; i < 7; i++) {
    const n = pick(nodes.filter((m) => m.plane === "Asserted" && m.status === "Active"));
    if (n && !n.belnap) {
      n.belnap = true;
      // a conflicting claim
      const other = pick(perTenant[n.tenant].filter((m) => m !== n));
      if (other) { addEdge(n, other, "Contradicts", false); contradictions.push({ a: n.id, b: other.id }); }
    }
  }

  // normalize degree -> size weight
  let maxDeg = 1;
  nodes.forEach((n) => { if (n.deg > maxDeg) maxDeg = n.deg; });
  nodes.forEach((n) => { n.sizeW = 0.45 + Math.pow(n.deg / maxDeg, 0.55) * 1.0; });

  // adjacency for neighbor lookups
  const adj = {};
  nodes.forEach((n) => (adj[n.id] = []));
  edges.forEach((e, i) => { adj[e.a].push({ e: i, o: e.b, dir: "out" }); adj[e.b].push({ e: i, o: e.a, dir: "in" }); });

  // helpers
  function fmtDate(ms) {
    const d = new Date(ms);
    return d.toISOString().slice(0, 10);
  }
  function fmtAgo(ms) {
    const days = Math.max(0, Math.round((T1 - ms) / 86400000));
    if (days === 0) return "today";
    if (days < 7) return days + "d ago";
    if (days < 60) return Math.round(days / 7) + "w ago";
    return Math.round(days / 30) + "mo ago";
  }

  window.MEM = {
    LANES, LANE_IDX, KINDS, KIND_LIST, TRUST, TRUST_IDX, STATUS,
    TENANTS, DEPENDS, nodes, edges, byId, adj, perTenant, hub,
    proposals, contradictions, clusterCenter, ccKey,
    T0, T1, span, fmtDate, fmtAgo,
    laneColor: (id) => (LANES[LANE_IDX[id]] || {}).color || "#cccccc",
    stats: {
      nodes: nodes.length, edges: edges.length,
      superseded: nodes.filter((n) => n.status === "Superseded").length,
      proposed: proposals.length,
      contradictions: contradictions.length,
      tenants: TENANTS.length,
    },
  };
})();
