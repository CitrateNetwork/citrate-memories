/* Memrizz — right-dock panels: Inspector (verify) + Ask (RAG) */

function verifyOf(n) {
  if (n.belnap) return { cls: "bad", icon: "x", head: "Contradiction recorded",
    reasons: ["Two assertions about this conflict (Belnap “Both”).", "Resolve it in the Review Center before relying on it."] };
  if (n.status === "Superseded") {
    const sup = n.supersededBy && MEM.byId[n.supersededBy];
    return { cls: "warn", icon: "history", head: "Superseded — newer memory exists",
      reasons: [sup ? `Replaced by “${sup.title}”.` : "A newer version has replaced this.", "Kept for the record; not load-bearing."] };
  }
  if (n.trust === "InferredAdvisory") return { cls: "warn", icon: "spark", head: "Advisory — not yet confirmed",
    reasons: ["An AI proposed this; it is quarantined.", "It is advisory only until a human confirms it."] };
  if (n.plane === "Derived") return { cls: "ok", icon: "shield", head: "Trustworthy by construction",
    reasons: ["Rebuilt deterministically from git/markdown.", "No signature needed — this is the record."] };
  if (n.trust === "HumanConfirmed") return { cls: "ok", icon: "check", head: "Signature valid · human-confirmed",
    reasons: ["Signed by its author and confirmed by a person.", "Load-bearing assertion."] };
  return { cls: "ok", icon: "check", head: "Signature valid · a claim",
    reasons: ["Signed and authentic.", "This is what someone said — not the deterministic record."] };
}

function Inspector({ node, onNeighbor, onClose, onAction, hops, onHops }) {
  if (!node) {
    return (
      <div className="dock-body" style={{ display: "grid", placeItems: "center", textAlign: "center", padding: 30 }}>
        <div style={{ maxWidth: 240 }}>
          <div style={{ color: "var(--ondark-3)", marginBottom: 12 }}><MIcon name="target" size={28}/></div>
          <div style={{ fontFamily: "var(--font-display)", fontSize: 17, color: "var(--ondark)", marginBottom: 6 }}>Nothing selected</div>
          <div style={{ fontSize: 12.5, color: "var(--ondark-2)", lineHeight: 1.5 }}>Click any point in the constellation — or a citation in an answer — to inspect a memory and check how trustworthy it is.</div>
        </div>
      </div>
    );
  }
  const color = MEM.laneColor(node.lane);
  const v = verifyOf(node);
  const nb = (MEM.adj[node.id] || []).slice(0, 8).map(a => ({ ...a, node: MEM.byId[a.o], edge: MEM.edges[a.e] }));
  const lane = MEM.LANES[node.laneIdx];
  return (
    <div className="dock-body">
      <div className="insp-pad">
        <div className="insp-kind">
          <span className="kdot" style={{ background: color, color }}></span>
          <span className="eyebrow" style={{ color }}>{node.kind}</span>
          <span style={{ marginLeft: "auto", cursor: "pointer", color: "var(--ondark-3)" }} onClick={onClose}><MIcon name="x" size={15}/></span>
        </div>
        <div className="insp-title">{node.title}</div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 14 }}>
          <PlaneBadge plane={node.plane}/>
          <TrustChip trust={node.trust} withTip/>
          <StatusPill status={node.status}/>
          {node.belnap ? <span className="tag" style={{ color: "#f08", borderColor: "rgba(210,60,40,.4)", background: "rgba(210,60,40,.08)" }}><span className="d" style={{ background: "#e23a28" }}></span>Contradiction</span> : null}
        </div>

        <div className="insp-grid">
          <span className="k">Repo</span><span className="v">{node.tenant}</span>
          <span className="k">Material</span><span className="v" style={{ color }}>{lane.label} — {lane.desc}</span>
          <span className="k">Recorded</span><span className="v tnum">{MEM.fmtDate(node.when)} · {MEM.fmtAgo(node.when)}</span>
          <span className="k">Address</span><span className="v mono" style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{node.hash}</span>
        </div>

        <div className={"verdict " + v.cls}>
          <div className="vh" style={{ color: v.cls === "ok" ? "var(--citrate-green)" : v.cls === "warn" ? "#f3d27a" : "#f0907f" }}>
            <MIcon name={v.icon} size={16}/>{v.head}
          </div>
          <ul className="vr">{v.reasons.map((r, i) => <li key={i}>{r}</li>)}</ul>
        </div>

        {node.plane === "Derived" ? (
          <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 11.5, color: "var(--ondark-2)", marginBottom: 4 }}>
            <MIcon name="branch" size={14}/>
            <span>Source: <span className="mono" style={{ color: "var(--ondark)" }}>{node.tenant}@{node.hash.slice(4, 11)}</span></span>
            <span style={{ marginLeft: "auto", color: "var(--ondark-3)" }}>we point, never copy →</span>
          </div>
        ) : null}

        <div className="sec-label">Neighbors · blast radius</div>
        {nb.length ? nb.map((x, i) => {
          const c = MEM.laneColor(x.node.lane);
          const rel = x.edge.quarantined ? "proposed" : x.edge.kind;
          return (
            <div className="neighbor" key={i} onClick={() => onNeighbor(x.node.id)}>
              <span className="ndot" style={{ background: c, boxShadow: `0 0 7px 0 ${c}` }}></span>
              <span className="nt">{x.node.title}</span>
              <span className="nk" style={{ color: x.edge.quarantined ? "var(--citrate-yellow)" : "var(--ondark-3)" }}>{x.dir === "out" ? "→" : "←"} {rel}</span>
            </div>
          );
        }) : <div style={{ fontSize: 12, color: "var(--ondark-3)" }}>No recorded neighbors.</div>}

        <div className="hops-ctl">
          <span className="hops-l"><MIcon name="target" size={13}/> Blast radius</span>
          <div className="seg-pick" style={{ marginLeft: "auto" }}>
            {[1, 2, 3].map((h) => <button key={h} className={hops === h ? "on" : ""} onClick={() => onHops(h)}>{h} hop{h > 1 ? "s" : ""}</button>)}
          </div>
        </div>
        <div className="act-row">
          <button className="act primary" onClick={() => onAction("discuss", node)}><MIcon name="spark" size={14}/>Ask about this</button>
          <button className="act" onClick={() => onAction("neighbors", node)}><MIcon name="target" size={14}/>Focus neighborhood</button>
          <button className="act" onClick={() => onAction("analogues", node)}><MIcon name="spark" size={14}/>Find analogues</button>
          <button className="act" onClick={() => onAction("propose", node)}><MIcon name="plus" size={14}/>Propose edge</button>
        </div>
      </div>
    </div>
  );
}

/* ---------------- Ask ---------------- */
function CitationChip({ cite, onClick, flash, mode }) {
  const n = MEM.byId[cite.id];
  const color = n ? MEM.laneColor(n.lane) : "#8ecc09";
  return (
    <span className={"cite" + (flash ? " flash" : "")} onClick={() => onClick(cite.id)} title={n ? n.title + (n ? " · " + n.tenant : "") : ""}>
      <span className="cd" style={{ background: color, color }}></span>
      {cite.label}
      {mode === "tech" && n ? <span className="cite-hash">{n.hash.slice(0, 9)}</span> : null}
    </span>
  );
}

function AnswerBody({ frags, onCite, flashId, mode }) {
  return (
    <span>
      {frags.map((f, i) => typeof f === "string"
        ? <span key={i} dangerouslySetInnerHTML={{ __html: f }} />
        : <CitationChip key={i} cite={f} onClick={onCite} flash={flashId === f.id} mode={mode} />)}
    </span>
  );
}

function AskMessage({ m, onCite, flashId, mode }) {
  const [showWork, setShowWork] = React.useState(mode === "tech");
  if (m.role === "user") return <div className="msg-user">{m.text}</div>;
  if (m.pending) return <div className="msg-ai"><div className="consulting"><Spinner/> {m.tool ? m.tool : "consulting memory…"}</div></div>;
  return (
    <div className="msg-ai">
      {m.ctxNote ? <div style={{ fontFamily: "var(--font-mono)", fontSize: 10, letterSpacing: "0.08em", textTransform: "uppercase", color: "var(--ondark-3)", marginBottom: 7 }}>{m.ctxNote}</div> : null}
      <div className="body"><AnswerBody frags={m.frags} onCite={onCite} flashId={flashId} mode={mode}/></div>
      {m.headsup ? (
        <div className="headsup">
          <div className="hh"><MIcon name="info" size={14}/>Heads-up — what this answer may be missing</div>
          <div className="hb">{m.headsup}</div>
        </div>
      ) : null}
      {m.work ? (
        <div className="work">
          <div className="wh" onClick={() => setShowWork(s => !s)}>
            <MIcon name={showWork ? "chevdown" : "chevright"} size={13}/> show your work · {m.work.length} memory tools
          </div>
          {showWork ? (
            <div className="wb">
              {m.work.map((w, i) => <div className="wstep" key={i}><span className="wc"><MIcon name="check" size={12}/></span>{w}</div>)}
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function Spinner() {
  return <span style={{ display: "inline-block", width: 13, height: 13, border: "2px solid rgba(205,231,214,.25)", borderTopColor: "var(--citrate-green)", borderRadius: "50%", animation: "spin .7s linear infinite" }}></span>;
}

function ToolsPop({ onTool, onClose, mode }) {
  return (
    <div className="tools-pop" onMouseLeave={onClose}>
      <div className="tp-h">Memory tools the model can call</div>
      {MEM.TOOLS.map((t) => (
        <div className="tool-item" key={t.id} onClick={() => { onTool(t); onClose(); }}>
          <span className="ti-ic"><MIcon name={t.icon} size={15}/></span>
          <div>
            <div className="ti-t">{t.plain}{mode === "tech" ? <span className="ti-tech">{t.tech}</span> : null}</div>
            <div className="ti-d">{t.desc}</div>
          </div>
        </div>
      ))}
    </div>
  );
}

function Ask({ thread, onAsk, onCite, flashId, model, onModel, suggestions, mode, onMode, context, onAddCtx, onRemoveCtx, onTool }) {
  const [val, setVal] = React.useState("");
  const [toolsOpen, setToolsOpen] = React.useState(false);
  const endRef = React.useRef(null);
  React.useEffect(() => { if (endRef.current) endRef.current.scrollTop = endRef.current.scrollHeight; }, [thread]);
  function submit(text) { const t = (text != null ? text : val).trim(); if (!t) return; setVal(""); onAsk(t); }
  return (
    <div className="ask-wrap">
      <div className="ask-head">
        <div className="modelpick" onClick={onModel}><span className="mdot"></span>{model}<span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={13}/></span></div>
        <div className="mode-toggle" style={{ marginLeft: "auto" }} title="How much detail to show">
          <button className={mode === "plain" ? "on" : ""} onClick={() => onMode("plain")}>Plain</button>
          <button className={mode === "tech" ? "on" : ""} onClick={() => onMode("tech")}>Technical</button>
        </div>
      </div>
      <div className="ask-thread" ref={endRef}>
        {thread.map((m, i) => <AskMessage key={i} m={m} onCite={onCite} flashId={flashId} mode={mode}/>)}
      </div>
      {context.length ? (
        <div className="ask-context">
          <div className="ctx-head">
            <span className="ch-l">Grounding · {context.length} {context.length === 1 ? "memory" : "memories"}</span>
            <button className="ctx-add" onClick={onAddCtx}><MIcon name="plus" size={12}/>add</button>
          </div>
          <div className="ctx-chips">
            {context.map((n) => (
              <span className="ctx-chip" key={n.id}>
                <span className="nd" style={{ background: MEM.laneColor(n.lane) }}></span>
                <span className="nm">{n.title}</span>
                <span className="rm" onClick={() => onRemoveCtx(n.id)}><MIcon name="x" size={12}/></span>
              </span>
            ))}
          </div>
        </div>
      ) : null}
      <div className="ask-input" style={{ position: "relative" }}>
        {thread.length <= 1 && !context.length ? (
          <div className="ask-suggest">
            {suggestions.map((s, i) => <span className="sugg" key={i} onClick={() => submit(s.q)}>{s.label}</span>)}
            <span className="sugg" onClick={onAddCtx} style={{ color: "var(--citrate-green)" }}>+ Add a memory to ground on</span>
          </div>
        ) : null}
        <div className="ask-box">
          <button className="tools-btn" onClick={() => setToolsOpen((v) => !v)} title="Memory tools"><MIcon name="sliders" size={15}/></button>
          <button className="tools-btn" onClick={onAddCtx} title="Pull a memory into the conversation"><MIcon name="plus" size={16}/></button>
          <input value={val} placeholder={context.length ? "Ask about the memories above…" : "Ask the org's memory anything…"}
            onChange={e => setVal(e.target.value)} onKeyDown={e => { if (e.key === "Enter") submit(); }} />
          <button className="send-btn" onClick={() => submit()}><MIcon name="send" size={15}/></button>
        </div>
        {toolsOpen ? <ToolsPop mode={mode} onTool={onTool} onClose={() => setToolsOpen(false)} /> : null}
      </div>
    </div>
  );
}

/* ---------------- Memory picker ---------------- */
function MemoryPicker({ onClose, onAdd, preselected }) {
  const [q, setQ] = React.useState("");
  const [sel, setSel] = React.useState(new Set(preselected || []));
  const results = React.useMemo(() => {
    const words = q.toLowerCase().split(/[^a-z0-9]+/).filter((w) => w.length > 1);
    let list;
    if (words.length) {
      list = MEM.nodes.map((n) => {
        const t = (n.title + " " + n.kind + " " + n.tenant).toLowerCase();
        let s = 0; words.forEach((w) => { if (t.includes(w)) s += 1; });
        return { n, s: s + (n.status === "Active" ? 0.2 : 0) + n.deg * 0.01 };
      }).filter((x) => x.s > 0).sort((a, b) => b.s - a.s).map((x) => x.n);
    } else {
      list = MEM.nodes.slice().sort((a, b) => b.deg - a.deg);
    }
    return list.slice(0, 50);
  }, [q]);
  function toggle(id) { setSel((s) => { const n = new Set(s); n.has(id) ? n.delete(id) : n.add(id); return n; }); }
  return (
    <div className="modal-back" onClick={onClose}>
      <div className="picker panel" onClick={(e) => e.stopPropagation()}>
        <div className="picker-h">
          <MIcon name="layers" size={17}/>
          <span className="t">Pull memories into the conversation</span>
          <span className="collapse-btn" style={{ marginLeft: "auto" }} onClick={onClose}><MIcon name="x" size={14}/></span>
        </div>
        <div className="picker-search">
          <MIcon name="search" size={16}/>
          <input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search by topic, repo, decision, file…" />
        </div>
        <div className="picker-scope"><MIcon name="shield" size={13}/> Searching only memories you can read · <b style={{ color: "var(--ondark)" }}>&nbsp;14 repos</b></div>
        <div className="picker-list">
          {results.map((n) => (
            <div className={"pick-row" + (sel.has(n.id) ? " sel" : "")} key={n.id} onClick={() => toggle(n.id)}>
              <span className="pk-box">{sel.has(n.id) ? <MIcon name="check" size={13}/> : null}</span>
              <span className="pk-d" style={{ background: MEM.laneColor(n.lane), color: MEM.laneColor(n.lane) }}></span>
              <span className="pk-t">{n.title}</span>
              <span className="pk-m">{n.kind} · {n.tenant}</span>
            </div>
          ))}
        </div>
        <div className="picker-foot">
          <span style={{ fontSize: 12.5, color: "var(--ondark-2)" }}>{sel.size} selected</span>
          <span style={{ flex: 1 }}></span>
          <button className="act" onClick={onClose}>Cancel</button>
          <button className="act primary" onClick={() => { onAdd([...sel]); onClose(); }}><MIcon name="plus" size={14}/>Add to conversation</button>
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { Inspector, Ask, CitationChip, verifyOf, Spinner, MemoryPicker, ToolsPop });
