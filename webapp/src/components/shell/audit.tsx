"use client";

/**
 * Audit-log viewer — surfaces the tamper-evident, blake3 hash-chained audit trail
 * the gateway's write + provisioning routes feed. Shows the live INTEGRITY verdict
 * (the chain re-verifies its linkage on every read — a broken chain shows loudly),
 * filters, and a live-tail poll. Org-scoped + fail-closed by the BFF.
 */
import { useEffect, useMemo, useState } from "react";
import { MIcon } from "@/components/icons";
import type { AuditRecord, AuditResult } from "@/lib/gateway/types";

const EV_COLOR: Record<string, string> = { Write: "#8ecc09", Read: "#5fa8e6", Denied: "#f0743a" };
const PALETTE = ["#ffbd10", "#8ecc09", "#5fa8e6", "#34c7b0", "#b58cff", "#f0743a"];
const hashIdx = (s: string, n: number) => { let h = 0; for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0; return h % n; };
const initials = (s: string) => (s.replace(/^did:citrate:|^dev:/, "")[0] ?? "?").toUpperCase();
function when(ts: number) {
  const d = Math.max(0, Date.now() - ts);
  if (d < 60_000) return "just now";
  if (d < 3_600_000) return Math.round(d / 60_000) + "m ago";
  if (d < 86_400_000) return Math.round(d / 3_600_000) + "h ago";
  return Math.round(d / 86_400_000) + "d ago";
}

export function AuditViewer({ org }: { org: string }) {
  const [data, setData] = useState<AuditResult | null>(null);
  const [down, setDown] = useState(false);
  const [actor, setActor] = useState("all");
  const [event, setEvent] = useState("all");
  const [q, setQ] = useState("");
  const [tail, setTail] = useState(true);

  useEffect(() => {
    let alive = true;
    const run = async () => {
      try {
        const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/audit?limit=500`, { cache: "no-store" });
        if (!alive) return;
        if (res.ok) { setData((await res.json()) as AuditResult); setDown(false); }
        else setDown(true);
      } catch {
        if (alive) setDown(true);
      }
    };
    void run();
    const iv = tail ? setInterval(run, 4000) : null;
    return () => { alive = false; if (iv) clearInterval(iv); };
  }, [org, tail]);

  const records = useMemo(() => data?.records ?? [], [data]);
  const actorNames = useMemo(() => [...new Set(records.map((r) => r.actor))], [records]);
  const filtered = records.filter((r) => {
    if (actor !== "all" && r.actor !== actor) return false;
    if (event !== "all" && r.event !== event) return false;
    if (q && !`${r.detail} ${r.actor} ${r.resource_id}`.toLowerCase().includes(q.toLowerCase())) return false;
    return true;
  });

  const Row = ({ r }: { r: AuditRecord }) => {
    const col = EV_COLOR[r.event] ?? "#9fc0e8";
    const av = PALETTE[hashIdx(r.actor, PALETTE.length)];
    return (
      <div className="au-row">
        <span className="au-seq">#{r.seq.toLocaleString()}</span>
        <span className="au-ev" style={{ color: col, borderColor: col + "66", background: col + "14" }}>{r.event}</span>
        <span className="au-actor"><span className="av" style={{ background: av }}>{initials(r.actor)}</span><span className="an">{r.actor}</span></span>
        <span className="au-detail">{r.detail || r.resource_id}</span>
        <span className="au-when">{when(r.ts)}</span>
      </div>
    );
  };

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Tamper-evident · blake3 hash-chained</span>
          <h1>Audit log</h1>
          <p>The audit is the system. Every write and denial across the Org is chained and continuously verified — surfaced here as a feature, not hidden.</p>
        </div>

        {down ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>The audit log loads from the gateway (unavailable — connect MEM_GATEWAY_ORIGIN).</div>
        ) : !data ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>Loading the audit chain…</div>
        ) : (
          <>
            <div className="au-status">
              <div className="au-badge" style={data.intact ? undefined : { borderColor: "rgba(210,60,40,.5)" }}>
                <MIcon name={data.intact ? "check" : "x"} size={17} />
                <div>
                  <div className="bt" style={data.intact ? undefined : { color: "#f0907f" }}>{data.intact ? "Chain intact" : "Chain BROKEN — tampering detected"}</div>
                  <div className="bs">{data.length.toLocaleString()} records · verified through #{data.verified_through.toLocaleString()}</div>
                </div>
              </div>
              <span style={{ fontSize: 12.5, color: "var(--ondark-2)", display: "flex", alignItems: "center", gap: 7 }}>
                <MIcon name="info" size={14} /> Each record seals the hash of the one before it — any tampering breaks the chain, visibly.
              </span>
            </div>

            <div className="au-filters">
              <select className="au-select" value={actor} onChange={(e) => setActor(e.target.value)}>
                <option value="all">All actors</option>
                {actorNames.map((n) => <option key={n} value={n}>{n}</option>)}
              </select>
              <select className="au-select" value={event} onChange={(e) => setEvent(e.target.value)}>
                <option value="all">All events</option>
                {["Write", "Read", "Denied"].map((k) => <option key={k} value={k}>{k}</option>)}
              </select>
              <div className="au-search"><MIcon name="search" size={14} /><input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter detail…" /></div>
              <span className={"au-tail" + (tail ? " on" : "")} onClick={() => setTail((v) => !v)}><span className="dot" />{tail ? "Live tail on" : "Live tail off"}</span>
            </div>

            <div className="au-table">
              <div className="au-hrow"><span>Seq</span><span>Event</span><span>Actor</span><span>Detail</span><span style={{ textAlign: "right" }}>When</span></div>
              {filtered.length ? filtered.map((r) => <Row key={r.seq} r={r} />) : (
                <div style={{ padding: 16, color: "var(--ondark-3)", fontSize: 13 }}>No audit records match.</div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
