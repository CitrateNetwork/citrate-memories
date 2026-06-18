"use client";

/**
 * BYOM — Connect a model (the /connect surface). Mints a real short-lived, scoped
 * token (`/api/orgs/:org/connect/token`) for an MCP client and renders the
 * endpoint + copy-paste config for Claude Desktop / Code / Cursor / generic. The
 * token reflects the caller's readable tenants and expires in 15 min; the
 * consuming MCP-over-HTTP endpoint (G-7) is the next backend milestone, surfaced
 * honestly in the Connected-clients panel.
 */
import { useEffect, useState } from "react";
import { MIcon } from "@/components/icons";

interface Conn { endpoint: string; token: string; ttlMinutes: number; tenants: string[] }
type ClientId = "desktop" | "code" | "cursor" | "generic";

const CLIENTS: { id: ClientId; label: string }[] = [
  { id: "desktop", label: "Claude Desktop" },
  { id: "code", label: "Claude Code" },
  { id: "cursor", label: "Cursor" },
  { id: "generic", label: "Generic MCP" },
];

function configFor(client: ClientId, endpoint: string, token: string): string {
  switch (client) {
    case "desktop":
      return `{
  "mcpServers": {
    "citrate-memory": {
      "url": "${endpoint}",
      "headers": { "Authorization": "Bearer ${token}" }
    }
  }
}`;
    case "code":
      return `// .mcp.json  (project root)
{
  "mcpServers": {
    "citrate-memory": {
      "type": "http",
      "url": "${endpoint}",
      "headers": { "Authorization": "Bearer ${token}" }
    }
  }
}`;
    case "cursor":
      return `// ~/.cursor/mcp.json
{
  "mcpServers": {
    "citrate-memory": {
      "url": "${endpoint}",
      "headers": { "Authorization": "Bearer ${token}" }
    }
  }
}`;
    case "generic":
      return `# Streamable-HTTP MCP transport
curl -N ${endpoint} \\
  -H "Authorization: Bearer ${token}" \\
  -H "Accept: text/event-stream"

# tools exposed: recall · search · neighbors
#                verify · as_of · analogy · critique`;
  }
}

export function ConnectPage({ org, onToast }: { org: string; onToast: (m: string) => void }) {
  const [conn, setConn] = useState<Conn | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "unconfigured" | "down">("loading");
  const [client, setClient] = useState<ClientId>("desktop");

  async function applyToken(res: Response) {
    if (res.ok) { setConn((await res.json()) as Conn); setState("ready"); }
    else if (res.status === 503) setState("unconfigured");
    else setState("down");
  }
  // initial mint — inlined so setState happens only inside the async callback
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/connect/token`, { method: "POST" });
        if (alive) await applyToken(res);
      } catch {
        if (alive) setState("down");
      }
    })();
    return () => { alive = false; };
  }, [org]);
  // regenerate — from the button (an event handler, not an effect)
  async function regenerate() {
    try {
      const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/connect/token`, { method: "POST" });
      await applyToken(res);
      if (res.ok) onToast("New token minted · 15-min TTL · audited.");
    } catch { setState("down"); }
  }

  const copy = (t: string, what: string) => {
    try { void navigator.clipboard?.writeText(t); } catch { /* ignore */ }
    onToast(`${what} copied to clipboard.`);
  };
  const masked = conn ? conn.token.slice(0, 16) + "•".repeat(10) + conn.token.slice(-6) : "";

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Bring your own model · MCP over Streamable-HTTP</span>
          <h1>Connect a model</h1>
          <p>Point any MCP client — Claude, Cursor, a local model — at your personal, scoped endpoint. It can only touch what your capability grant allows, and every call is written to the audit chain under your name. The model never touches the raw store.</p>
        </div>

        {state === "loading" ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>Minting your connection token…</div>
        ) : state === "unconfigured" ? (
          <div className="cn-card"><h3>BYOM not configured</h3><p className="sub">Set <span className="mono">MEM_CONNECT_SECRET</span> on the server to mint connection tokens.</p></div>
        ) : state === "down" ? (
          <div className="cn-card"><h3>Unavailable</h3><p className="sub">Couldn&rsquo;t reach the gateway to mint a token.</p></div>
        ) : conn ? (
          <>
            <div className="cn-grid">
              <div className="cn-card">
                <h3>Your endpoint &amp; token</h3>
                <p className="sub">A short-lived token, minted from your grant. Rotate it any time — connected clients re-auth automatically.</p>
                <div className="field-row">
                  <div className="field-mono">{conn.endpoint}</div>
                  <button className="icon-btn" title="Copy endpoint" onClick={() => copy(conn.endpoint, "Endpoint")}><MIcon name="copy" size={15} /></button>
                </div>
                <div className="field-row">
                  <div className="field-mono">{masked}</div>
                  <button className="icon-btn" title="Copy token" onClick={() => copy(conn.token, "Token")}><MIcon name="copy" size={15} /></button>
                  <button className="icon-btn" title="Regenerate token" onClick={() => void regenerate()}><MIcon name="refresh" size={15} /></button>
                </div>
                <div className="scope-line">
                  <span className="cap-chip"><MIcon name="shield" size={12} /> This token can <b>read</b> <span className="rw">{conn.tenants.length} REPOS</span></span>
                  <span className="cap-chip"><b>propose</b> to <span className="rw">SCOPED</span></span>
                  <span className="cap-chip">TTL <b>{conn.ttlMinutes} min</b></span>
                </div>
              </div>

              <div className="cn-card">
                <h3>Connected clients</h3>
                <p className="sub">Models attached to your endpoint. Each call is audited under your name; revoke any one without touching the others.</p>
                <div style={{ color: "var(--ondark-3)", fontSize: 12.5, lineHeight: 1.6, padding: "10px 0" }}>
                  <MIcon name="info" size={14} /> The MCP-over-HTTP endpoint is live — it verifies this token, scopes every tool call to your grant, and writes each call to the audit chain. A live connected-clients registry (per-session tracking + per-client revoke) is a follow-up.
                </div>
              </div>
            </div>

            <div className="cn-card" style={{ padding: 0 }}>
              <div style={{ padding: "18px 20px 0" }}>
                <h3>Client configuration</h3>
                <p className="sub">Drop this into your client&rsquo;s MCP config. The endpoint and token above are already filled in.</p>
              </div>
              <div style={{ padding: "0 20px" }}>
                <div className="code-tabs">
                  {CLIENTS.map((cl) => (
                    <div key={cl.id} className={"code-tab" + (client === cl.id ? " on" : "")} onClick={() => setClient(cl.id)}>{cl.label}</div>
                  ))}
                </div>
              </div>
              <div style={{ padding: "0 20px 20px" }}>
                <div className="code-block">
                  <button className="icon-btn copy-fab" title="Copy" onClick={() => copy(configFor(client, conn.endpoint, conn.token), "Config")}><MIcon name="copy" size={15} /></button>
                  <pre>{configFor(client, conn.endpoint, conn.token)}</pre>
                </div>
              </div>
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
