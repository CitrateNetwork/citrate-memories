/* Memrizz — shared UI primitives, icons, chrome (exports to window) */

function MIcon({ name, size = 16, stroke = 1.6 }) {
  const s = { width: size, height: size, strokeWidth: stroke, fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeLinejoin: "round" };
  const p = {
    search:   <g><circle cx="10" cy="10" r="6"/><path d="M15 15 L21 21"/></g>,
    bell:     <g><path d="M18 16 V11 A6 6 0 0 0 6 11 V16 L4 18 H20 L18 16Z"/><path d="M10 21 A2 2 0 0 0 14 21"/></g>,
    chevdown: <path d="M6 9 L12 15 L18 9"/>,
    chevright:<path d="M9 6 L15 12 L9 18"/>,
    chevleft: <path d="M15 6 L9 12 L15 18"/>,
    layers:   <g><path d="M12 3 L21 8 L12 13 L3 8 Z"/><path d="M3 13 L12 18 L21 13"/></g>,
    filter:   <path d="M4 5 H20 M7 12 H17 M10 19 H14"/>,
    clock:    <g><circle cx="12" cy="12" r="9"/><path d="M12 7 V12 L15 14"/></g>,
    history:  <g><path d="M3 12 a9 9 0 1 0 3-6.7 M3 4 V8 H7"/><path d="M12 8 V12 L15 14"/></g>,
    spark:    <path d="M12 3 L13.6 9.4 L20 11 L13.6 12.6 L12 19 L10.4 12.6 L4 11 L10.4 9.4 Z"/>,
    sliders:  <g><path d="M4 8 H20 M4 16 H20"/><circle cx="9" cy="8" r="2.2"/><circle cx="15" cy="16" r="2.2"/></g>,
    eye:      <g><path d="M2 12 S6 5 12 5 S22 12 22 12 S18 19 12 19 S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></g>,
    send:     <path d="M4 12 L20 4 L14 20 L11 13 Z"/>,
    x:        <path d="M6 6 L18 18 M18 6 L6 18"/>,
    play:     <path d="M7 5 L19 12 L7 19 Z"/>,
    pause:    <g><path d="M8 5 V19 M16 5 V19"/></g>,
    check:    <path d="M5 12 L10 17 L19 8"/>,
    target:   <g><circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3"/><path d="M12 2 V5 M12 19 V22 M2 12 H5 M19 12 H22"/></g>,
    branch:   <g><circle cx="6" cy="6" r="2.4"/><circle cx="6" cy="18" r="2.4"/><circle cx="18" cy="10" r="2.4"/><path d="M6 8.4 V15.6 M6 12 H13 a3 3 0 0 0 3-3 V12.4"/></g>,
    shield:   <g><path d="M12 3 L20 6 V12 C20 17 16 21 12 22 C8 21 4 17 4 12 V6 Z"/><path d="M9 12 L11 14 L15 9"/></g>,
    plus:     <path d="M12 4 V20 M4 12 H20"/>,
    reset:    <g><path d="M3 12 a9 9 0 1 1 3 6.7 M3 16 V20 M3 20 H7"/></g>,
    info:     <g><circle cx="12" cy="12" r="9"/><path d="M12 11 V16 M12 8 H12.01"/></g>,
    grid:     <g><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/></g>,
    cube:     <g><path d="M12 3 L21 8 V16 L12 21 L3 16 V8 Z"/><path d="M3 8 L12 13 L21 8 M12 13 V21"/></g>,
    waves:    <path d="M3 8 Q7 4 12 8 T21 8 M3 14 Q7 10 12 14 T21 14"/>,
    help:     <g><circle cx="12" cy="12" r="9"/><path d="M9.5 9.5 A2.5 2.5 0 1 1 12 13 V14.5 M12 18 H12.01"/></g>,
    link:     <g><path d="M10 13 a4 4 0 0 0 6 0 l3-3 a4 4 0 0 0-6-6 l-1 1"/><path d="M14 11 a4 4 0 0 0-6 0 l-3 3 a4 4 0 0 0 6 6 l1-1"/></g>,
    ledger:   <g><rect x="5" y="3" width="14" height="18" rx="1.5"/><path d="M9 7 H15 M9 11 H15 M9 15 H13"/></g>,
    copy:     <g><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15 V5 a2 2 0 0 1 2-2 H15"/></g>,
    download: <g><path d="M12 3 V15 M7 11 L12 16 L17 11"/><path d="M4 20 H20"/></g>,
    refresh:  <g><path d="M3 12 a9 9 0 0 1 15-6.7 L21 8 M21 3 V8 H16"/><path d="M21 12 a9 9 0 0 1-15 6.7 L3 16 M3 21 V16 H8"/></g>,
  };
  return <svg viewBox="0 0 24 24" style={s}>{p[name] || null}</svg>;
}

function TrustChip({ trust, withTip }) {
  const t = MEM.TRUST[MEM.TRUST_IDX[trust]];
  const cls = trust === "DerivedDeterministic" ? "plane-derived"
    : trust === "InferredAdvisory" ? "plane-asserted" : "";
  return (
    <span className="tag" title={withTip ? t.tip : undefined}
      style={{ color: t.ring, borderColor: t.ring + "55", background: t.ring + "11" }}>
      <span className="d" style={{ background: t.ring, boxShadow: `0 0 6px 0 ${t.ring}` }}></span>{t.short}
    </span>
  );
}
function PlaneBadge({ plane }) {
  return <span className={"tag " + (plane === "Derived" ? "plane-derived" : "plane-asserted")}>{plane === "Derived" ? "The record" : "Asserted"}</span>;
}
function StatusPill({ status }) {
  const m = { Active: "status-active", Superseded: "status-superseded", Archived: "status-archived" }[status];
  return <span className={"tag " + m}>{status}</span>;
}

/* ---------------- Nav rail ---------------- */
function NavRail({ view, onView, reviewCount }) {
  const items = [
    { id: "constellation", icon: "layers", label: "Constellation" },
    { id: "ask", icon: "spark", label: "Ask" },
    { id: "review", icon: "shield", label: "Review", badge: reviewCount },
    { id: "connect", icon: "link", label: "Connect a model" },
    { id: "audit", icon: "ledger", label: "Audit log" },
  ];
  const bottom = [
    { id: "settings", icon: "sliders", label: "Settings" },
    { id: "profile", icon: "target", label: "Profile", avatar: true },
  ];
  const Btn = (it) => (
    <button key={it.id} className={"nav-btn" + (view === it.id ? " on" : "")} onClick={() => onView(it.id)}>
      {it.avatar
        ? <span className="avatar" style={{ width: 24, height: 24, borderRadius: 7, background: MEM.ME.color, fontSize: 12 }}>{MEM.ME.initials}</span>
        : <MIcon name={it.icon} size={20} />}
      {it.badge ? <span className="nb-badge">{it.badge}</span> : null}
      <span className="nav-tip">{it.label}</span>
    </button>
  );
  return (
    <div className="navrail">
      <img className="nav-mark" src="assets/citrate_mark_green.svg" alt="" />
      {items.map(Btn)}
      <div className="nav-sp"></div>
      {bottom.map(Btn)}
    </div>
  );
}

function NotifPop({ onClose, onGoReview }) {
  return (
    <div className="notif-pop" onMouseLeave={onClose}>
      <div className="np-h"><span className="t">Notifications</span><span className="eyebrow" style={{ marginLeft: "auto" }}>calm feed</span></div>
      {MEM.NOTIFS.map((n) => (
        <div className="notif-item" key={n.id} onClick={() => { if (n.kind === "proposal" || n.kind === "contradiction") onGoReview(); onClose(); }}>
          <span className="ni-ic"><MIcon name={n.icon} size={15} /></span>
          <div><div className="ni-t">{n.text}</div><div className="ni-s">{n.sub}</div><div className="ni-w">{n.when}</div></div>
        </div>
      ))}
    </div>
  );
}

/* ---------------- Top bar ---------------- */
function TopBar({ onOmni, model, onModel, onProfile, onGoReview }) {
  const [notif, setNotif] = React.useState(false);
  return (
    <div className="topbar">
      <div className="brand">
        <div className="name"><b>Memrizz</b></div>
      </div>
      <div className="divider-v"></div>
      <div className="switcher" title="Workspace — everything below is scoped to this Org">
        <span className="sw-dot">C</span>
        <div>
          <div className="sw-l1">Citrate Federation</div>
          <div className="sw-l2">Org · 14 repos</div>
        </div>
        <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={14}/></span>
      </div>
      <div className="omnibox" onClick={onOmni}>
        <MIcon name="search" size={15}/>
        <span style={{ fontSize: 13 }}>Search memory — filings, decisions, repos…</span>
        <span className="kbd">⌘K</span>
      </div>
      <div className="spacer"></div>
      <div className="modelpick" onClick={onModel} title="Model used to answer">
        <span className="mdot"></span>{model}
        <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={13}/></span>
      </div>
      <button className="tb-btn" style={{ position: "relative" }} title="Notifications" onClick={() => setNotif((v) => !v)}>
        <MIcon name="bell" size={16}/><span className="tb-badge"></span>
      </button>
      <div className="switcher" style={{ padding: "0 6px 0 9px" }} title="You" onClick={onProfile}>
        <span className="sw-dot" style={{ background: MEM.ME.color }}>{MEM.ME.initials}</span>
        <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={14}/></span>
      </div>
      {notif ? <NotifPop onClose={() => setNotif(false)} onGoReview={onGoReview} /> : null}
    </div>
  );
}

/* ---------------- Left rail ---------------- */
const LAYOUTS = [
  { id: "lattice", s1: "Lattice", s2: "Type · Time · Trust", icon: "grid" },
  { id: "galaxy",  s1: "Galaxy",  s2: "By meaning", icon: "spark" },
  { id: "islands", s1: "Islands", s2: "By repo", icon: "cube" },
  { id: "river",   s1: "River",   s2: "By time", icon: "waves" },
];

function FilterChip({ on, color, round, label, onClick }) {
  return (
    <span className={"fchip" + (on ? "" : " off") + (round ? " round" : "")} onClick={onClick}>
      {color ? <span className="sw" style={{ background: color, boxShadow: on ? `0 0 7px 0 ${color}` : "none" }}></span> : null}
      {label}
    </span>
  );
}

function LeftRail({ collapsed, onCollapse, mode, onMode, filters, setFilter, encoding }) {
  function toggleSet(key, val) {
    const cur = filters[key];
    const set = cur ? new Set(cur) : new Set(MEM[key === "lanes" ? "LANES" : key === "trusts" ? "TRUST" : "TENANTS"].map(x => x.id || x));
    // build full set lazily
    let full;
    if (key === "lanes") full = MEM.LANES.map(l => l.id);
    else if (key === "trusts") full = MEM.TRUST.map(t => t.id);
    else if (key === "statuses") full = Object.keys(MEM.STATUS);
    else full = MEM.TENANTS;
    const s = cur ? new Set(cur) : new Set(full);
    if (s.has(val)) s.delete(val); else s.add(val);
    setFilter(key, s.size === full.length ? null : s);
  }
  const isOn = (key, val, full) => { const c = filters[key]; return c ? c.has(val) : true; };

  if (collapsed) {
    return (
      <div className="leftrail collapsed panel" style={{ position: "absolute" }}>
        <div className="rail-toggle" onClick={onCollapse}><MIcon name="chevright" size={15}/></div>
        <div className="rail-icon-col">
          {LAYOUTS.map(l => (
            <button key={l.id} className={mode === l.id ? "on" : ""} title={l.s1} onClick={() => onMode(l.id)}><MIcon name={l.icon} size={18}/></button>
          ))}
          <div style={{ height: 1, background: "var(--hair)", width: 28, margin: "6px 0" }}></div>
          <button title="Filters" onClick={onCollapse}><MIcon name="filter" size={18}/></button>
        </div>
      </div>
    );
  }
  return (
    <div className="leftrail panel">
      <div className="rail-toggle" onClick={onCollapse}><MIcon name="chevleft" size={15}/></div>
      <div className="panel-h">
        <MIcon name="layers" size={16}/>
        <div className="t">Constellation</div>
      </div>
      <div className="rail-scroll">
        <div className="rail-sec">
          <div className="lbl"><span className="eyebrow">View</span></div>
          <div className="seg">
            {LAYOUTS.map(l => (
              <button key={l.id} className={"seg-btn" + (mode === l.id ? " on" : "")} onClick={() => onMode(l.id)}>
                <span style={{ display: "flex", alignItems: "center", gap: 6 }}><MIcon name={l.icon} size={14}/><span className="s1">{l.s1}</span></span>
                <span className="s2">{l.s2}</span>
              </button>
            ))}
          </div>
        </div>

        <div className="rail-sec">
          <div className="lbl"><span className="eyebrow">Material type</span><span className="eyebrow" style={{ color: encoding === "kind" ? "var(--citrate-green)" : "var(--ondark-3)" }}>{encoding === "kind" ? "● color" : ""}</span></div>
          <div className="chips">
            {MEM.LANES.map(l => (
              <FilterChip key={l.id} on={isOn("lanes", l.id)} color={l.color} label={l.label} onClick={() => toggleSet("lanes", l.id)} />
            ))}
          </div>
        </div>

        <div className="rail-sec">
          <div className="lbl"><span className="eyebrow">Trust</span><span className="eyebrow" style={{ color: encoding === "trust" ? "var(--citrate-green)" : "var(--ondark-3)" }}>{encoding === "trust" ? "● color" : ""}</span></div>
          <div className="chips">
            {MEM.TRUST.map(t => (
              <FilterChip key={t.id} on={isOn("trusts", t.id)} color={t.ring} round label={t.short} onClick={() => toggleSet("trusts", t.id)} />
            ))}
          </div>
        </div>

        <div className="rail-sec">
          <div className="lbl"><span className="eyebrow">Status</span></div>
          <div className="chips">
            {Object.keys(MEM.STATUS).map(s => (
              <FilterChip key={s} on={isOn("statuses", s)} label={s} onClick={() => toggleSet("statuses", s)} />
            ))}
          </div>
          <div className="toggle-row" style={{ marginTop: 10 }}>
            <span className="tl">Show proposed (advisory)</span>
            <span className={"sw-toggle" + (filters.showQuarantined ? " on" : "")} onClick={() => setFilter("showQuarantined", !filters.showQuarantined)}></span>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ---------------- Time scrubber ---------------- */
function TimeScrubber({ t, onT, playing, onPlay, onCollapse }) {
  const trackRef = React.useRef(null);
  const cutoff = MEM.T0 + t * MEM.span;
  const plain = new Date(cutoff).toLocaleDateString("en-US", { month: "long", day: "numeric", year: "numeric" });
  const isNow = t > 0.992;
  function fromEvent(e) {
    const r = trackRef.current.getBoundingClientRect();
    return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
  }
  function down(e) {
    onPlay(false);
    const move = (ev) => onT(fromEvent(ev));
    const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
    window.addEventListener("pointermove", move); window.addEventListener("pointerup", up);
    onT(fromEvent(e));
  }
  const ticks = ["Jan ’25", "Apr", "Jul", "Oct", "Jan ’26", "Jun"];
  return (
    <div className="scrubber panel">
      <div className="scrub-head">
        <div className="plain">{isNow ? <span>Memory as it is <b>now</b></span> : <span>Rewound to <b>{plain}</b></span>}</div>
        <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
          <div className="scrub-play" onClick={() => onPlay(!playing)}>
            <MIcon name={playing ? "pause" : "history"} size={14}/>{playing ? "Pause" : "Replay history"}
          </div>
          <span className="collapse-btn" title="Hide timeline" onClick={onCollapse}><MIcon name="x" size={14}/></span>
        </div>
      </div>
      <div className="scrub-track">
        <div className="scrub-ticks">{ticks.map((tk, i) => <span key={i} className="scrub-tick">{tk}</span>)}</div>
        <div className="scrub-rail" ref={trackRef} onPointerDown={down}>
          <div className="scrub-fill" style={{ width: (t * 100) + "%" }}></div>
        </div>
        <div className="scrub-knob" style={{ left: (t * 100) + "%" }} onPointerDown={down}></div>
      </div>
    </div>
  );
}

/* ---------------- HUD + tooltip + guide ---------------- */
/* collapsed timeline pill + dock icons */
function ScrubMini({ t, onOpen }) {
  const cutoff = MEM.T0 + t * MEM.span;
  const isNow = t > 0.992;
  const d = new Date(cutoff).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "2-digit" });
  return (
    <div className="scrubmini panel" onClick={onOpen} title="Show memory timeline">
      <MIcon name="history" size={15}/>
      <span className="sm-d">{isNow ? <span>Memory · <b>now</b></span> : <span><b>{d}</b></span>}</span>
      <MIcon name="chevright" size={13}/>
    </div>
  );
}
function DockMini({ onOpen, reviewSel }) {
  return (
    <div className="dockmini">
      <button className="tb-btn dm-btn panel" title="Ask" onClick={() => onOpen("ask")}><MIcon name="spark" size={19}/></button>
      <button className="tb-btn dm-btn panel" title="Inspect" onClick={() => onOpen("inspect")}>
        <MIcon name="shield" size={19}/>{reviewSel ? <span className="num" style={{ position: "absolute", top: 5, right: 5, background: "rgba(255,189,16,.16)", color: "var(--citrate-yellow)", borderRadius: 99, padding: "0 5px", fontFamily: "var(--font-mono)", fontSize: 9 }}>1</span> : null}</button>
    </div>
  );
}

function HUD({ visible, total, mode }) {
  return (
    <div className="hud">
      <span className="stat"><b className="tnum">{MEM.stats.nodes.toLocaleString()}</b> nodes</span>
      <span className="stat">·</span>
      <span className="stat"><b className="tnum">{MEM.stats.edges.toLocaleString()}</b> edges</span>
      <span className="stat">·</span>
      <span className="stat"><b className="tnum">{visible}</b> in view</span>
    </div>
  );
}

function Tooltip({ data }) {
  if (!data || !data.node) return null;
  const n = data.node;
  const color = MEM.laneColor(n.lane);
  return (
    <div className="tip" style={{ left: Math.min(data.x + 16, window.innerWidth - 260), top: data.y + 16 }}>
      <div className="tt">{n.title}</div>
      <div className="tm">
        <span><b style={{ color }}>{n.kind}</b></span>
        <span>{n.tenant}</span>
        <span>{MEM.TRUST[n.trustIdx].short}</span>
        <span>{MEM.fmtAgo(n.when)}</span>
      </div>
    </div>
  );
}

function GuidedOverlay({ onStart }) {
  return (
    <div className="guide-back">
      <div className="guide panel">
        <div className="ge eyebrow">Citrate Federation · Organization memory</div>
        <h2>Everything your org remembers, as one living constellation.</h2>
        <p>This is the memory of 14 repositories — every decision, document, commit and claim, held as a map you can fly through and simply <i>ask</i>. Nothing here is guessed at you: each point shows how sure it is and how current.</p>
        <div className="gsteps">
          <div className="gstep">
            <div className="gi" style={{ color: "var(--citrate-green)" }}><MIcon name="target" size={16}/></div>
            <div><div className="gtt">See it</div><div className="gtd">Drag to orbit, scroll to zoom. Each glowing point is a memory; brighter means more trusted.</div></div>
          </div>
          <div className="gstep">
            <div className="gi" style={{ color: "var(--citrate-yellow)" }}><MIcon name="spark" size={16}/></div>
            <div><div className="gtt">Ask it</div><div className="gtd">Ask a plain-language question. The answer cites real memories — click a citation and it flies into view.</div></div>
          </div>
          <div className="gstep">
            <div className="gi" style={{ color: "#9fc0e8" }}><MIcon name="history" size={16}/></div>
            <div><div className="gtt">Rewind it</div><div className="gtd">Drag the timeline at the bottom to watch the memory breathe through history.</div></div>
          </div>
        </div>
        <div className="gactions">
          <button className="btn-primary-lg" onClick={onStart}>Enter the constellation</button>
          <button className="link-muted" onClick={onStart}>Skip the tour</button>
        </div>
      </div>
    </div>
  );
}

Object.assign(window, {
  MIcon, TrustChip, PlaneBadge, StatusPill, TopBar, LeftRail, LAYOUTS,
  TimeScrubber, HUD, Tooltip, GuidedOverlay, FilterChip,
  NavRail, NotifPop, ScrubMini, DockMini,
});
