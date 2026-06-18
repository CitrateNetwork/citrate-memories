/* =============================================================
   Memrizz — extras: people/RBAC, review queues, tools, models,
   notifications, current-user profile. Augments window.MEM.
   ============================================================= */
(function () {
  const MEM = window.MEM;
  const rand = (() => { let s = 7; return () => (s = (s * 16807) % 2147483647) / 2147483647; })();
  const pick = (a) => a[(rand() * a.length) | 0];

  // ---- agents that propose / assert ----
  const AGENTS = ["Claude Fable 5", "Analogy Engine", "Drift Sentinel", "Mentor–Mentee", "GPT-5.1 session"];

  // ---- people (RBAC: Org Owner → Org Admin → Member; agents inherit) ----
  const PEOPLE = [
    { id: "u_aleia", name: "Aleia Mercer", initials: "A", role: "Org Owner", policy: "Maintainer", scopes: ["*"], parent: null, color: "#ffbd10", title: "Head of Operations", email: "aleia@citrate.ai", me: true },
    { id: "u_dana", name: "Dana Okafor", initials: "D", role: "Org Admin", policy: "Operator", scopes: ["mem-gateway", "citrate-identity", "citrate-explorer"], parent: "u_aleia", color: "#8ecc09", title: "Platform Lead", email: "dana@citrate.ai" },
    { id: "u_marco", name: "Marco Reyes", initials: "M", role: "Org Admin", policy: "Operator", scopes: ["lattice-vm", "ghostdag-consensus", "citrate-inference"], parent: "u_aleia", color: "#5fa8e6", title: "Consensus Lead", email: "marco@citrate.ai" },
    { id: "u_noor", name: "Noor Haddad", initials: "N", role: "Org Admin", policy: "Operator", scopes: ["salt-tokenomics", "x402-precompiles"], parent: "u_aleia", color: "#34c7b0", title: "Tokenomics Lead", email: "noor@citrate.ai" },
    { id: "u_priya", name: "Priya Anand", initials: "P", role: "Member", policy: "Guided", scopes: ["citrate-docs", "citrate-memories"], parent: "u_dana", color: "#b58cff", title: "Product Manager", email: "priya@citrate.ai" },
    { id: "u_sven", name: "Sven Halvorsen", initials: "S", role: "Member", policy: "ReadOnly", scopes: ["citrate-dashboard"], parent: "u_marco", color: "#f0743a", title: "Analyst", email: "sven@citrate.ai" },
    { id: "u_lin", name: "Lin Wei", initials: "L", role: "Member", policy: "Guided", scopes: ["federated-learning"], parent: "u_marco", color: "#cbb78a", title: "ML Engineer", email: "lin@citrate.ai" },
    { id: "u_fable", name: "Fable-5 · agent", initials: "F", role: "Agent / BYOM", policy: "inherits Priya", scopes: ["citrate-memories"], parent: "u_priya", color: "#9fc0e8", title: "BYOM session", email: "mcp://fable-5", agent: true },
  ];
  const ME = PEOPLE.find((p) => p.me);

  // ---- review queues, derived from the graph ----
  const RATIONALES = [
    "embeddings cosine 0.91 + shared ADR lineage suggest a supersession",
    "manifest drift map links these two across the seam",
    "both reference the same blake3 source artifact",
    "temporal adjacency + author overlap; advisory only",
    "analogy engine matched structure across tenants",
  ];
  const proposals = MEM.proposals.map((e, i) => {
    const a = MEM.byId[e.a], b = MEM.byId[e.b];
    return {
      id: "pr_" + i, edge: e, a, b,
      relation: e.kind, proposer: AGENTS[i % AGENTS.length],
      when: MEM.T1 - (rand() * 26 + 1) * 3600000,
      rationale: RATIONALES[i % RATIONALES.length],
      confidence: Math.round((0.62 + rand() * 0.33) * 100),
      status: "pending",
    };
  });
  const contradictions = MEM.contradictions.map((c, i) => {
    const a = MEM.byId[c.a], b = MEM.byId[c.b];
    return {
      id: "cx_" + i, a, b,
      claimA: a.title, claimB: b.title,
      assertedByA: pick(MEM.people ? MEM.people : PEOPLE).name, assertedByB: pick(AGENTS),
      when: MEM.T1 - (rand() * 60 + 2) * 3600000, status: "pending",
    };
  });
  const supersessions = MEM.nodes.filter((n) => n.status === "Superseded" && n.supersededBy).slice(0, 9).map((n, i) => ({
    id: "ss_" + i, old: n, neu: MEM.byId[n.supersededBy],
    when: MEM.byId[n.supersededBy].when, status: "pending",
  }));
  const critic = [
    { id: "ct_0", title: "A recall omitted 2 superseded sources", detail: "Last week’s “gateway packaging” recall didn’t surface 2 superseded ADRs that still shape the decision. Re-run with superseded included?", node: MEM.nodes.find((n) => n.kind === "Adr"), severity: "medium" },
    { id: "ct_1", title: "Answer drew on a stale index", detail: "The bge index watermark was 3h behind when this answer was produced. 14 newer memories were not considered.", node: MEM.nodes.find((n) => n.kind === "Commit"), severity: "low" },
    { id: "ct_2", title: "Adjacent unconfirmed proposal", detail: "An answer about delegation leaned on a region with 3 quarantined proposals — none confirmed. Coverage may be optimistic.", node: MEM.nodes.find((n) => n.kind === "Blocker"), severity: "high" },
    { id: "ct_3", title: "Contradiction left unresolved", detail: "Two assertions about the DB-lock owner conflict (Belnap “Both”) and were both cited without flagging the conflict.", node: MEM.nodes.find((n) => n.belnap), severity: "high" },
  ].filter((x) => x.node);

  // ---- models ----
  const MODELS = [
    { id: "sonnet", name: "Claude Sonnet 4.6", provider: "Anthropic", kind: "in-app", cost: "$$", latency: "fast", note: "Best reasoning over the graph" },
    { id: "gpt", name: "GPT-5.1", provider: "OpenAI", kind: "in-app", cost: "$$$", latency: "medium", note: "Strong general recall" },
    { id: "citrate", name: "Citrate-LM", provider: "Citrate", kind: "on-prem", cost: "$", latency: "fast", note: "Runs inside your tenancy" },
    { id: "local", name: "Local · Llama 4", provider: "Self-hosted", kind: "BYOM", cost: "free", latency: "varies", note: "Connected over MCP" },
  ];

  // ---- the memory tools the model can call ----
  const TOOLS = [
    { id: "recall", plain: "Pull a storyline", tech: "recall()", icon: "history", desc: "Walk a repo’s memory newest-first — the story of what happened." },
    { id: "search", plain: "Find memories", tech: "search() · bge cosine", icon: "search", desc: "Semantic search across everything you can read." },
    { id: "neighbors", plain: "See what’s connected", tech: "neighbors()", icon: "target", desc: "The blast-radius around a memory — what it touches." },
    { id: "verify", plain: "Check trust", tech: "verify()", icon: "shield", desc: "Is a memory trustworthy? Signature, supersession, contradiction." },
    { id: "as_of", plain: "Look back in time", tech: "as_of(T)", icon: "clock", desc: "What the memory looked like at a chosen moment." },
    { id: "analogy", plain: "Find analogues", tech: "analogy()", icon: "spark", desc: "Latent cross-repo parallels ranked by structure + meaning." },
    { id: "critique", plain: "What’s missing?", tech: "critique()", icon: "info", desc: "Self-critic: gaps, stale sources, unconfirmed proposals." },
  ];

  // ---- audit chain: synthetic blake3 hash-chained event stream ----
  const AUDIT_KINDS = [
    { id: "Read", w: 44, color: "#9fc0e8" },
    { id: "Recall", w: 11, color: "#34c7b0" },
    { id: "Write", w: 12, color: "#8ecc09" },
    { id: "Assert", w: 9, color: "#b58cff" },
    { id: "Confirm", w: 7, color: "#8ecc09" },
    { id: "Propose", w: 6, color: "#ffbd10" },
    { id: "Denied", w: 7, color: "#f0743a" },
    { id: "Shred", w: 1, color: "#a72414" },
  ];
  const AUDIT_CUM = (() => { let c = 0; return AUDIT_KINDS.map((k) => (c += k.w)); })();
  const AUDIT_TOTAL = AUDIT_CUM[AUDIT_CUM.length - 1];
  function pickKind() { const r = rand() * AUDIT_TOTAL; for (let i = 0; i < AUDIT_KINDS.length; i++) if (r < AUDIT_CUM[i]) return AUDIT_KINDS[i]; return AUDIT_KINDS[0]; }
  function shortHash() { return (0x10000000 + ((rand() * 0xefffffff) | 0)).toString(16).slice(0, 10); }
  function auditDetail(kind, repo, actor) {
    switch (kind) {
      case "Read": return "read " + (rand() < 0.5 ? "node " + shortHash() : "repo:" + repo + "/memory");
      case "Recall": return 'recall("' + repo + '") · ' + ((rand() * 40 + 4) | 0) + " memories";
      case "Write": return "merge_diff → +" + ((rand() * 5 + 1) | 0) + " nodes, +" + ((rand() * 4) | 0) + " edges";
      case "Assert": return "signed AgentAsserted node " + shortHash();
      case "Confirm": return "confirmed proposal → promoted to load-bearing";
      case "Propose": return "propose_edge → quarantined " + shortHash();
      case "Denied": return "scope check failed · repo:" + repo + " (no read grant)";
      case "Shred": return "crypto-shred · key destroyed for repo:" + repo;
      default: return "";
    }
  }
  const auditActors = PEOPLE.concat(AGENTS.map((a, i) => ({ id: "ag_" + i, name: a, initials: a[0], color: "#9fc0e8", agent: true })));
  const auditEvents = [];
  let aseq = 2481902;
  let at = MEM.T1;
  for (let i = 0; i < 260; i++) {
    const kind = pickKind();
    const actor = kind === "Shred" || kind === "Confirm" ? ME : auditActors[(rand() * auditActors.length) | 0];
    const repo = (actor.scopes && actor.scopes[0] && actor.scopes[0] !== "*") ? actor.scopes[0] : MEM.TENANTS[(rand() * MEM.TENANTS.length) | 0];
    at -= (rand() * 90 + 6) * 1000; // seconds apart
    auditEvents.push({
      seq: aseq--, kind: kind.id, color: kind.color,
      actor: actor.name, initials: actor.initials, actorColor: actor.color, agent: !!actor.agent,
      repo, detail: auditDetail(kind.id, repo, actor), when: at, hash: shortHash(),
    });
  }

  const NOTIFS = [
    { id: "nt0", icon: "branch", text: "3 proposals are awaiting your witness", sub: "Review Center · proposals", when: "12m ago", kind: "proposal" },
    { id: "nt1", icon: "shield", text: "Contradiction detected in citrate-memories", sub: "Belnap “Both” on a root anchor", when: "1h ago", kind: "contradiction" },
    { id: "nt2", icon: "check", text: "Ingestion finished for lattice-vm", sub: "+212 nodes · +148 edges", when: "3h ago", kind: "ingest" },
    { id: "nt3", icon: "spark", text: "Priya asserted a memory in citrate-docs", sub: "“Onboarding for non-technical teammates”", when: "5h ago", kind: "assert" },
  ];

  Object.assign(MEM, {
    AGENTS, PEOPLE, ME, MODELS, TOOLS, NOTIFS, AUDIT_KINDS, auditEvents,
    auditActors,
    review: { proposals, contradictions, supersessions, critic },
    reviewCount: proposals.length + contradictions.length + supersessions.length + critic.length,
    personById: (id) => PEOPLE.find((p) => p.id === id),
  });
})();
