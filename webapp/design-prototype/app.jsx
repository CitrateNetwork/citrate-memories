/* Memrizz — main app: routing, collapsibles, chat (context + tools + modes) */
const { useState, useEffect, useRef, useMemo } = React;

/* ---------- grounded Ask logic ---------- */
function pickTitle(sub, kind) { return MEM.nodes.find(n => (!kind || n.kind === kind) && n.title.toLowerCase().includes(sub)); }
function c(node, label) { return node ? { id: node.id, label } : label; }
function short(s) { return s.length > 24 ? s.slice(0, 22) + "…" : s; }
function searchNodes(q, n) {
  const words = q.toLowerCase().split(/[^a-z0-9]+/).filter(w => w.length > 3);
  const scored = MEM.nodes.map(nd => {
    let s = 0; const t = (nd.title + " " + nd.kind + " " + nd.tenant).toLowerCase();
    words.forEach(w => { if (t.includes(w)) s += 1; });
    s += nd.deg * 0.02; if (nd.status === "Active") s += 0.3;
    return { nd, s };
  });
  return scored.filter(x => x.s > 0.5).sort((a, b) => b.s - a.s).slice(0, n).map(x => x.nd);
}
function citeIdsOf(frags) { return frags.filter(f => typeof f === "object").map(f => f.id); }
function uniqByTitle(arr) { const seen = new Set(); return arr.filter(n => { if (seen.has(n.title)) return false; seen.add(n.title); return true; }); }

function answerFor(q) {
  const k = q.toLowerCase();
  if (/isolat|multi.?ten|org|tenant|saas/.test(k)) {
    const adr = pickTitle("isolated", "Adr") || pickTitle("multi-tenancy");
    const rat = MEM.nodes.find(n => n.kind === "Rationale" && /boundary|first/.test(n.title)) || MEM.nodes.find(n => n.kind === "Rationale");
    const sup = MEM.nodes.find(n => n.status === "Superseded" && n.lane === "specs");
    return {
      frags: ["We moved to <b>one isolated memory store per Org</b> — its own encrypted store, keyring and index — so cross-org leakage is physically impossible rather than a query-filter bug waiting to happen. The decision is on record in ", c(adr, "ADR · org-isolated"), ", and the gateway checks the Org boundary <i>before</i> any repo scope, for the reasons in ", c(rat, "Rationale"), "."],
      headsup: sup ? "This draws on a superseded spec that originally proposed a single multi-store process — it was replaced. Two advisory proposals near this topic are still unconfirmed." : "Two advisory proposals near this topic are still unconfirmed.",
      work: ['recall("multi-tenancy") → 6 memories', 'search("org isolation") · bge cosine → 9 ranked', "neighbors(ADR) → 4 edges", "critique() → 1 completeness gap"],
    };
  }
  if (/block|launch|gateway|ship|ready/.test(k)) {
    const bl = uniqByTitle(MEM.nodes.filter(n => n.kind === "Blocker")).slice(0, 3);
    const frags = ["Three things still gate the gateway. "];
    bl.forEach((b, i) => { frags.push(i === 0 ? "First, " : i === 1 ? " Next, " : " And "); frags.push(c(b, short(b.title))); frags.push("."); });
    frags.push(" The first two are backend prerequisites the RBAC needs.");
    return { frags, headsup: "One of these is marked advisory by an agent and hasn't been human-confirmed — treat its scope as provisional.", work: ['recall("gateway blockers") → 5', "neighbors(WP-6.1) → 7", 'search("prerequisite") → 4', "verify() on each"] };
  }
  if (/secur|finding|audit|risk|vuln/.test(k)) {
    const fs = uniqByTitle(MEM.nodes.filter(n => n.kind === "Finding")).slice(0, 3);
    const frags = ["The open findings cluster around the new network surface. "];
    fs.forEach((f, i) => { frags.push(i ? " " : ""); frags.push(c(f, short(f.title))); frags.push(i === fs.length - 1 ? "." : ";"); });
    frags.push(" All trace back to the fail-closed class the federation audit set.");
    return { frags, headsup: "Findings are Asserted-plane claims, not the deterministic record — confirm each against its source before acting.", work: ['search("security finding") · bge → 8', "recall(audit) → 6", "neighbors(Audit) → 5", "critique()"] };
  }
  if (/chang|recent|latest|new|mem-gateway|update/.test(k)) {
    const repo = "mem-gateway";
    const recent = MEM.perTenant[repo].slice().sort((a, b) => b.tf - a.tf).filter(n => /code|config|specs/.test(n.lane)).slice(0, 3);
    const frags = ["The most recent movement in <b>" + repo + "</b>: "];
    recent.forEach((r, i) => { frags.push(i ? ", " : ""); frags.push(c(r, r.kind + " · " + short(r.title))); });
    frags.push(". These are Derived — rebuilt straight from git, so trusted by construction.");
    return { frags, headsup: null, work: ['recall("mem-gateway") · newest-first → 7', "as_of(now)", "neighbors → 6"] };
  }
  const hits = searchNodes(q, 3);
  if (!hits.length) return { frags: ["I couldn't find memories matching that yet. Try a repo name, a decision, or a topic like “isolation”, “gateway blockers”, or “security findings”."], work: ['search("' + q.slice(0, 24) + '") → 0'] };
  const frags = ["Here's what the memory holds on that. "];
  hits.forEach((h, i) => { frags.push(i ? " Related: " : ""); frags.push(c(h, h.kind + " · " + short(h.title))); frags.push("."); });
  return { frags, headsup: hits.some(h => h.trust === "InferredAdvisory") ? "One source is an advisory proposal — quarantined until confirmed." : null, work: ['search("' + q.slice(0, 22) + '") · bge cosine → ' + (3 + hits.length), "neighbors → " + hits.reduce((s, h) => s + h.deg, 0), "critique()"] };
}

function contextAnswer(text, ctx) {
  const lead = ctx.length === 1 ? "Looking just at the memory you pulled in — " : `Looking across the ${ctx.length} memories you pulled in — `;
  const frags = [lead];
  ctx.forEach((n, i) => { frags.push(i ? (i === ctx.length - 1 ? " and " : ", ") : ""); frags.push(c(n, short(n.title))); });
  const tenants = [...new Set(ctx.map(n => n.tenant))];
  frags.push(` — they sit in ${tenants.length === 1 ? tenants[0] : tenants.length + " repos"} and ${ctx.some(n => n.plane === "Asserted") ? "mix the deterministic record with what people asserted" : "are all part of the deterministic record"}. `);
  const nb = MEM.adj[ctx[0].id] && MEM.adj[ctx[0].id][0];
  if (nb) { frags.push("The closest connected memory is "); frags.push(c(MEM.byId[nb.o], short(MEM.byId[nb.o].title))); frags.push("."); }
  return { frags, ctxNote: "grounded on " + ctx.length + " attached " + (ctx.length === 1 ? "memory" : "memories"), work: ["context[" + ctx.length + "]", "neighbors() → " + ctx.reduce((s, n) => s + n.deg, 0), "verify() on each"], headsup: ctx.some(n => n.trust === "InferredAdvisory") ? "One attached memory is an advisory proposal — quarantined until confirmed." : null };
}

function runTool(tool, target) {
  const t = target || MEM.nodes.slice().sort((a, b) => b.deg - a.deg)[0];
  const tech = (s) => s;
  if (tool.id === "recall") {
    const arr = MEM.perTenant[t.tenant].slice().sort((a, b) => b.tf - a.tf).slice(0, 3);
    const frags = ["Storyline for <b>" + t.tenant + "</b>, newest first: "];
    arr.forEach((n, i) => { frags.push(i ? ", " : ""); frags.push(c(n, n.kind + " · " + short(n.title))); }); frags.push(".");
    return { frags, work: ['recall("' + t.tenant + '") → ' + MEM.perTenant[t.tenant].length + " memories"] };
  }
  if (tool.id === "neighbors") {
    const nb = (MEM.adj[t.id] || []).slice(0, 3).map(a => MEM.byId[a.o]);
    const frags = ["What <b>" + short(t.title) + "</b> touches: "];
    nb.forEach((n, i) => { frags.push(i ? ", " : ""); frags.push(c(n, short(n.title))); }); frags.push(nb.length ? "." : "nothing recorded yet.");
    return { frags, work: ["neighbors(" + t.id + ") → " + (MEM.adj[t.id] || []).length + " edges"] };
  }
  if (tool.id === "verify") {
    const v = verifyOf(t);
    return { frags: ["Trust check on ", c(t, short(t.title)), ": <b>" + v.head + "</b>. " + v.reasons[0]], work: ["verify(" + t.id + ")"] };
  }
  if (tool.id === "analogy") {
    const an = MEM.nodes.find(n => n.kind === "AnalogyHypothesis") || t;
    return { frags: ["A latent cross-repo parallel: ", c(an, short(an.title)), " — ranked by combined embedding + structural score."], work: ["analogy(" + t.id + ") → 4 candidates"], headsup: "Analogues are advisory — confirm before treating as fact." };
  }
  if (tool.id === "critique") {
    const ct = MEM.review.critic[0];
    return { frags: ["Self-critic on the last answer: <b>" + ct.title.toLowerCase() + "</b>. ", c(ct.node, "see the source"), "."], work: ["critique() → 1 gap"], headsup: ct.detail };
  }
  // search default
  const hits = searchNodes(t.title, 3);
  const frags = ["Closest memories to <b>" + short(t.title) + "</b>: "];
  hits.forEach((h, i) => { frags.push(i ? ", " : ""); frags.push(c(h, short(h.title))); }); frags.push(".");
  return { frags, work: ['search() · bge cosine → ' + hits.length] };
}

const SUGGESTIONS = [
  { label: "Why per-Org isolation?", q: "Why did we move to per-Org isolation?" },
  { label: "What's blocking the gateway?", q: "What's still blocking the gateway launch?" },
  { label: "Open security findings", q: "Show me the open security findings." },
  { label: "Recent in mem-gateway", q: "What changed in mem-gateway lately?" },
];

const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
  "layout": "lattice", "encoding": "kind", "bloom": 0.55, "particles": 0.6,
  "field": "#0a1810", "motion": true, "quality": "full"
}/*EDITMODE-END*/;

function App() {
  const [t, setTweak] = useTweaks(TWEAK_DEFAULTS);
  const canvasRef = useRef(null);
  const instRef = useRef(null);
  const fromAsk = useRef(false);

  const prefersReduced = useMemo(() => window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches, []);
  const [guided, setGuided] = useState(true);
  const [view, setView] = useState("constellation");
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [dockOpen, setDockOpen] = useState(true);
  const [scrubOpen, setScrubOpen] = useState(true);
  const [filters, setFilters] = useState({ tenants: null, lanes: null, trusts: null, statuses: null, showQuarantined: true });
  const [timeT, setTimeT] = useState(1);
  const [playing, setPlaying] = useState(false);
  const [selected, setSelected] = useState(null);
  const [tab, setTab] = useState("ask");
  const [tip, setTip] = useState(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [context, setContext] = useState([]); // node objects
  const [flashId, setFlashId] = useState(null);
  const [hops, setHops] = useState(1);
  const [cmdOpen, setCmdOpen] = useState(false);
  const [marquee, setMarquee] = useState([]);
  const [toast, setToast] = useState(null);
  const [settings, setSettings] = useState({ landing: "constellation", mode: "plain", reduce: false, showQ: true, retention: "1y", model: "sonnet", byom: true, work: false, failClosed: true, tokenSafe: true, tokenTtl: "15m" });
  const reduced = settings.reduce || prefersReduced;
  const modelName = (MEM.MODELS.find(m => m.id === settings.model) || MEM.MODELS[0]).name;

  const [thread, setThread] = useState([{ role: "ai", frags: ["I'm grounded in your org's memory — <b>" + MEM.stats.nodes.toLocaleString() + "</b> records across " + MEM.stats.tenants + " repos. Ask me anything, pull specific memories in to ground on, or run a tool. I'll cite the exact memories — click any citation to fly to it."] }]);

  // ---- engine init ----
  useEffect(() => {
    const inst = new window.Constellation(canvasRef.current, {
      onSelect: (node) => { setSelected(node); if (node && !fromAsk.current) { setView("constellation"); setTab("inspect"); setDockOpen(true); } fromAsk.current = false; },
      onHover: (node, x, y) => setTip(node ? { node, x, y } : null),
      onMarquee: (ids) => setMarquee(ids),
    });
    instRef.current = inst; window.__mn = inst;
    return () => inst.dispose();
  }, []);

  // ---- ⌘K ----
  useEffect(() => {
    const onKey = (e) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) { e.preventDefault(); setCmdOpen((v) => !v); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const inst = instRef.current; if (!inst) return;
    inst.setState({ mode: t.layout, encoding: t.encoding, bloom: +t.bloom, particles: +t.particles, field: t.field, motion: !!t.motion, reduced });
  }, [t.layout, t.encoding, t.bloom, t.particles, t.field, t.motion, reduced]);
  useEffect(() => { const inst = instRef.current; if (inst) inst.setState({ filters }); }, [filters]);
  useEffect(() => { const inst = instRef.current; if (inst) inst.setState({ timeT }); }, [timeT]);

  useEffect(() => {
    if (!playing) return;
    let raf, last = performance.now();
    const tick = (now) => { const dt = (now - last) / 1000; last = now; setTimeT(p => { const nx = p + dt * 0.18; if (nx >= 1) { setPlaying(false); return 1; } return nx; }); raf = requestAnimationFrame(tick); };
    raf = requestAnimationFrame(tick); return () => cancelAnimationFrame(raf);
  }, [playing]);

  function setFilter(key, val) { setFilters(f => ({ ...f, [key]: val })); }
  function setSetting(k, v) { setSettings(s => ({ ...s, [k]: v })); }
  function fireToast(text, seq) { setToast({ text, seq }); setTimeout(() => setToast(null), 2800); }

  function selectNode(id, fromCite) { fromAsk.current = !!fromCite; instRef.current && instRef.current.select(id); }
  function gotoNode(id) { setView("constellation"); fromAsk.current = false; instRef.current && instRef.current.select(id); setTab("inspect"); setDockOpen(true); }
  function onCite(id) { setFlashId(id); setTimeout(() => setFlashId(null), 1000); if (instRef.current) instRef.current.cited = null; selectNode(id, true); }

  function onAsk(text) {
    const ctx = context.slice();
    setThread(th => [...th, { role: "user", text }, { role: "ai", pending: true }]);
    setTimeout(() => {
      const ans = ctx.length ? contextAnswer(text, ctx) : answerFor(text);
      const ids = citeIdsOf(ans.frags);
      setThread(th => { const cp = th.slice(); cp[cp.length - 1] = { role: "ai", ...ans }; return cp; });
      if (ids.length && instRef.current) { fromAsk.current = true; instRef.current.setCited(ids); setSelected(MEM.byId[ids[0]]); }
    }, 950);
  }

  function onTool(tool) {
    setView("constellation"); setTab("ask"); setDockOpen(true);
    if (tool.id === "as_of") { setScrubOpen(true); fireToast("Drag the timeline to look back in time", ""); return; }
    const target = context[0] || selected || null;
    setThread(th => [...th, { role: "ai", pending: true, tool: "running " + (settings.mode === "tech" ? tool.tech : tool.plain.toLowerCase()) + "…" }]);
    setTimeout(() => {
      const ans = runTool(tool, target);
      const ids = citeIdsOf(ans.frags);
      setThread(th => { const cp = th.slice(); cp[cp.length - 1] = { role: "ai", ...ans }; return cp; });
      if (ids.length && instRef.current) { fromAsk.current = true; instRef.current.setCited(ids); setSelected(MEM.byId[ids[0]]); }
    }, 820);
  }

  function addContext(ids) { setContext(cx => { const have = new Set(cx.map(n => n.id)); const add = ids.filter(id => !have.has(id)).map(id => MEM.byId[id]); return [...cx, ...add]; }); }
  function removeContext(id) { setContext(cx => cx.filter(n => n.id !== id)); }

  // ---- command palette routing ----
  function cmdNav(view) { if (view === "ask") { setView("constellation"); setTab("ask"); setDockOpen(true); } else setView(view); }
  function cmdAction(a) {
    setView("constellation");
    if (a === "lasso") { instRef.current && instRef.current.setLassoMode(true); fireToast("Lasso armed · drag a box, or Shift-drag anytime", ""); }
    else if (a === "replay") { setScrubOpen(true); setTimeT(0); setPlaying(true); }
    else if (a === "reset") { instRef.current && instRef.current.resetView(); instRef.current && instRef.current.select(null); setSelected(null); }
    else if (a.startsWith("layout:")) setTweak("layout", a.split(":")[1]);
  }
  function cmdFly(id) { setView("constellation"); fromAsk.current = false; instRef.current && instRef.current.select(id); setTab("inspect"); setDockOpen(true); }

  // ---- lasso bulk actions ----
  function clearMarquee() { instRef.current && instRef.current.clearMarquee(); setMarquee([]); }
  function onMarqueeAction(kind, ids) {
    if (kind === "ask") { addContext(ids.slice(0, 12)); setTab("ask"); setDockOpen(true); fireToast("Pulled " + Math.min(ids.length, 12) + " memories into the conversation", ""); }
    else if (kind === "focus") { fromAsk.current = false; instRef.current && instRef.current.select(ids[0]); }
    else if (kind === "confirm") { const a = ids.map(id => MEM.byId[id]).filter(n => n.trust === "InferredAdvisory").length; fireToast("Confirmed " + a + " advisory → load-bearing", "AUDIT #" + (50100 + (Math.random() * 99 | 0))); }
    else if (kind === "propose") { fireToast("Proposed a cluster edge over " + ids.length + " memories · quarantined", "AUDIT #" + (50200 + (Math.random() * 99 | 0))); }
    else if (kind === "export") { fireToast("Exported " + ids.length + " memories · lineage attached", ""); }
  }

  function onAction(kind, node) {
    if (kind === "discuss") { addContext([node.id]); setView("constellation"); setTab("ask"); setDockOpen(true); return; }
    if (kind === "neighbors") { fromAsk.current = false; instRef.current.select(node.id); }
    if (kind === "analogues") { fromAsk.current = false; const res = instRef.current.showAnalogues(node.id); fireToast("Found " + (res ? res.length : 0) + " cross-repo analogues", ""); }
    if (kind === "propose") fireToast("Edge proposed · quarantined for review", "AUDIT #" + (60100 + (Math.random() * 99 | 0)));
  }

  const visible = useMemo(() => {
    const cutoff = MEM.T0 + timeT * MEM.span;
    return MEM.nodes.filter(n => {
      if (n.when > cutoff) return false;
      if (filters.lanes && !filters.lanes.has(n.lane)) return false;
      if (filters.trusts && !filters.trusts.has(n.trust)) return false;
      if (filters.statuses && !filters.statuses.has(n.status)) return false;
      if (!filters.showQuarantined && n.trust === "InferredAdvisory") return false;
      return true;
    }).length;
  }, [filters, timeT]);

  const cycleModel = () => { const i = MEM.MODELS.findIndex(m => m.id === settings.model); setSetting("model", MEM.MODELS[(i + 1) % MEM.MODELS.length].id); };
  function onView(id) {
    if (id === "ask") { setView("constellation"); setTab("ask"); setDockOpen(true); }
    else setView(id);
  }
  const navView = view === "constellation" ? (dockOpen && tab === "ask" ? "ask" : "constellation") : view;
  const inConst = view === "constellation";
  const minimapRef = (node) => { if (instRef.current) instRef.current.setMinimap(node); };

  return (
    <div className="stage">
      <div className="canvas-wrap"><canvas ref={canvasRef} className="constellation"></canvas></div>

      <NavRail view={navView} onView={onView} reviewCount={MEM.reviewCount} />
      <TopBar onOmni={() => setCmdOpen(true)} model={modelName} onModel={cycleModel} onProfile={() => setView("profile")} onGoReview={() => setView("review")} />

      {inConst ? (
        <React.Fragment>
          <LeftRail collapsed={leftCollapsed} onCollapse={() => setLeftCollapsed(c => !c)} mode={t.layout} onMode={(m) => setTweak("layout", m)} filters={filters} setFilter={setFilter} encoding={t.encoding} />

          {dockOpen ? (
            <div className="rightdock panel">
              <div className="dock-tabs">
                <div className={"dock-tab" + (tab === "ask" ? " on" : "")} onClick={() => setTab("ask")}><MIcon name="spark" size={15}/>Ask</div>
                <div className={"dock-tab" + (tab === "inspect" ? " on" : "")} onClick={() => setTab("inspect")}><MIcon name="shield" size={15}/>Inspect{selected ? <span className="num">1</span> : null}</div>
                <span className="collapse-btn" style={{ alignSelf: "center", marginLeft: 4, marginBottom: 6 }} title="Collapse" onClick={() => setDockOpen(false)}><MIcon name="chevright" size={14}/></span>
              </div>
              {tab === "ask"
                ? <Ask thread={thread} onAsk={onAsk} onCite={onCite} flashId={flashId} model={modelName} onModel={cycleModel} suggestions={SUGGESTIONS} mode={settings.mode} onMode={(m) => setSetting("mode", m)} context={context} onAddCtx={() => setPickerOpen(true)} onRemoveCtx={removeContext} onTool={onTool} />
                : <Inspector node={selected} hops={hops} onHops={(h) => { setHops(h); instRef.current && instRef.current.setFocusHops(h); }} onNeighbor={(id) => selectNode(id, false)} onClose={() => { setSelected(null); instRef.current && instRef.current.select(null); }} onAction={onAction} />}
            </div>
          ) : <DockMini onOpen={(which) => { setTab(which); setDockOpen(true); }} reviewSel={!!selected} />}

          <div className="minimap-wrap panel">
            <div className="minimap-l">Overview</div>
            <canvas className="minimap" width={132} height={132} ref={minimapRef}></canvas>
          </div>

          {(scrubOpen && !marquee.length)
            ? <TimeScrubber t={timeT} onT={setTimeT} playing={playing} onPlay={setPlaying} onCollapse={() => setScrubOpen(false)} />
            : <ScrubMini t={timeT} onOpen={() => { setScrubOpen(true); clearMarquee(); }} />}
          <HUD visible={visible} mode={t.layout} />
          <Tooltip data={tip} />
          {marquee.length ? <SelectionHUD ids={marquee} onClear={clearMarquee} onAction={onMarqueeAction} /> : null}
        </React.Fragment>
      ) : null}

      {view === "review" ? <ReviewCenter onCite={gotoNode} onToast={fireToast} /> : null}
      {view === "connect" ? <ConnectPage onToast={fireToast} /> : null}
      {view === "audit" ? <AuditPage /> : null}
      {view === "settings" ? <SettingsPage onToast={fireToast} settings={settings} setSetting={setSetting} /> : null}
      {view === "profile" ? <ProfilePage onToast={fireToast} onCite={gotoNode} /> : null}

      {pickerOpen ? <MemoryPicker onClose={() => setPickerOpen(false)} onAdd={addContext} preselected={context.map(n => n.id)} /> : null}
      {cmdOpen ? <CommandPalette onClose={() => setCmdOpen(false)} onNav={cmdNav} onAction={cmdAction} onFly={cmdFly} /> : null}

      <TweaksPanel>
        <TweakSection label="Encoding" />
        <TweakRadio label="Color by" value={t.encoding} options={["kind", "trust", "tenant"]} onChange={(v) => setTweak("encoding", v)} />
        <TweakSection label="Layout" />
        <TweakRadio label="Mode" value={t.layout} options={["lattice", "galaxy", "islands", "river"]} onChange={(v) => setTweak("layout", v)} />
        <TweakSection label="Render" />
        <TweakRadio label="Quality" value={t.quality} options={["lite", "full"]} onChange={(v) => { setTweak("quality", v); if (v === "lite") { setTweak("bloom", 0.3); setTweak("particles", 0.25); } else { setTweak("bloom", 0.55); setTweak("particles", 0.6); } }} />
        <TweakSlider label="Bloom / glow" value={+t.bloom} min={0} max={1} step={0.05} onChange={(v) => setTweak("bloom", v)} />
        <TweakSlider label="Edge particles" value={+t.particles} min={0} max={1} step={0.05} onChange={(v) => setTweak("particles", v)} />
        <TweakToggle label="Alive motion" value={!!t.motion} onChange={(v) => setTweak("motion", v)} />
        <TweakSection label="Field" />
        <TweakColor label="Background" value={t.field} options={["#0a1810", "#0f2a1a", "#f4f1ea"]} onChange={(v) => setTweak("field", v)} />
      </TweaksPanel>

      {toast ? <div className="toast"><MIcon name="check" size={16} /><span className="tk">{toast.text}</span>{toast.seq ? <span className="seq">{toast.seq}</span> : null}</div> : null}
      {guided ? <GuidedOverlay onStart={() => setGuided(false)} /> : null}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(<App />);
