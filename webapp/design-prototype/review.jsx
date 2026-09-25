/* Memrizz — HIC Review Center (proposals · contradictions · supersessions · critic) */

function NodeMini({ node, onCite }) {
  const c = MEM.laneColor(node.lane);
  return (
    <div className="rv-chip-node" onClick={() => onCite(node.id)} title={node.title}>
      <span className="nd" style={{ background: c, color: c }}></span>
      <div style={{ minWidth: 0 }}>
        <div className="nt">{node.title}</div>
        <div className="nk">{node.kind} · {node.tenant}</div>
      </div>
    </div>
  );
}

function ResolvedTag({ state }) {
  const map = { confirmed: ["check", "Confirmed", "var(--citrate-green)"], rejected: ["x", "Rejected", "#f0907f"], asked: ["info", "Sent back for evidence", "var(--citrate-yellow)"], escalated: ["info", "Escalated", "var(--citrate-yellow)"] };
  const [ic, label, col] = map[state] || map.confirmed;
  return <span className="rv-resolved-tag" style={{ color: col }}><MIcon name={ic} size={15}/>{label}</span>;
}

function ProposalCard({ p, resolved, onResolve, onCite }) {
  return (
    <div className={"rv-card" + (resolved ? " resolved" : "")}>
      <div className="rv-top">
        <div className="rv-prevpair">
          <NodeMini node={p.a} onCite={onCite}/>
          <div className="rv-rel"><span>{p.relation}</span><span className="arr">→</span></div>
          <NodeMini node={p.b} onCite={onCite}/>
        </div>
      </div>
      <div className="rv-meta">
        <span>Proposed by <b>{p.proposer}</b></span>
        <span>{MEM.fmtAgo(p.when)}</span>
        <span>confidence <b>{p.confidence}%</b></span>
        <span><span className="tag plane-asserted" style={{ padding: "1px 7px" }}>Quarantined</span></span>
      </div>
      <div className="rv-rationale"><b style={{ color: "var(--ondark)" }}>Why: </b>{p.rationale}</div>
      <div className="rv-actions">
        {resolved
          ? <ResolvedTag state={resolved}/>
          : <React.Fragment>
              <button className="act primary" onClick={() => onResolve(p.id, "confirmed")}><MIcon name="check" size={14}/>Confirm — make load-bearing</button>
              <button className="act" onClick={() => onResolve(p.id, "asked")}><MIcon name="info" size={14}/>Ask for more evidence</button>
              <button className="act" onClick={() => onResolve(p.id, "rejected")}><MIcon name="x" size={14}/>Reject</button>
            </React.Fragment>}
      </div>
    </div>
  );
}

function ContradictionCard({ c, resolved, onResolve, onCite }) {
  return (
    <div className={"rv-card" + (resolved ? " resolved" : "")}>
      <div className="cx-claims">
        <div className="cx-claim">
          <div className="cl-who">Asserted by {c.assertedByA}</div>
          <div className="cl-t">{c.claimA}</div>
          <div style={{ marginTop: 9 }}><span className="act" style={{ padding: "5px 9px" }} onClick={() => onCite(c.a.id)}>View in constellation</span></div>
        </div>
        <div className="cx-vs"><div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 4 }}><span style={{ color: "#f0907f" }}><MIcon name="x" size={18}/></span>conflicts</div></div>
        <div className="cx-claim">
          <div className="cl-who">Asserted by {c.assertedByB}</div>
          <div className="cl-t">{c.claimB}</div>
          <div style={{ marginTop: 9 }}><span className="act" style={{ padding: "5px 9px" }} onClick={() => onCite(c.b.id)}>View in constellation</span></div>
        </div>
      </div>
      <div className="rv-meta"><span>Belnap value <b>Both</b> · detected {MEM.fmtAgo(c.when)}</span><span>repo <b>{c.a.tenant}</b></span></div>
      <div className="rv-actions">
        {resolved
          ? <ResolvedTag state={resolved}/>
          : <React.Fragment>
              <button className="act primary" onClick={() => onResolve(c.id, "confirmed")}><MIcon name="check" size={14}/>Keep first, supersede second</button>
              <button className="act" onClick={() => onResolve(c.id, "confirmed")}><MIcon name="check" size={14}/>Keep second</button>
              <button className="act" onClick={() => onResolve(c.id, "escalated")}><MIcon name="info" size={14}/>Escalate to an admin</button>
            </React.Fragment>}
      </div>
    </div>
  );
}

function SupersessionCard({ s, resolved, onResolve, onCite }) {
  return (
    <div className={"rv-card" + (resolved ? " resolved" : "")}>
      <div className="rv-top">
        <div className="rv-prevpair">
          <NodeMini node={s.old} onCite={onCite}/>
          <div className="rv-rel"><span>superseded by</span><span className="arr">→</span></div>
          <NodeMini node={s.neu} onCite={onCite}/>
        </div>
      </div>
      <div className="rv-meta"><span>Pending since {MEM.fmtAgo(s.when)}</span><span>repo <b>{s.old.tenant}</b></span><span><span className="tag status-superseded" style={{ padding: "1px 7px" }}>Superseded</span></span></div>
      <div className="rv-actions">
        {resolved
          ? <ResolvedTag state={resolved}/>
          : <React.Fragment>
              <button className="act primary" onClick={() => onResolve(s.id, "confirmed")}><MIcon name="check" size={14}/>Confirm supersession</button>
              <button className="act" onClick={() => onResolve(s.id, "rejected")}><MIcon name="x" size={14}/>Flag as wrong</button>
            </React.Fragment>}
      </div>
    </div>
  );
}

function CriticCard({ ct, resolved, onResolve, onCite }) {
  const sev = { high: "#f0907f", medium: "var(--citrate-yellow)", low: "var(--ondark-2)" }[ct.severity];
  return (
    <div className={"rv-card" + (resolved ? " resolved" : "")}>
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 9 }}>
        <span style={{ color: sev }}><MIcon name="info" size={17}/></span>
        <div style={{ fontSize: 14.5, fontWeight: 600, color: "var(--ondark)" }}>{ct.title}</div>
        <span className="role-badge" style={{ marginLeft: "auto", color: sev, borderColor: sev + "55" }}>{ct.severity}</span>
      </div>
      <div style={{ fontSize: 12.5, color: "var(--ondark-2)", lineHeight: 1.55, marginBottom: 12 }}>{ct.detail}</div>
      <div className="rv-meta"><span>relates to <b onClick={() => onCite(ct.node.id)} style={{ cursor: "pointer", color: "var(--citrate-green)" }}>{ct.node.title}</b></span></div>
      <div className="rv-actions">
        {resolved
          ? <ResolvedTag state={resolved}/>
          : <React.Fragment>
              <button className="act primary" onClick={() => onResolve(ct.id, "confirmed")}><MIcon name="history" size={14}/>Re-run with the missing sources</button>
              <button className="act" onClick={() => onResolve(ct.id, "rejected")}><MIcon name="x" size={14}/>Dismiss</button>
            </React.Fragment>}
      </div>
    </div>
  );
}

function ReviewCenter({ onCite, onToast }) {
  const [tab, setTab] = React.useState("proposals");
  const [resolved, setResolved] = React.useState({});
  const seqRef = React.useRef(48213);
  const R = MEM.review;

  function resolve(id, state) {
    setResolved((r) => ({ ...r, [id]: state }));
    const seq = ++seqRef.current;
    const verb = state === "confirmed" ? "Confirmed" : state === "rejected" ? "Rejected" : state === "asked" ? "Sent back" : "Escalated";
    onToast(verb + " · written to the audit chain", "AUDIT #" + seq);
  }
  const remaining = (arr) => arr.filter((x) => !resolved[x.id]).length;

  const tabs = [
    { id: "proposals", label: "Proposals", n: remaining(R.proposals) },
    { id: "contradictions", label: "Contradictions", n: remaining(R.contradictions) },
    { id: "supersessions", label: "Supersessions", n: remaining(R.supersessions) },
    { id: "critic", label: "Self-critic", n: remaining(R.critic) },
  ];

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Human In Control (HIC) · org-scoped</span>
          <h1>Review center</h1>
          <p>Nothing an AI proposes becomes load-bearing until a person confirms it. Witness each one — every decision is written to the tamper-evident audit chain.</p>
        </div>
        <div className="rv-tabs">
          {tabs.map((t) => (
            <div key={t.id} className={"rv-tab" + (tab === t.id ? " on" : "")} onClick={() => setTab(t.id)}>
              {t.label}{t.n ? <span className="num">{t.n}</span> : <MIcon name="check" size={13}/>}
            </div>
          ))}
        </div>
        <div className="rv-list">
          {tab === "proposals" && R.proposals.map((p) => <ProposalCard key={p.id} p={p} resolved={resolved[p.id]} onResolve={resolve} onCite={onCite}/>)}
          {tab === "contradictions" && R.contradictions.map((c) => <ContradictionCard key={c.id} c={c} resolved={resolved[c.id]} onResolve={resolve} onCite={onCite}/>)}
          {tab === "supersessions" && R.supersessions.map((s) => <SupersessionCard key={s.id} s={s} resolved={resolved[s.id]} onResolve={resolve} onCite={onCite}/>)}
          {tab === "critic" && R.critic.map((ct) => <CriticCard key={ct.id} ct={ct} resolved={resolved[ct.id]} onResolve={resolve} onCite={onCite}/>)}
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { ReviewCenter });
