/* Memrizz — Connect (BYOM / MCP) + Audit log viewer */

function copyText(t, onToast) { try { navigator.clipboard && navigator.clipboard.writeText(t); } catch (e) {} onToast && onToast("Copied to clipboard", ""); }

function CodeBlock({ text, onToast }) {
  return (
    <div className="code-block">
      <button className="icon-btn copy-fab" title="Copy" onClick={() => copyText(text, onToast)}><MIcon name="copy" size={15} /></button>
      <pre>{text}</pre>
    </div>
  );
}

const ENDPOINT = "https://mem-gateway.citrate.ai/mcp/u/aleia";
function tokenStr(n) { return "ctr_mcp_" + "live_sk_" + n + "x9f4a2"; }

function ConnectPage({ onToast }) {
  const [tok, setTok] = React.useState("ctr_mcp_live_sk_7f3a9c2e4b");
  const [client, setClient] = React.useState("desktop");
  const masked = tok.slice(0, 12) + "•".repeat(10) + tok.slice(-4);

  const configs = {
    desktop: `{
  "mcpServers": {
    "citrate-memory": {
      "url": "${ENDPOINT}",
      "headers": {
        "Authorization": "Bearer ${tok}"
      }
    }
  }
}`,
    code: `// .mcp.json  (project root)
{
  "mcpServers": {
    "citrate-memory": {
      "type": "http",
      "url": "${ENDPOINT}",
      "headers": { "Authorization": "Bearer ${tok}" }
    }
  }
}`,
    cursor: `// ~/.cursor/mcp.json
{
  "mcpServers": {
    "citrate-memory": {
      "url": "${ENDPOINT}",
      "headers": { "Authorization": "Bearer ${tok}" }
    }
  }
}`,
    generic: `# Streamable-HTTP MCP transport
curl -N ${ENDPOINT} \\
  -H "Authorization: Bearer ${tok}" \\
  -H "Accept: text/event-stream"

# tools exposed: recall · search · neighbors
#                verify · as_of · analogy · critique`,
  };
  const clients = [
    { id: "desktop", label: "Claude Desktop" },
    { id: "code", label: "Claude Code" },
    { id: "cursor", label: "Cursor" },
    { id: "generic", label: "Generic MCP" },
  ];
  const connected = [
    { name: "Claude Desktop", scope: "read · all 14 repos", last: "active now", live: true },
    { name: "Cursor · local Llama 4", scope: "read + propose · citrate-docs", last: "1h ago", live: false },
    { name: "Fable-5 · agent session", scope: "read · citrate-memories", last: "3h ago", live: false },
  ];

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Bring your own model · MCP over Streamable-HTTP</span>
          <h1>Connect a model</h1>
          <p>Point any MCP client — Claude, Cursor, a local model — at your personal, scoped endpoint. It can only touch what your capability grant allows, and every call is written to the audit chain under your name. The model never touches the raw store.</p>
        </div>

        <div className="cn-grid">
          <div className="cn-card">
            <h3>Your endpoint &amp; token</h3>
            <p className="sub">A short-lived token, minted from your grant. Rotate it any time — connected clients re-auth automatically.</p>
            <div className="field-row">
              <div className="field-mono">{ENDPOINT}</div>
              <button className="icon-btn" title="Copy endpoint" onClick={() => copyText(ENDPOINT, onToast)}><MIcon name="copy" size={15} /></button>
            </div>
            <div className="field-row">
              <div className="field-mono">{masked}</div>
              <button className="icon-btn" title="Copy token" onClick={() => copyText(tok, onToast)}><MIcon name="copy" size={15} /></button>
              <button className="icon-btn" title="Regenerate token" onClick={() => { setTok(tokenStr((Math.random() * 1e6 | 0).toString(36))); onToast("New token minted · 15-min TTL", "AUDIT #" + (80100 + (Math.random() * 99 | 0))); }}><MIcon name="refresh" size={15} /></button>
            </div>
            <div className="scope-line">
              <span className="cap-chip"><MIcon name="shield" size={12} /> This token can <b>read</b> <span className="rw">14 REPOS</span></span>
              <span className="cap-chip"><b>propose</b> to <span className="rw">ALL</span></span>
              <span className="cap-chip">TTL <b>15 min</b></span>
            </div>
          </div>

          <div className="cn-card">
            <h3>Connected clients</h3>
            <p className="sub">Models attached to your endpoint right now. Revoke any one without touching the others.</p>
            {connected.map((c, i) => (
              <div className="person-row" key={i} style={{ borderTop: i ? "1px solid var(--hair)" : "0" }}>
                <span className="avatar" style={{ background: c.live ? "var(--citrate-green)" : "var(--stone-500)" }}><MIcon name="spark" size={16} /></span>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 13, color: "var(--ondark)", fontWeight: 500 }}>{c.name}</div>
                  <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{c.scope}</div>
                </div>
                <span className="mono" style={{ fontSize: 11, color: c.live ? "var(--citrate-green)" : "var(--ondark-3)" }}>{c.last}</span>
                <button className="act" style={{ padding: "5px 10px" }} onClick={() => onToast("Connection revoked · " + c.name, "AUDIT #" + (80200 + (Math.random() * 99 | 0)))}><MIcon name="x" size={13} />Revoke</button>
              </div>
            ))}
          </div>
        </div>

        <div className="cn-card" style={{ padding: 0 }}>
          <div style={{ padding: "18px 20px 0" }}>
            <h3>Client configuration</h3>
            <p className="sub">Drop this into your client’s MCP config. The endpoint and token above are already filled in.</p>
          </div>
          <div style={{ padding: "0 20px" }}>
            <div className="code-tabs">
              {clients.map((c) => <div key={c.id} className={"code-tab" + (client === c.id ? " on" : "")} onClick={() => setClient(c.id)}>{c.label}</div>)}
            </div>
          </div>
          <div style={{ padding: "0 20px 20px" }}>
            <CodeBlock text={configs[client]} onToast={onToast} />
          </div>
        </div>
      </div>
    </div>
  );
}

/* ---------------- Audit log ---------------- */
function auWhen(ms) {
  const d = (MEM.T1 - ms) / 1000;
  if (d < 2) return "now";
  if (d < 60) return Math.floor(d) + "s";
  if (d < 3600) return Math.floor(d / 60) + "m";
  if (d < 86400) return Math.floor(d / 3600) + "h";
  return new Date(ms).toISOString().slice(5, 10);
}
function evColor(kind) { const k = MEM.AUDIT_KINDS.find(k => k.id === kind); return k ? k.color : "#9fc0e8"; }

function AuditRow({ e, fresh }) {
  const col = e.color || evColor(e.kind);
  return (
    <div className={"au-row" + (fresh ? " fresh" : "")}>
      <span className="au-seq">#{e.seq.toLocaleString()}</span>
      <span className="au-ev" style={{ color: col, borderColor: col + "66", background: col + "14" }}>{e.kind}</span>
      <span className="au-actor"><span className="av" style={{ background: e.actorColor || "#9fc0e8" }}>{e.initials}</span><span className="an">{e.actor}</span></span>
      <span className="au-detail">{e.detail}</span>
      <span className="au-when">{auWhen(e.when)}</span>
    </div>
  );
}

function AuditPage() {
  const [actor, setActor] = React.useState("all");
  const [kind, setKind] = React.useState("all");
  const [repo, setRepo] = React.useState("all");
  const [q, setQ] = React.useState("");
  const [tail, setTail] = React.useState(true);
  const [live, setLive] = React.useState([]);
  const tailK = React.useRef(0);

  React.useEffect(() => {
    if (!tail) return;
    const iv = setInterval(() => {
      const k = MEM.AUDIT_KINDS[(Math.random() * 6) | 0];
      const a = MEM.auditActors[(Math.random() * MEM.auditActors.length) | 0];
      const rp = (a.scopes && a.scopes[0] && a.scopes[0] !== "*") ? a.scopes[0] : MEM.TENANTS[(Math.random() * MEM.TENANTS.length) | 0];
      tailK.current += 1;
      const seq = (live[0] ? live[0].seq : MEM.auditEvents[0].seq) + 1;
      const detail = ({ Read: "read repo:" + rp + "/memory", Recall: 'recall("' + rp + '")', Write: "merge_diff → +2 nodes", Assert: "signed node", Confirm: "confirmed proposal", Propose: "propose_edge → quarantined", Denied: "scope check failed · repo:" + rp, Shred: "crypto-shred · " + rp })[k.id] || "event";
      setLive((L) => [{ seq, kind: k.id, color: k.color, actor: a.name, initials: a.initials, actorColor: a.color, repo: rp, detail, when: MEM.T1 + tailK.current, _fresh: true }, ...L].slice(0, 40));
    }, 2200);
    return () => clearInterval(iv);
  }, [tail, live]);

  const all = React.useMemo(() => [...live, ...MEM.auditEvents], [live]);
  const filtered = all.filter((e) => {
    if (actor !== "all" && e.actor !== actor) return false;
    if (kind !== "all" && e.kind !== kind) return false;
    if (repo !== "all" && e.repo !== repo) return false;
    if (q && !(e.detail + " " + e.actor + " " + e.repo).toLowerCase().includes(q.toLowerCase())) return false;
    return true;
  }).slice(0, 140);

  const actorNames = [...new Set(MEM.auditActors.map((a) => a.name))];

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Tamper-evident · blake3 hash-chained</span>
          <h1>Audit log</h1>
          <p>The audit is the system. Every read, write and denial across the Org is chained and continuously verified — surfaced here as a feature, not hidden.</p>
        </div>

        <div className="au-status">
          <div className="au-badge"><MIcon name="check" size={17} /><div><div className="bt">Chain intact</div><div className="bs">2,481,902 records · verified 2s ago</div></div></div>
          <span style={{ fontSize: 12.5, color: "var(--ondark-2)", display: "flex", alignItems: "center", gap: 7 }}><MIcon name="info" size={14} /> Each record seals the hash of the one before it — any tampering breaks the chain, visibly.</span>
        </div>

        <div className="au-filters">
          <select className="au-select" value={actor} onChange={(e) => setActor(e.target.value)}>
            <option value="all">All actors</option>
            {actorNames.map((n) => <option key={n} value={n}>{n}</option>)}
          </select>
          <select className="au-select" value={kind} onChange={(e) => setKind(e.target.value)}>
            <option value="all">All events</option>
            {MEM.AUDIT_KINDS.map((k) => <option key={k.id} value={k.id}>{k.id}</option>)}
          </select>
          <select className="au-select" value={repo} onChange={(e) => setRepo(e.target.value)}>
            <option value="all">All repos</option>
            {MEM.TENANTS.map((t) => <option key={t} value={t}>{t}</option>)}
          </select>
          <div className="au-search"><MIcon name="search" size={14} /><input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter detail…" /></div>
          <span className={"au-tail" + (tail ? " on" : "")} onClick={() => setTail((v) => !v)}><span className="dot"></span>{tail ? "Live tail on" : "Live tail off"}</span>
        </div>

        <div className="au-table">
          <div className="au-hrow"><span>Seq</span><span>Event</span><span>Actor</span><span>Detail</span><span style={{ textAlign: "right" }}>When</span></div>
          {filtered.map((e) => <AuditRow key={e.seq} e={e} fresh={e._fresh} />)}
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { ConnectPage, AuditPage });
