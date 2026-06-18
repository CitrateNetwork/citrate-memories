/* Memrizz — Settings + Profile pages */

function Toggle({ on, onClick }) { return <span className={"sw-toggle" + (on ? " on" : "")} onClick={onClick}></span>; }

function SetRow({ t, d, children }) {
  return (
    <div className="set-row">
      <div className="rl"><div className="t">{t}</div>{d ? <div className="d">{d}</div> : null}</div>
      {children}
    </div>
  );
}

function SegPick({ value, options, onChange }) {
  return (
    <div className="seg-pick">
      {options.map((o) => <button key={o.v || o} className={value === (o.v || o) ? "on" : ""} onClick={() => onChange(o.v || o)}>{o.l || o}</button>)}
    </div>
  );
}

/* ---- crypto-shred (forget) high-friction flow ---- */
function ForgetFlow({ onToast }) {
  const [tenant, setTenant] = React.useState(MEM.TENANTS[0]);
  const [typed, setTyped] = React.useState("");
  const [done, setDone] = React.useState(false);
  const match = typed.trim() === tenant;
  const count = MEM.perTenant[tenant].length;
  return (
    <div className="danger-zone">
      <h4>Forget a repository — crypto-shred</h4>
      <p>This permanently forgets all of a repo-tenant’s memory by destroying its key. It is <b style={{ color: "#f0907f" }}>cryptographically irreversible</b>. Org Owner only, and double-audited.</p>
      <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap", marginBottom: 12 }}>
        <select className="set-input" value={tenant} onChange={(e) => { setTenant(e.target.value); setTyped(""); setDone(false); }}>
          {MEM.TENANTS.map((t) => <option key={t} value={t}>{t}</option>)}
        </select>
        <span style={{ fontSize: 12.5, color: "var(--ondark-2)" }}>will forget <b className="tnum" style={{ color: "var(--ondark)" }}>{count}</b> memories</span>
      </div>
      {done ? (
        <div className="rv-resolved-tag" style={{ color: "#f0907f" }}><MIcon name="check" size={15}/> {tenant} forgotten · double-audited</div>
      ) : (
        <React.Fragment>
          <div style={{ fontSize: 12, color: "var(--ondark-2)", marginBottom: 7 }}>Type <b style={{ color: "var(--ondark)", fontFamily: "var(--font-mono)" }}>{tenant}</b> to confirm</div>
          <div style={{ display: "flex", gap: 10 }}>
            <input className="set-input" value={typed} onChange={(e) => setTyped(e.target.value)} placeholder="repo name" />
            <button className="btn-danger" disabled={!match} style={{ opacity: match ? 1 : 0.4, cursor: match ? "pointer" : "not-allowed" }}
              onClick={() => { if (match) { setDone(true); onToast("Crypto-shred executed · " + tenant, "AUDIT #" + (90400 + (Math.random() * 99 | 0))); } }}>
              Permanently forget {tenant}
            </button>
          </div>
        </React.Fragment>
      )}
    </div>
  );
}

function DelegationTree({ onToast }) {
  const byParent = {};
  MEM.PEOPLE.forEach((p) => { (byParent[p.parent] = byParent[p.parent] || []).push(p); });
  const [revoked, setRevoked] = React.useState({});
  function descendants(id) { let out = []; (byParent[id] || []).forEach((c) => { out.push(c.id); out = out.concat(descendants(c.id)); }); return out; }
  function revoke(p) {
    const d = descendants(p.id);
    setRevoked((r) => { const n = { ...r, [p.id]: true }; d.forEach((x) => (n[x] = true)); return n; });
    onToast(`Revoked ${p.name}${d.length ? " + " + d.length + " under them" : ""}`, "AUDIT #" + (70100 + (Math.random() * 99 | 0)));
  }
  const render = (p, depth) => (
    <React.Fragment key={p.id}>
      <div className="person-row" style={{ opacity: revoked[p.id] ? 0.4 : 1, paddingLeft: depth * 22 }}>
        {depth ? <span className="dt-line">└</span> : null}
        <span className="avatar" style={{ background: p.color }}>{p.initials}</span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 13.5, color: "var(--ondark)", fontWeight: 500 }}>{p.name}</div>
          <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{p.scopes.map((s) => s === "*" ? "all repos" : s).join(" · ")}</div>
        </div>
        <span className="role-badge">{p.role}</span>
        {p.me || revoked[p.id] ? null : <button className="act" onClick={() => revoke(p)} style={{ padding: "5px 10px" }}><MIcon name="x" size={13}/>Revoke</button>}
      </div>
      {(byParent[p.id] || []).map((c) => render(c, depth + 1))}
    </React.Fragment>
  );
  return (
    <div className="deltree">
      {(byParent[null] || []).map((p) => render(p, 0))}
      <div style={{ fontSize: 11.5, color: "var(--ondark-3)", marginTop: 12, display: "flex", gap: 7, alignItems: "center" }}>
        <MIcon name="info" size={13}/> Revoking a parent cascades to everyone they granted. A child can never hold more scope than its parent.
      </div>
    </div>
  );
}

function SettingsPage({ onToast, settings, setSetting }) {
  const [sec, setSec] = React.useState("general");
  const SECS = [
    { id: "general", label: "General", icon: "sliders" },
    { id: "memory", label: "Memory & index", icon: "layers" },
    { id: "models", label: "Models & AI", icon: "spark" },
    { id: "people", label: "People & roles", icon: "target" },
    { id: "delegation", label: "Delegation", icon: "branch" },
    { id: "security", label: "Security & audit", icon: "shield" },
    { id: "danger", label: "Danger zone", icon: "x" },
  ];
  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Citrate Federation · org settings</span>
          <h1>Settings</h1>
          <p>Everything here is scoped to this Org. You’re signed in as <b style={{ color: "var(--ondark)" }}>{MEM.ME.name}</b> — Org Owner.</p>
        </div>
        <div className="set-grid">
          <div className="set-nav">
            {SECS.map((s) => <button key={s.id} className={sec === s.id ? "on" : ""} onClick={() => setSec(s.id)}><MIcon name={s.icon} size={15}/>{s.label}</button>)}
          </div>
          <div>
            {sec === "general" && (
              <div className="set-sec">
                <h3>General</h3>
                <p className="sub">Workspace identity and defaults.</p>
                <SetRow t="Organization name"><input className="set-input" defaultValue="Citrate Federation" /></SetRow>
                <SetRow t="Default landing view" d="Where everyone starts each session."><SegPick value={settings.landing} options={[{ v: "constellation", l: "Constellation" }, { v: "ask", l: "Ask" }, { v: "review", l: "Review" }]} onChange={(v) => setSetting("landing", v)} /></SetRow>
                <SetRow t="Answer detail by default" d="Plain language for everyone, or technical with tool calls and hashes."><SegPick value={settings.mode} options={[{ v: "plain", l: "Plain" }, { v: "tech", l: "Technical" }]} onChange={(v) => setSetting("mode", v)} /></SetRow>
                <SetRow t="Reduce motion" d="Calm the particles and morphs across the constellation."><Toggle on={settings.reduce} onClick={() => setSetting("reduce", !settings.reduce)} /></SetRow>
              </div>
            )}
            {sec === "memory" && (
              <div className="set-sec">
                <h3>Memory &amp; index</h3>
                <p className="sub">How the org’s memory is kept fresh and durable.</p>
                <SetRow t="Index freshness" d="Last semantic re-embed across all repos."><span style={{ display: "flex", alignItems: "center", gap: 9 }}><span className="tag status-active" style={{ padding: "2px 9px" }}>synced · 18m ago</span><button className="act" onClick={() => onToast("Re-embed queued for 14 repos", "JOB #" + (3300 + (Math.random() * 99 | 0)))}><MIcon name="history" size={13}/>Re-embed now</button></span></SetRow>
                <SetRow t="Rolling checkpoints" d="Recovery point objective for crash recovery."><span style={{ fontSize: 12.5, color: "var(--ondark)" }}>recovery point · <b className="tnum">4 min ago</b></span></SetRow>
                <SetRow t="Show proposed (advisory) memory by default" d="Quarantined AI proposals appear, clearly marked, until confirmed."><Toggle on={settings.showQ} onClick={() => setSetting("showQ", !settings.showQ)} /></SetRow>
                <SetRow t="Retention" d="How long superseded memory is kept before archival."><SegPick value={settings.retention} options={[{ v: "90", l: "90 days" }, { v: "1y", l: "1 year" }, { v: "forever", l: "Forever" }]} onChange={(v) => setSetting("retention", v)} /></SetRow>
              </div>
            )}
            {sec === "models" && (
              <div className="set-sec">
                <h3>Models &amp; AI</h3>
                <p className="sub">Which model answers, and how it grounds itself.</p>
                {MEM.MODELS.map((m) => (
                  <div className="set-row" key={m.id} onClick={() => setSetting("model", m.id)} style={{ cursor: "pointer" }}>
                    <span className="pk-box" style={{ width: 18, height: 18, borderRadius: "50%", border: "1px solid var(--hair-2)", background: settings.model === m.id ? "var(--citrate-green)" : "transparent", flex: "none" }}>{settings.model === m.id ? <MIcon name="check" size={12}/> : null}</span>
                    <div className="rl"><div className="t">{m.name} <span style={{ color: "var(--ondark-3)", fontWeight: 400, fontSize: 12 }}>· {m.provider}</span></div><div className="d">{m.note}</div></div>
                    <span className="role-badge">{m.kind}</span>
                    <span className="mono" style={{ fontSize: 11, color: "var(--ondark-3)" }}>{m.cost} · {m.latency}</span>
                  </div>
                ))}
                <SetRow t="Allow bring-your-own-model (BYOM)" d="Members may connect their own model over a scoped, audited MCP endpoint."><Toggle on={settings.byom} onClick={() => setSetting("byom", !settings.byom)} /></SetRow>
                <SetRow t="Always show the model’s work" d="Expose tool calls and cited memories by default."><Toggle on={settings.work} onClick={() => setSetting("work", !settings.work)} /></SetRow>
              </div>
            )}
            {sec === "people" && (
              <div className="set-sec">
                <h3>People &amp; roles</h3>
                <p className="sub">{MEM.PEOPLE.length} members in this Org. <button className="act" style={{ padding: "5px 10px", marginLeft: 6 }} onClick={() => onToast("Invitation drafted", "")}><MIcon name="plus" size={13}/>Invite</button></p>
                {MEM.PEOPLE.map((p) => (
                  <div className="person-row" key={p.id}>
                    <span className="avatar" style={{ background: p.color }}>{p.initials}</span>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontSize: 13.5, color: "var(--ondark)", fontWeight: 500 }}>{p.name}{p.me ? <span style={{ color: "var(--ondark-3)", fontWeight: 400 }}> · you</span> : null}</div>
                      <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{p.title} · {p.email}</div>
                    </div>
                    <span className="role-badge">{p.role}</span>
                    <span className="mono" style={{ fontSize: 11, color: "var(--ondark-3)" }}>{p.policy}</span>
                  </div>
                ))}
              </div>
            )}
            {sec === "delegation" && (
              <div className="set-sec">
                <h3>Delegation tree</h3>
                <p className="sub">Who granted whom, and what they can touch. Attenuation and revocation cascade are enforced server-side.</p>
                <DelegationTree onToast={onToast} />
              </div>
            )}
            {sec === "security" && (
              <div className="set-sec">
                <h3>Security &amp; audit</h3>
                <p className="sub">The audit is the system. Every read, write and denial is hash-chained.</p>
                <SetRow t="Audit chain" d="blake3 hash-chained event log, continuously verified."><span className="tag status-active" style={{ padding: "2px 9px" }}><MIcon name="check" size={12}/>&nbsp;chain intact · 2,481,902 records</span></SetRow>
                <SetRow t="Fail closed on missing config" d="Reject all tokens if OIDC issuer/audience is unset. Recommended."><Toggle on={settings.failClosed} onClick={() => setSetting("failClosed", !settings.failClosed)} /></SetRow>
                <SetRow t="Tokens out of localStorage" d="Keep session tokens in memory only (CSP-hardened)."><Toggle on={settings.tokenSafe} onClick={() => setSetting("tokenSafe", !settings.tokenSafe)} /></SetRow>
                <SetRow t="BYOM token lifetime" d="Short-lived tokens minted from a member’s capability grant."><SegPick value={settings.tokenTtl} options={[{ v: "15m", l: "15 min" }, { v: "1h", l: "1 hour" }, { v: "8h", l: "8 hours" }]} onChange={(v) => setSetting("tokenTtl", v)} /></SetRow>
                <SetRow t="Export audit log" d="Download the verified event stream for compliance."><button className="act" onClick={() => onToast("Audit export prepared", "")}><MIcon name="download" size={13}/>Export</button></SetRow>
              </div>
            )}
            {sec === "danger" && (
              <div className="set-sec">
                <h3>Danger zone</h3>
                <p className="sub">Irreversible, Org-Owner-only actions. Designed to be impossible to do by accident.</p>
                <ForgetFlow onToast={onToast} />
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

/* ---------------- Profile ---------------- */
function ProfilePage({ onToast, onCite }) {
  const me = MEM.ME;
  const grantedBy = me.parent ? MEM.personById(me.parent) : null;
  const myActivity = [
    { icon: "check", t: "Confirmed a proposed supersession", s: "citrate-memories · Adr supersedes Adr", w: "2h ago" },
    { icon: "spark", t: "Asked: “Why per-Org isolation?”", s: "4 memories cited · Claude Sonnet 4.6", w: "3h ago" },
    { icon: "shield", t: "Resolved a contradiction", s: "kept the ADR, superseded the claim", w: "yesterday" },
    { icon: "download", t: "Exported the audit log", s: "2,481,902 records", w: "2d ago" },
  ];
  const connected = [
    { name: "Claude Desktop", scope: "read citrate-memories, citrate-docs", last: "active now", live: true },
    { name: "Cursor · local Llama", scope: "read + propose citrate-docs", last: "1h ago", live: false },
  ];
  return (
    <div className="route">
      <div className="route-inner">
        <div className="prof-hero">
          <span className="avatar lg" style={{ background: me.color }}>{me.initials}</span>
          <div style={{ flex: 1 }}>
            <div className="pn">{me.name}</div>
            <div className="pt">{me.title} · {me.email}</div>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 7, alignItems: "flex-end" }}>
            <span className="role-badge" style={{ color: "var(--citrate-green)", borderColor: "rgba(142,204,9,.4)" }}>{me.role}</span>
            <span className="mono" style={{ fontSize: 11, color: "var(--ondark-3)" }}>policy · {me.policy}</span>
          </div>
        </div>

        <div className="stat-grid">
          <div className="stat-card"><div className="sv tnum">{MEM.stats.nodes.toLocaleString()}</div><div className="sl">Memories readable</div></div>
          <div className="stat-card"><div className="sv tnum">128</div><div className="sl">Questions asked</div></div>
          <div className="stat-card"><div className="sv tnum">34</div><div className="sl">Proposals confirmed</div></div>
          <div className="stat-card"><div className="sv tnum">14</div><div className="sl">Repos in scope</div></div>
        </div>

        <div className="set-sec">
          <h3>What I can reach</h3>
          <p className="sub">Your capability grant — the repos you can read and write, within this Org.</p>
          <div>
            {MEM.TENANTS.map((t) => (
              <span className="scope-chip" key={t}>{t}<span className="rw">READ · WRITE</span></span>
            ))}
          </div>
          {grantedBy ? <div style={{ fontSize: 12, color: "var(--ondark-2)", marginTop: 12, display: "flex", gap: 7, alignItems: "center" }}><MIcon name="branch" size={13}/> Granted by {grantedBy.name} · root grant, no expiry</div> : <div style={{ fontSize: 12, color: "var(--ondark-2)", marginTop: 12, display: "flex", gap: 7, alignItems: "center" }}><MIcon name="shield" size={13}/> Root grant · you are the Org Owner</div>}
        </div>

        <div className="set-sec">
          <h3>My connected models</h3>
          <p className="sub">Bring your own model over a scoped, audited MCP endpoint. <button className="act" style={{ padding: "5px 10px", marginLeft: 6 }} onClick={() => onToast("New MCP token minted · 15-min TTL", "")}><MIcon name="plus" size={13}/>Connect a model</button></p>
          {connected.map((c, i) => (
            <div className="person-row" key={i}>
              <span className="avatar" style={{ background: c.live ? "var(--citrate-green)" : "var(--stone-500)" }}><MIcon name="spark" size={16}/></span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13.5, color: "var(--ondark)", fontWeight: 500 }}>{c.name}</div>
                <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{c.scope}</div>
              </div>
              <span className="mono" style={{ fontSize: 11, color: c.live ? "var(--citrate-green)" : "var(--ondark-3)" }}>{c.last}</span>
              <button className="act" style={{ padding: "5px 10px" }} onClick={() => onToast("Connection revoked · " + c.name, "")}><MIcon name="x" size={13}/>Revoke</button>
            </div>
          ))}
        </div>

        <div className="set-sec">
          <h3>Recent activity</h3>
          <p className="sub">Your own slice of the audit chain.</p>
          {myActivity.map((a, i) => (
            <div className="person-row" key={i}>
              <span className="ni-ic" style={{ width: 32, height: 32, borderRadius: 8, display: "grid", placeItems: "center", border: "1px solid var(--hair)", color: "var(--citrate-green)" }}><MIcon name={a.icon} size={15}/></span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13, color: "var(--ondark)" }}>{a.t}</div>
                <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{a.s}</div>
              </div>
              <span className="mono" style={{ fontSize: 11, color: "var(--ondark-3)" }}>{a.w}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { SettingsPage, ProfilePage });
