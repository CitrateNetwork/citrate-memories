/* Memrizz — ⌘K command palette (search-to-fly) + lasso selection HUD */

const CMD_NAV = [
  { id: "nav:constellation", icon: "layers", title: "Go to Constellation", group: "Navigate", view: "constellation" },
  { id: "nav:ask", icon: "spark", title: "Ask the memory", group: "Navigate", view: "ask" },
  { id: "nav:review", icon: "shield", title: "Review center", group: "Navigate", view: "review" },
  { id: "nav:connect", icon: "link", title: "Connect a model", group: "Navigate", view: "connect" },
  { id: "nav:audit", icon: "ledger", title: "Audit log", group: "Navigate", view: "audit" },
  { id: "nav:settings", icon: "sliders", title: "Settings", group: "Navigate", view: "settings" },
  { id: "nav:profile", icon: "target", title: "Your profile", group: "Navigate", view: "profile" },
];
const CMD_ACTIONS = [
  { id: "act:lasso", icon: "grid", title: "Lasso-select memories", sub: "drag a box over the constellation", group: "Action", action: "lasso" },
  { id: "act:replay", icon: "history", title: "Replay history", sub: "animate the memory through time", group: "Action", action: "replay" },
  { id: "act:reset", icon: "reset", title: "Reset the view", group: "Action", action: "reset" },
  { id: "act:layout-galaxy", icon: "spark", title: "Layout · Galaxy", group: "Action", action: "layout:galaxy" },
  { id: "act:layout-lattice", icon: "grid", title: "Layout · Lattice", group: "Action", action: "layout:lattice" },
  { id: "act:layout-islands", icon: "cube", title: "Layout · Islands", group: "Action", action: "layout:islands" },
  { id: "act:layout-river", icon: "waves", title: "Layout · River", group: "Action", action: "layout:river" },
];

function CommandPalette({ onClose, onNav, onAction, onFly }) {
  const [q, setQ] = React.useState("");
  const [active, setActive] = React.useState(0);
  const inputRef = React.useRef(null);
  React.useEffect(() => { if (inputRef.current) inputRef.current.focus(); }, []);

  const groups = React.useMemo(() => {
    const term = q.trim().toLowerCase();
    const words = term.split(/[^a-z0-9]+/).filter(Boolean);
    // memories
    let mems = [];
    if (term.length) {
      mems = MEM.nodes.map((n) => {
        const t = (n.title + " " + n.kind + " " + n.tenant).toLowerCase();
        let s = 0; words.forEach((w) => { if (t.includes(w)) s += 1; });
        if (t.startsWith(term)) s += 0.5;
        return { n, s: s + n.deg * 0.008 };
      }).filter((x) => x.s > 0).sort((a, b) => b.s - a.s).slice(0, 7)
        .map((x) => ({ id: "mem:" + x.n.id, icon: "spark", title: x.n.title, sub: x.n.kind + " · " + x.n.tenant, group: "Memories", node: x.n, color: MEM.laneColor(x.n.lane) }));
    }
    // repos
    const repos = MEM.TENANTS.filter((t) => !term || t.toLowerCase().includes(term)).slice(0, term ? 4 : 0)
      .map((t) => ({ id: "repo:" + t, icon: "cube", title: t, sub: MEM.perTenant[t].length + " memories", group: "Repos", repo: t }));
    const match = (it) => !term || (it.title + " " + (it.sub || "")).toLowerCase().includes(term);
    const nav = CMD_NAV.filter(match);
    const acts = CMD_ACTIONS.filter(match);
    const out = [];
    if (mems.length) out.push(["Memories", mems]);
    if (repos.length) out.push(["Repos", repos]);
    if (acts.length) out.push(["Action", acts]);
    if (nav.length) out.push(["Navigate", nav]);
    return out;
  }, [q]);

  const flat = React.useMemo(() => groups.reduce((a, [, items]) => a.concat(items), []), [groups]);
  React.useEffect(() => { setActive(0); }, [q]);

  function run(item) {
    if (!item) return;
    if (item.node) onFly(item.node.id);
    else if (item.repo) onFly(MEM.hub[item.repo].id);
    else if (item.view) onNav(item.view);
    else if (item.action) onAction(item.action);
    onClose();
  }
  function onKey(e) {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((a) => Math.min(flat.length - 1, a + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive((a) => Math.max(0, a - 1)); }
    else if (e.key === "Enter") { e.preventDefault(); run(flat[active]); }
    else if (e.key === "Escape") { onClose(); }
  }

  let idx = -1;
  return (
    <div className="modal-back cmd-back" onClick={onClose}>
      <div className="cmdk panel" onClick={(e) => e.stopPropagation()}>
        <div className="cmdk-search">
          <MIcon name="search" size={18} />
          <input ref={inputRef} value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={onKey} placeholder="Search memories, repos, actions — or jump anywhere…" />
          <span className="kbd">esc</span>
        </div>
        <div className="cmdk-list">
          {flat.length === 0 ? <div className="cmdk-empty">No matches. Try a repo name, a decision, or “replay”.</div> : null}
          {groups.map(([label, items]) => (
            <div className="cmdk-group" key={label}>
              <div className="cmdk-glabel">{label}</div>
              {items.map((it) => {
                idx += 1; const me = idx;
                return (
                  <div key={it.id} className={"cmdk-row" + (active === me ? " on" : "")} onMouseEnter={() => setActive(me)} onClick={() => run(it)}>
                    <span className="cmdk-ic" style={it.color ? { color: it.color } : null}><MIcon name={it.icon} size={16} /></span>
                    <span className="cmdk-t">{it.title}</span>
                    {it.sub ? <span className="cmdk-sub">{it.sub}</span> : null}
                    {it.node || it.repo ? <span className="cmdk-fly"><MIcon name="target" size={13} /> fly</span> : null}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
        <div className="cmdk-foot">
          <span><b>↑↓</b> move</span><span><b>↵</b> select</span><span><b>esc</b> close</span>
          <span style={{ marginLeft: "auto" }}>Shift-drag the constellation to lasso-select</span>
        </div>
      </div>
    </div>
  );
}

/* ---------------- Lasso selection HUD ---------------- */
function summarize(ids) {
  const ns = ids.map((id) => MEM.byId[id]);
  const byLane = {}, byTenant = {}, byTrust = {};
  let advisory = 0, superseded = 0, contradictions = 0;
  ns.forEach((n) => {
    byLane[n.lane] = (byLane[n.lane] || 0) + 1;
    byTenant[n.tenant] = (byTenant[n.tenant] || 0) + 1;
    byTrust[n.trust] = (byTrust[n.trust] || 0) + 1;
    if (n.trust === "InferredAdvisory") advisory++;
    if (n.status === "Superseded") superseded++;
    if (n.belnap) contradictions++;
  });
  const topLane = Object.entries(byLane).sort((a, b) => b[1] - a[1])[0];
  return { count: ns.length, tenants: Object.keys(byTenant).length, advisory, superseded, contradictions, topLane, byLane };
}

function SelectionHUD({ ids, onClear, onAction }) {
  if (!ids || !ids.length) return null;
  const s = summarize(ids);
  return (
    <div className="selhud panel">
      <div className="selhud-h">
        <div className="selhud-count"><span className="tnum">{s.count}</span> selected</div>
        <span className="selhud-meta">across {s.tenants} {s.tenants === 1 ? "repo" : "repos"}</span>
        <span className="collapse-btn" style={{ marginLeft: "auto" }} title="Clear selection" onClick={onClear}><MIcon name="x" size={14} /></span>
      </div>
      <div className="selhud-bars">
        {MEM.LANES.filter((l) => s.byLane[l.id]).map((l) => (
          <div className="selhud-bar" key={l.id} title={l.label + " · " + s.byLane[l.id]}>
            <span className="sb-fill" style={{ width: Math.max(8, (s.byLane[l.id] / s.count) * 100) + "%", background: l.color }}></span>
            <span className="sb-l">{l.label}</span><span className="sb-n tnum">{s.byLane[l.id]}</span>
          </div>
        ))}
      </div>
      {(s.advisory || s.superseded || s.contradictions) ? (
        <div className="selhud-flags">
          {s.advisory ? <span className="tag plane-asserted" style={{ padding: "2px 8px" }}>{s.advisory} advisory</span> : null}
          {s.superseded ? <span className="tag status-superseded" style={{ padding: "2px 8px" }}>{s.superseded} superseded</span> : null}
          {s.contradictions ? <span className="tag" style={{ padding: "2px 8px", color: "#f0907f", borderColor: "rgba(210,60,40,.4)", background: "rgba(210,60,40,.08)" }}>{s.contradictions} contradiction{s.contradictions > 1 ? "s" : ""}</span> : null}
        </div>
      ) : null}
      <div className="selhud-actions">
        <button className="act primary" onClick={() => onAction("ask", ids)}><MIcon name="spark" size={14} />Ask about these {s.count}</button>
        <button className="act" onClick={() => onAction("focus", ids)}><MIcon name="target" size={14} />Focus these</button>
        {s.advisory ? <button className="act" onClick={() => onAction("confirm", ids)}><MIcon name="check" size={14} />Confirm {s.advisory} advisory</button> : null}
        <button className="act" onClick={() => onAction("propose", ids)}><MIcon name="plus" size={14} />Propose cluster edge</button>
        <button className="act" onClick={() => onAction("export", ids)}><MIcon name="download" size={14} />Export</button>
      </div>
    </div>
  );
}

Object.assign(window, { CommandPalette, SelectionHUD, summarize });
