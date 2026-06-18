"use client";

/**
 * Admin console (WP-7.9) — surfaces the G-4 member routes as a real People &
 * Delegation surface: the live roster (`GET members`), attenuation-aware invite
 * (`POST members`), and cascade-revoke (`DELETE members/:sub`, F-7). The backend
 * is authoritative on attenuation + role ceilings; the UI shows the gateway's
 * verdict (a 403 surfaces as a clear toast) rather than guessing. Degrades
 * gracefully when the gateway is unreachable.
 */
import { useCallback, useEffect, useState } from "react";
import { MIcon } from "@/components/icons";
import type { Member } from "@/lib/gateway/client";
import type { OpsSnapshot } from "@/lib/gateway/types";

function ago(ms: number | null): string {
  if (!ms) return "never";
  const d = Math.max(0, Date.now() - ms);
  if (d < 60_000) return "just now";
  if (d < 3_600_000) return Math.round(d / 60_000) + "m ago";
  if (d < 86_400_000) return Math.round(d / 3_600_000) + "h ago";
  return Math.round(d / 86_400_000) + "d ago";
}

const ROLE_LABEL: Record<string, string> = { OrgOwner: "Org Owner", OrgAdmin: "Org Admin", Member: "Member" };
const PALETTE = ["#ffbd10", "#8ecc09", "#5fa8e6", "#34c7b0", "#b58cff", "#f0743a"];

function hashIdx(s: string, n: number) {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % n;
}
function initialsOf(sub: string) {
  const base = sub.replace(/^did:citrate:|^dev:|^google-oauth2\|/, "");
  return (base[0] ?? "?").toUpperCase();
}
function scopeLabel(m: Member) {
  if (m.role === "OrgOwner") return "all repos";
  if (!m.scopes.length) return "no scopes";
  return m.scopes.map((s) => (s.resource_id === "*" ? "all repos" : s.resource_id.replace(/^repo:/, "").replace(/\/memory$/, "") + (s.can_write ? " (rw)" : " (r)"))).join(" · ");
}

export function AdminConsole({ org, onToast }: { org: string; onToast: (m: string) => void }) {
  const [sec, setSec] = useState<"people" | "delegation" | "neworg" | "ops">("people");
  const [newOrgName, setNewOrgName] = useState("");
  const [ops, setOps] = useState<OpsSnapshot | null>(null);
  const [opsDown, setOpsDown] = useState(false);
  const [members, setMembers] = useState<Member[] | null>(null);
  const [down, setDown] = useState(false);
  // invite form
  const [sub, setSub] = useState("");
  const [role, setRole] = useState("Member");
  const [repo, setRepo] = useState("");
  const [canWrite, setCanWrite] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/members`, { cache: "no-store" });
      if (res.ok) { setMembers(((await res.json()) as { members: Member[] }).members); setDown(false); }
      else setDown(true);
    } catch { setDown(true); }
  }, [org]);
  useEffect(() => { void load(); }, [load]);

  async function invite() {
    const s = sub.trim();
    if (!s) return;
    const scopes = repo.trim()
      ? [{ resource_id: `repo:${repo.trim()}/memory`, can_read: true, can_write: canWrite }]
      : [];
    try {
      const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/members`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ sub: s, role: role.toLowerCase(), scopes }),
      });
      if (res.ok) { onToast(`Onboarded ${s} as ${ROLE_LABEL[role] ?? role}.`); setSub(""); setRepo(""); setCanWrite(false); void load(); }
      else if (res.status === 403) onToast("Denied — you can't grant more than you hold (attenuation).");
      else onToast("Onboarding failed.");
    } catch { onToast("Couldn't reach the gateway."); }
  }

  async function createOrg() {
    const name = newOrgName.trim();
    if (!name) return;
    try {
      const res = await fetch("/api/orgs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) });
      if (res.ok) { const o = (await res.json()) as { id: string }; onToast(`Org “${o.id}” created — its store is initialized and you're the Owner.`); setNewOrgName(""); }
      else if (res.status === 409) onToast("An Org with that name already exists.");
      else if (res.status === 503) onToast("Store backend unavailable on the server.");
      else onToast("Couldn't create the Org.");
    } catch { onToast("Couldn't reach the gateway."); }
  }

  // ops snapshot — fetched when the Ops tab is open (inlined to keep setState in the async callback)
  useEffect(() => {
    if (sec !== "ops") return;
    let alive = true;
    (async () => {
      try {
        const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/ops`, { cache: "no-store" });
        if (!alive) return;
        if (res.ok) { setOps((await res.json()) as OpsSnapshot); setOpsDown(false); }
        else setOpsDown(true);
      } catch {
        if (alive) setOpsDown(true);
      }
    })();
    return () => { alive = false; };
  }, [org, sec]);

  async function createCheckpoint() {
    try {
      const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/ops/checkpoint`, { method: "POST" });
      if (res.ok) {
        const r = (await res.json()) as { count: number };
        onToast(`Checkpoint created · ${r.count} recovery points retained · audited.`);
        const refresh = await fetch(`/api/orgs/${encodeURIComponent(org)}/ops`, { cache: "no-store" });
        if (refresh.ok) setOps((await refresh.json()) as OpsSnapshot);
      } else if (res.status === 403) onToast("Only an admin can create checkpoints.");
      else onToast("Checkpoint failed (store backend may be in-memory).");
    } catch { onToast("Couldn't reach the gateway."); }
  }

  async function revoke(target: string) {
    setConfirming(null);
    try {
      const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/members/${encodeURIComponent(target)}`, { method: "DELETE" });
      if (res.ok) {
        const { revoked } = (await res.json()) as { revoked: string[] };
        onToast(`Revoked ${target}${revoked.length > 1 ? ` + ${revoked.length - 1} under them` : ""} · audited.`);
        void load();
      } else onToast(res.status === 403 ? "Only an admin can revoke." : "Revoke failed.");
    } catch { onToast("Couldn't reach the gateway."); }
  }

  // delegation tree
  const byParent = new Map<string | null, Member[]>();
  for (const m of members ?? []) {
    const key = m.parent ?? null;
    byParent.set(key, [...(byParent.get(key) ?? []), m]);
  }
  const countDescendants = (s: string): number => {
    const kids = byParent.get(s) ?? [];
    return kids.reduce((acc, k) => acc + 1 + countDescendants(k.sub), 0);
  };

  function Row({ m, depth }: { m: Member; depth: number }) {
    const color = PALETTE[hashIdx(m.sub, PALETTE.length)];
    const desc = countDescendants(m.sub);
    return (
      <>
        <div className="person-row" style={{ paddingLeft: depth * 22 }}>
          {depth ? <span className="dt-line">└</span> : null}
          <span className="avatar" style={{ background: color }}>{initialsOf(m.sub)}</span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 13.5, color: "var(--ondark)", fontWeight: 500 }}>{m.sub}</div>
            <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{scopeLabel(m)}</div>
          </div>
          <span className="role-badge">{ROLE_LABEL[m.role] ?? m.role}</span>
          {confirming === m.sub ? (
            <span style={{ display: "flex", gap: 6 }}>
              <button className="act primary" style={{ padding: "5px 10px" }} onClick={() => revoke(m.sub)}>
                <MIcon name="x" size={13} />Confirm{desc ? ` (+${desc})` : ""}
              </button>
              <button className="act" style={{ padding: "5px 10px" }} onClick={() => setConfirming(null)}>Cancel</button>
            </span>
          ) : (
            <button className="act" style={{ padding: "5px 10px" }} onClick={() => setConfirming(m.sub)}>
              <MIcon name="x" size={13} />Revoke
            </button>
          )}
        </div>
        {(byParent.get(m.sub) ?? []).map((c) => <Row key={c.sub} m={c} depth={depth + 1} />)}
      </>
    );
  }

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Org admin · scoped to this Org</span>
          <h1>People &amp; access</h1>
          <p>Onboard members, scope their access, and manage the delegation tree. A child can never hold more than its parent; revoking a parent cascades to everyone beneath them — every change is audited.</p>
        </div>

        <div className="rv-tabs">
          {(["people", "delegation", "neworg", "ops"] as const).map((id) => (
            <div key={id} className={"rv-tab" + (sec === id ? " on" : "")} onClick={() => setSec(id)}>
              {id === "people" ? "People & roles" : id === "delegation" ? "Delegation tree" : id === "neworg" ? "New Org" : "Durability"}
            </div>
          ))}
        </div>

        {sec === "ops" ? (
          opsDown ? (
            <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>Ops loads from the gateway (admin-only; unavailable — connect MEM_GATEWAY_ORIGIN).</div>
          ) : !ops ? (
            <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>Loading durability status…</div>
          ) : (
            <div className="cn-grid">
              <div className="cn-card">
                <h3>Recovery points</h3>
                <p className="sub">Rolling encrypted checkpoints. Each is a complete, independently-openable store.</p>
                <div className="field-row" style={{ alignItems: "center" }}>
                  <div className="field-mono">{ops.checkpoints.count} checkpoint{ops.checkpoints.count === 1 ? "" : "s"} · last {ago(ops.checkpoints.latest_ms)}</div>
                  <button className="act primary" style={{ padding: "6px 12px" }} onClick={() => void createCheckpoint()}><MIcon name="download" size={14} />Checkpoint now</button>
                </div>
                <div className="scope-line">
                  <span className="cap-chip"><MIcon name="layers" size={12} /> <b>{ops.store.nodes.toLocaleString()}</b> nodes</span>
                  <span className="cap-chip"><b>{ops.store.edges.toLocaleString()}</b> edges</span>
                  {ops.chain_anchor ? <span className="cap-chip"><MIcon name="shield" size={12} /> notarized · block <b>{ops.chain_anchor.block_number.toLocaleString()}</b></span> : null}
                </div>
              </div>
              <div className="cn-card">
                <h3>Tenant anchors</h3>
                <p className="sub">Each tenant&rsquo;s merkle root, and whether the live state still matches its last anchor.</p>
                {ops.tenants.length ? ops.tenants.map((t) => {
                  const status = t.anchor == null ? ["never anchored", "var(--ondark-3)"] : t.anchor_valid ? ["in sync", "var(--citrate-green)"] : ["drifted since anchor", "#f3d27a"];
                  return (
                    <div className="person-row" key={t.repo}>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: 13.5, color: "var(--ondark)", fontWeight: 500 }}>{t.repo}</div>
                        <div className="mono" style={{ fontSize: 11, color: "var(--ondark-3)" }}>{t.live_root ? `root ${t.live_root.slice(0, 12)}` : "—"}{t.anchor ? ` · anchored ${ago(t.anchor.anchored_at_ms)}` : ""}</div>
                      </div>
                      <span className="role-badge" style={{ color: status[1], borderColor: status[1] + "55" }}>{status[0]}</span>
                    </div>
                  );
                }) : <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>No tenants.</div>}
              </div>
            </div>
          )
        ) : null}

        {sec === "neworg" ? (
          <div className="cn-card" style={{ maxWidth: 540 }}>
            <h3>Create a new Org</h3>
            <p className="sub">A new Org gets its own isolated, encrypted memory store and keyring — its data is physically separate from every other Org. You become its founding Owner.</p>
            <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 8 }}>
              <input
                value={newOrgName}
                onChange={(e) => setNewOrgName(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") void createOrg(); }}
                placeholder="Org name (e.g. Acme Research)"
                style={{ flex: 1, background: "rgba(6,15,10,.55)", border: "1px solid var(--hair)", borderRadius: "var(--r-1)", color: "var(--ondark)", height: 36, padding: "0 12px", fontSize: 13.5 }}
              />
              <button className="act primary" style={{ padding: "8px 14px" }} onClick={() => void createOrg()}><MIcon name="plus" size={14} />Create Org</button>
            </div>
            {newOrgName.trim() ? (
              <div style={{ fontSize: 11.5, color: "var(--ondark-3)", marginTop: 8 }}>id: <span className="mono">{newOrgName.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "")}</span></div>
            ) : null}
          </div>
        ) : null}

        {(sec === "people" || sec === "delegation") && (down ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>The roster loads from the gateway (unavailable — connect MEM_GATEWAY_ORIGIN).</div>
        ) : members === null ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>Loading members…</div>
        ) : sec === "people" ? (
          <div className="set-sec">
            <div className="invite-row" style={{ display: "flex", flexWrap: "wrap", gap: 8, alignItems: "center", marginBottom: 16, padding: 12, border: "1px solid var(--hair)", borderRadius: "var(--r-2)" }}>
              <input value={sub} onChange={(e) => setSub(e.target.value)} placeholder="member sub (did:citrate:…)" style={{ flex: 1, minWidth: 180, background: "rgba(6,15,10,.55)", border: "1px solid var(--hair)", borderRadius: "var(--r-1)", color: "var(--ondark)", height: 34, padding: "0 10px", fontSize: 13 }} />
              <select value={role} onChange={(e) => setRole(e.target.value)} style={{ background: "rgba(6,15,10,.55)", border: "1px solid var(--hair)", borderRadius: "var(--r-1)", color: "var(--ondark)", height: 34, padding: "0 8px", fontSize: 13 }}>
                <option value="Member">Member</option>
                <option value="OrgAdmin">Org Admin</option>
                <option value="OrgOwner">Org Owner</option>
              </select>
              <input value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="repo (e.g. mem-gateway)" style={{ width: 170, background: "rgba(6,15,10,.55)", border: "1px solid var(--hair)", borderRadius: "var(--r-1)", color: "var(--ondark)", height: 34, padding: "0 10px", fontSize: 13 }} />
              <label style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 12.5, color: "var(--ondark-2)" }}>
                <input type="checkbox" checked={canWrite} onChange={(e) => setCanWrite(e.target.checked)} /> write
              </label>
              <button className="act primary" style={{ padding: "6px 12px" }} onClick={invite}><MIcon name="plus" size={13} />Onboard</button>
            </div>
            {members.map((m) => {
              const color = PALETTE[hashIdx(m.sub, PALETTE.length)];
              return (
                <div className="person-row" key={m.sub}>
                  <span className="avatar" style={{ background: color }}>{initialsOf(m.sub)}</span>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 13.5, color: "var(--ondark)", fontWeight: 500 }}>{m.sub}</div>
                    <div style={{ fontSize: 11.5, color: "var(--ondark-2)" }}>{scopeLabel(m)}{m.parent ? ` · invited by ${m.parent}` : " · root"}</div>
                  </div>
                  <span className="role-badge">{ROLE_LABEL[m.role] ?? m.role}</span>
                </div>
              );
            })}
            {members.length === 0 ? <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>No members yet.</div> : null}
          </div>
        ) : sec === "delegation" ? (
          <div className="deltree">
            {(byParent.get(null) ?? []).map((m) => <Row key={m.sub} m={m} depth={0} />)}
            {(byParent.get(null) ?? []).length === 0 ? <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>No delegation tree yet.</div> : null}
            <div style={{ fontSize: 11.5, color: "var(--ondark-3)", marginTop: 12, display: "flex", gap: 7, alignItems: "center" }}>
              <MIcon name="info" size={13} /> Revoking a parent cascades to everyone they granted. A child can never hold more scope than its parent.
            </div>
          </div>
        ) : null)}
      </div>
    </div>
  );
}
