"use client";

/**
 * HIC Review Center (WP-7.8). The queues are DERIVED from the same Org scene the
 * constellation uses (no new gateway route needed): proposals = quarantined edges,
 * contradictions = `contradicted` nodes, supersessions = Supersedes edges.
 * **Confirm** calls the gateway's write-scoped, audited `edges/confirm` route
 * (gap G-1) — the moment an AI proposal becomes load-bearing is a deliberate,
 * recorded human act. Reject/escalate are local dismissals (no destructive gateway
 * route). Degrades gracefully when the gateway is unreachable.
 */
import { useEffect, useState } from "react";
import { MIcon } from "@/components/icons";
import { laneColor } from "@/lib/data/taxonomy";
import type { Scene, SceneEdge, SceneNode } from "@/lib/gateway/types";

type Tab = "proposals" | "contradictions" | "supersessions";
type Resolution = "confirmed" | "rejected" | "escalated";

function NodeMini({ node, onCite }: { node: SceneNode | undefined; onCite: (id: string) => void }) {
  if (!node) return <div className="rv-chip-node" style={{ opacity: 0.5 }}>unknown node</div>;
  const c = laneColor(node.lane);
  return (
    <div className="rv-chip-node" onClick={() => onCite(node.id)} title={node.title}>
      <span className="nd" style={{ background: c, color: c }} />
      <div style={{ minWidth: 0 }}>
        <div className="nt">{node.title}</div>
        <div className="nk">{node.kind} · {node.repo}</div>
      </div>
    </div>
  );
}

function ResolvedTag({ state }: { state: Resolution }) {
  const map: Record<Resolution, [Parameters<typeof MIcon>[0]["name"], string, string]> = {
    confirmed: ["check", "Confirmed · written to audit", "var(--citrate-green)"],
    rejected: ["x", "Rejected", "#f0907f"],
    escalated: ["info", "Escalated", "var(--citrate-yellow)"],
  };
  const [ic, label, col] = map[state];
  return <span className="rv-resolved-tag" style={{ color: col }}><MIcon name={ic} size={15} />{label}</span>;
}

export function ReviewCenter({
  org,
  onCite,
  onToast,
}: {
  org: string;
  onCite: (id: string) => void;
  onToast: (msg: string) => void;
}) {
  const [tab, setTab] = useState<Tab>("proposals");
  const [scene, setScene] = useState<Scene | null>(null);
  const [down, setDown] = useState(false);
  const [resolved, setResolved] = useState<Record<string, Resolution>>({});

  useEffect(() => {
    let cancelled = false;
    fetch(`/api/orgs/${encodeURIComponent(org)}/layout`, { cache: "no-store" })
      .then(async (r) => {
        if (cancelled) return;
        if (r.ok) setScene((await r.json()) as Scene);
        else setDown(true);
      })
      .catch(() => !cancelled && setDown(true));
    return () => { cancelled = true; };
  }, [org]);

  const byId = new Map<string, SceneNode>((scene?.nodes ?? []).map((n) => [n.id, n]));
  const edgeKey = (e: SceneEdge) => `${e.from}|${e.to}|${e.kind}`;
  const proposals = (scene?.edges ?? []).filter((e) => e.quarantined);
  const supersessions = (scene?.edges ?? []).filter((e) => !e.quarantined && e.kind === "Supersedes");
  const contradictions = (scene?.nodes ?? []).filter((n) => n.contradicted);

  async function confirm(e: SceneEdge) {
    try {
      const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/edges/confirm`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ from: e.from, to: e.to, kind: e.kind }),
      });
      if (!res.ok) {
        onToast(res.status === 403 ? "You don't have write access to confirm this." : "Confirm failed.");
        return;
      }
      setResolved((r) => ({ ...r, [edgeKey(e)]: "confirmed" }));
      onToast("Confirmed → load-bearing · written to the audit chain.");
    } catch {
      onToast("Couldn't reach the memory service.");
    }
  }
  function localResolve(key: string, state: Resolution, verb: string) {
    setResolved((r) => ({ ...r, [key]: state }));
    onToast(verb);
  }

  const remaining = (keys: string[]) => keys.filter((k) => !resolved[k]).length;
  const tabs: { id: Tab; label: string; n: number }[] = [
    { id: "proposals", label: "Proposals", n: remaining(proposals.map(edgeKey)) },
    { id: "contradictions", label: "Contradictions", n: remaining(contradictions.map((n) => n.id)) },
    { id: "supersessions", label: "Supersessions", n: remaining(supersessions.map(edgeKey)) },
  ];

  return (
    <div className="route">
      <div className="route-inner">
        <div className="route-head">
          <span className="eyebrow">Human In Control (HIC) · org-scoped</span>
          <h1>Review center</h1>
          <p>
            Nothing an AI proposes becomes load-bearing until a person confirms it. Witness
            each one — every confirmation is written to the tamper-evident audit chain.
          </p>
        </div>

        {down ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>
            The review queues load from the gateway scene (unavailable — connect MEM_GATEWAY_ORIGIN).
          </div>
        ) : !scene ? (
          <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>Loading the review queues…</div>
        ) : (
          <>
            <div className="rv-tabs">
              {tabs.map((t) => (
                <div key={t.id} className={"rv-tab" + (tab === t.id ? " on" : "")} onClick={() => setTab(t.id)}>
                  {t.label}{t.n ? <span className="num">{t.n}</span> : <MIcon name="check" size={13} />}
                </div>
              ))}
            </div>

            <div className="rv-list">
              {tab === "proposals" && (proposals.length ? proposals.map((e) => {
                const key = edgeKey(e);
                return (
                  <div className={"rv-card" + (resolved[key] ? " resolved" : "")} key={key}>
                    <div className="rv-top">
                      <div className="rv-prevpair">
                        <NodeMini node={byId.get(e.from)} onCite={onCite} />
                        <div className="rv-rel"><span>{e.kind}</span><span className="arr">→</span></div>
                        <NodeMini node={byId.get(e.to)} onCite={onCite} />
                      </div>
                    </div>
                    <div className="rv-meta">
                      <span><span className="tag plane-asserted" style={{ padding: "1px 7px" }}>Quarantined</span></span>
                      <span>advisory until confirmed</span>
                    </div>
                    <div className="rv-actions">
                      {resolved[key] ? <ResolvedTag state={resolved[key]} /> : (
                        <>
                          <button className="act primary" onClick={() => confirm(e)}><MIcon name="check" size={14} />Confirm — make load-bearing</button>
                          <button className="act" onClick={() => localResolve(key, "rejected", "Proposal rejected.")}><MIcon name="x" size={14} />Reject</button>
                        </>
                      )}
                    </div>
                  </div>
                );
              }) : <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>No proposals awaiting review. ✓</div>)}

              {tab === "supersessions" && (supersessions.length ? supersessions.map((e) => {
                const key = edgeKey(e);
                return (
                  <div className={"rv-card" + (resolved[key] ? " resolved" : "")} key={key}>
                    <div className="rv-top">
                      <div className="rv-prevpair">
                        <NodeMini node={byId.get(e.from)} onCite={onCite} />
                        <div className="rv-rel"><span>superseded by</span><span className="arr">→</span></div>
                        <NodeMini node={byId.get(e.to)} onCite={onCite} />
                      </div>
                    </div>
                    <div className="rv-actions">
                      {resolved[key] ? <ResolvedTag state={resolved[key]} /> : (
                        <>
                          <button className="act primary" onClick={() => confirm(e)}><MIcon name="check" size={14} />Confirm supersession</button>
                          <button className="act" onClick={() => localResolve(key, "rejected", "Flagged as wrong.")}><MIcon name="x" size={14} />Flag as wrong</button>
                        </>
                      )}
                    </div>
                  </div>
                );
              }) : <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>No pending supersessions. ✓</div>)}

              {tab === "contradictions" && (contradictions.length ? contradictions.map((n) => (
                <div className={"rv-card" + (resolved[n.id] ? " resolved" : "")} key={n.id}>
                  <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 9 }}>
                    <span style={{ color: "#f0907f" }}><MIcon name="x" size={17} /></span>
                    <div style={{ fontSize: 14.5, fontWeight: 600, color: "var(--ondark)" }}>{n.title}</div>
                  </div>
                  <div className="rv-meta"><span>Belnap value <b>Both</b> · repo <b>{n.repo}</b></span></div>
                  <div className="rv-actions">
                    {resolved[n.id] ? <ResolvedTag state={resolved[n.id]} /> : (
                      <>
                        <button className="act primary" onClick={() => onCite(n.id)}><MIcon name="target" size={14} />View in constellation</button>
                        <button className="act" onClick={() => localResolve(n.id, "escalated", "Escalated to an admin.")}><MIcon name="info" size={14} />Escalate to an admin</button>
                      </>
                    )}
                  </div>
                </div>
              )) : <div style={{ color: "var(--ondark-3)", fontSize: 13 }}>No recorded contradictions. ✓</div>)}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
