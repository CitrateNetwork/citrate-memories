"use client";

/**
 * RightDock — Ask + Inspect, ported 1:1 from panels.jsx / app.jsx.
 *
 * WP-7.3 ships the dock CHROME: the tab bar, the Ask intro + composer + Plain/
 * Technical toggle + suggestions, and the Inspect empty state. The grounded-RAG
 * answer pipeline (citations, show-your-work, tools) and the full Inspector
 * (verify verdict, neighbours, blast-radius) are M1 (WP-7.5/7.6) and wire to the
 * gateway — so the composer here records the question and shows an honest
 * "answering wires in M1" placeholder rather than a fabricated answer (Rule 11).
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { useChat } from "@ai-sdk/react";
import { DefaultChatTransport, type UIMessage } from "ai";
import { MIcon } from "@/components/icons";
import { LANES, LANE_IDX, SUGGESTIONS, TRUST, TRUST_IDX, laneColor, stats } from "@/lib/data/taxonomy";
import type { EngineNode } from "@/components/constellation/types";
import {
  deleteThread, loadThread, loadThreads, newThreadId, saveThread, type ConversationSummary,
} from "@/lib/chat/threads";

type AskMode = "plain" | "tech";
interface Citation { id: string; title: string; kind: string; trust: string; repo: string }

const INTRO_TEXT =
  `I'm grounded in your org's memory — ${stats.nodes.toLocaleString()} records across ` +
  `${stats.tenants} repos. Ask me anything; I'll cite the exact memories and stream the ` +
  `answer — click any citation to fly to it.`;

/** Concatenate the streamed text parts of a UI message. */
function textOf(m: UIMessage): string {
  return m.parts.filter((p): p is { type: "text"; text: string } => p.type === "text").map((p) => p.text).join("");
}
/** Pull the citations data part (written by /api/chat as `data-citations`). */
function citationsOf(m: UIMessage): Citation[] {
  const part = m.parts.find((p) => p.type === "data-citations") as { data?: Citation[] } | undefined;
  return part?.data ?? [];
}
function whenShort(ms: number): string {
  const d = Math.max(0, Date.now() - ms);
  if (d < 60_000) return "now";
  if (d < 3_600_000) return Math.round(d / 60_000) + "m";
  if (d < 86_400_000) return Math.round(d / 3_600_000) + "h";
  return Math.round(d / 86_400_000) + "d";
}

export function DockTabs({
  tab,
  onTab,
  hasSelection,
  onCollapse,
}: {
  tab: "ask" | "inspect";
  onTab: (t: "ask" | "inspect") => void;
  hasSelection: boolean;
  onCollapse: () => void;
}) {
  return (
    <div className="dock-tabs">
      <div className={"dock-tab" + (tab === "ask" ? " on" : "")} onClick={() => onTab("ask")}>
        <MIcon name="spark" size={15} />Ask
      </div>
      <div className={"dock-tab" + (tab === "inspect" ? " on" : "")} onClick={() => onTab("inspect")}>
        <MIcon name="shield" size={15} />Inspect{hasSelection ? <span className="num">1</span> : null}
      </div>
      <span
        className="collapse-btn"
        style={{ alignSelf: "center", marginLeft: 4, marginBottom: 6 }}
        title="Collapse"
        onClick={onCollapse}
      >
        <MIcon name="chevright" size={14} />
      </span>
    </div>
  );
}

export function Ask({
  org,
  model,
  onModel,
  mode,
  onMode,
  seed,
  onCite,
}: {
  org: string;
  model: string;
  onModel: () => void;
  mode: AskMode;
  onMode: (m: AskMode) => void;
  seed?: EngineNode | null;
  onCite?: (id: string) => void;
}) {
  const [threadId, setThreadId] = useState("");
  const [val, setVal] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [threads, setThreads] = useState<ConversationSummary[]>([]);
  const endRef = useRef<HTMLDivElement>(null);

  // Static `org` in the body; the per-message `tenant` (from "Ask about this") is
  // passed through sendMessage's options below.
  const transport = useMemo(
    () => new DefaultChatTransport({ api: "/api/chat", body: { org } }),
    [org],
  );
  const { messages, sendMessage, status, setMessages } = useChat({ transport });

  // Start a fresh conversation on mount (past ones live in the history drawer, in the DB).
  useEffect(() => {
    let alive = true;
    queueMicrotask(() => { if (alive) setThreadId(newThreadId()); });
    return () => { alive = false; };
  }, [org]);

  // Persist to the DB when a turn finishes (best-effort; no setState → lint-safe).
  useEffect(() => {
    if (status === "ready" && messages.length && threadId) void saveThread(org, threadId, messages);
  }, [status, messages, threadId, org]);

  useEffect(() => { if (endRef.current) endRef.current.scrollTop = endRef.current.scrollHeight; }, [messages, status]);

  // "Ask about this" seeds a grounded question on the selected node's tenant.
  useEffect(() => {
    if (seed) void sendMessage({ text: `What's the story around “${seed.title}”?` }, { body: { org, tenant: seed.tenant } });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seed]);

  const busy = status === "submitted" || status === "streaming";
  function submit(text?: string) {
    const t = (text ?? val).trim();
    if (!t || busy) return;
    setVal("");
    void sendMessage({ text: t });
  }
  function newConversation() {
    setThreadId(newThreadId());
    setMessages([]);
    setHistoryOpen(false);
  }
  async function openThread(id: string) {
    setHistoryOpen(false);
    const conv = await loadThread(id);
    setThreadId(id);
    setMessages(conv?.messages ?? []);
  }
  async function removeThread(id: string) {
    await deleteThread(id);
    setThreads(await loadThreads(org));
    if (id === threadId) newConversation();
  }
  async function toggleHistory() {
    const next = !historyOpen;
    setHistoryOpen(next);
    if (next) setThreads(await loadThreads(org));
  }

  const empty = messages.length === 0;
  const waiting = status === "submitted" && messages.length > 0 && messages[messages.length - 1].role === "user";

  return (
    <div className="ask-wrap">
      <div className="ask-head" style={{ position: "relative" }}>
        <div className="modelpick" onClick={onModel}>
          <span className="mdot" />{model}
          <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={13} /></span>
        </div>
        <button className="tools-btn" title="New conversation" style={{ marginLeft: "auto" }} onClick={newConversation}><MIcon name="plus" size={15} /></button>
        <button className="tools-btn" title="Conversation history" onClick={() => void toggleHistory()}><MIcon name="history" size={15} /></button>
        <div className="mode-toggle" title="How much detail to show">
          <button className={mode === "plain" ? "on" : ""} onClick={() => onMode("plain")}>Plain</button>
          <button className={mode === "tech" ? "on" : ""} onClick={() => onMode("tech")}>Technical</button>
        </div>
        {historyOpen ? (
          <div className="notif-pop" style={{ top: 42, right: 6, width: 282 }} onMouseLeave={() => setHistoryOpen(false)}>
            <div className="np-h"><span className="t">Conversations</span><span className="eyebrow" style={{ marginLeft: "auto" }}>{threads.length} saved</span></div>
            {threads.length ? threads.map((t) => (
              <div className="notif-item" key={t.id} onClick={() => void openThread(t.id)} style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div className="ni-t" style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", color: t.id === threadId ? "var(--citrate-green)" : undefined }}>{t.title || "Untitled"}</div>
                  <div className="ni-w">{whenShort(t.updatedAt)} ago</div>
                </div>
                <span onClick={(e) => { e.stopPropagation(); void removeThread(t.id); }} title="Delete" style={{ cursor: "pointer", color: "var(--ondark-3)" }}><MIcon name="x" size={13} /></span>
              </div>
            )) : <div style={{ padding: 12, fontSize: 12, color: "var(--ondark-3)" }}>No saved conversations yet.</div>}
          </div>
        ) : null}
      </div>
      <div className="ask-thread" ref={endRef}>
        {empty ? <div className="msg-ai"><div className="ai-body">{INTRO_TEXT}</div></div> : null}
        {messages.map((m) => {
          if (m.role === "user") return <div className="msg-user" key={m.id}>{textOf(m)}</div>;
          const cites = citationsOf(m);
          const body = textOf(m);
          return (
            <div className="msg-ai" key={m.id}>
              <div className="ai-body">{body || (status === "streaming" ? "…" : "")}</div>
              {cites.length ? (
                <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 8 }}>
                  {cites.map((cit) => (
                    <button key={cit.id} className="cite" title={`${cit.kind} · ${cit.repo} · ${cit.trust}`} onClick={() => onCite?.(cit.id)}>
                      <span className="cite-dot" style={{ background: laneColor(laneOfKind(cit.kind)) }} />
                      {mode === "tech" ? `ctr:${cit.id.slice(0, 8)}` : truncate(cit.title, 28)}
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          );
        })}
        {waiting ? <div className="msg-ai"><div className="ai-body">Consulting memory…</div></div> : null}
      </div>
      <div className="ask-input" style={{ position: "relative" }}>
        {empty ? (
          <div className="ask-suggest">
            {SUGGESTIONS.map((s, i) => <span className="sugg" key={i} onClick={() => submit(s.q)}>{s.label}</span>)}
          </div>
        ) : null}
        <div className="ask-box">
          <button className="tools-btn" title="Memory tools"><MIcon name="sliders" size={15} /></button>
          <button className="tools-btn" title="Pull a memory into the conversation"><MIcon name="plus" size={16} /></button>
          <input value={val} placeholder="Ask the org's memory anything…" onChange={(e) => setVal(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") submit(); }} />
          <button className="send-btn" onClick={() => submit()}><MIcon name="send" size={15} /></button>
        </div>
      </div>
    </div>
  );
}

const truncate = (s: string, n: number) => (s.length > n ? s.slice(0, n - 1) + "…" : s);
const KIND_LANE: Record<string, string> = {
  Commit: "code", Pr: "code", Doc: "docs", Narrative: "docs", Handoff: "docs",
  Sprint: "specs", Adr: "specs", WorkPackage: "specs", Rationale: "specs",
  ManifestChange: "config", PinBump: "config", DriftEvent: "config",
  Audit: "audit", Finding: "audit", Benchmark: "audit", Blocker: "audit", TechDebt: "audit",
  Claim: "claims", AgentAction: "claims", AnalogyHypothesis: "claims",
};
const laneOfKind = (kind: string) => KIND_LANE[kind] ?? "claims";

/* ---------------- Inspect: trust chips + node card ---------------- */
function PlaneBadge({ plane }: { plane: string }) {
  const derived = plane.toLowerCase() === "derived";
  return <span className={"tag " + (derived ? "plane-derived" : "plane-asserted")}>{derived ? "The record" : "Asserted"}</span>;
}
function TrustChip({ trust }: { trust: string }) {
  const t = TRUST[TRUST_IDX[trust] ?? 2];
  return (
    <span className="tag" title={t.tip} style={{ color: t.ring, borderColor: t.ring + "55", background: t.ring + "11" }}>
      <span className="d" style={{ background: t.ring, boxShadow: `0 0 6px 0 ${t.ring}` }} />{t.short}
    </span>
  );
}
function StatusPill({ status }: { status: string }) {
  const m = ({ Active: "status-active", Superseded: "status-superseded", Archived: "status-archived" } as Record<string, string>)[status] ?? "status-active";
  return <span className={"tag " + m}>{status}</span>;
}

interface Verdict {
  cls: "ok" | "warn" | "bad";
  icon: "check" | "shield" | "x" | "history" | "spark";
  head: string;
  reasons: string[];
}
/** Local trust verdict from the node's own fields (ported from the prototype's verifyOf). */
function verdictOf(node: EngineNode): Verdict {
  if (node.belnap)
    return { cls: "bad", icon: "x", head: "Contradiction recorded", reasons: ["Two assertions about this conflict (Belnap “Both”).", "Resolve it in the Review Center before relying on it."] };
  if (node.status === "Superseded")
    return { cls: "warn", icon: "history", head: "Superseded — newer memory exists", reasons: ["A newer version has replaced this.", "Kept for the record; not load-bearing."] };
  if (node.trust === "InferredAdvisory")
    return { cls: "warn", icon: "spark", head: "Advisory — not yet confirmed", reasons: ["An AI proposed this; it is quarantined.", "It is advisory only until a human confirms it."] };
  if (node.plane.toLowerCase() === "derived")
    return { cls: "ok", icon: "shield", head: "Trustworthy by construction", reasons: ["Rebuilt deterministically from git/markdown.", "No signature needed — this is the record."] };
  if (node.trust === "HumanConfirmed")
    return { cls: "ok", icon: "check", head: "Signature valid · human-confirmed", reasons: ["Signed by its author and confirmed by a person.", "Load-bearing assertion."] };
  return { cls: "ok", icon: "check", head: "Signature valid · a claim", reasons: ["Signed and authentic.", "This is what someone said — not the deterministic record."] };
}

interface NeighborView { id: string; title: string; lane: string; rel: string; dir: "out" | "in"; quarantined: boolean }

/**
 * The trust surface (WP-7.5). Identity + plane/trust/status come from the scene;
 * the signature posture (`verify`) and blast-radius (`neighbors`) are fetched live
 * from the gateway BFF. Degrades gracefully when the gateway is unreachable
 * (sample mode) — it shows the local verdict and a note rather than erroring.
 */
export function InspectNode({
  node, org, hops, onHops, onClose, onNeighbor, onAction,
}: {
  node: EngineNode;
  org: string;
  hops: number;
  onHops: (h: number) => void;
  onClose: () => void;
  onNeighbor: (id: string) => void;
  onAction: (kind: "discuss" | "neighbors" | "analogues" | "propose", node: EngineNode) => void;
}) {
  const color = laneColor(node.lane);
  const lane = LANES[LANE_IDX[node.lane] ?? 5];
  const v = verdictOf(node);
  const [data, setData] = useState<{
    nodeId: string;
    neighbors: NeighborView[] | null;
    live: { trustworthy: boolean; signature: string } | null;
    down: boolean;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    const id = encodeURIComponent(node.id);
    const o = encodeURIComponent(org);
    Promise.allSettled([
      fetch(`/api/orgs/${o}/nodes/${id}/verify`, { cache: "no-store" }),
      fetch(`/api/orgs/${o}/nodes/${id}/neighbors`, { cache: "no-store" }),
    ]).then(async ([vr, nr]) => {
      if (cancelled) return;
      let any = false;
      let live: { trustworthy: boolean; signature: string } | null = null;
      let neighbors: NeighborView[] | null = null;
      if (vr.status === "fulfilled" && vr.value.ok) {
        any = true;
        const j = (await vr.value.json()) as { trustworthy: boolean; signature: string };
        live = { trustworthy: j.trustworthy, signature: j.signature };
      }
      if (nr.status === "fulfilled" && nr.value.ok) {
        any = true;
        const j = (await nr.value.json()) as { neighbors: { edge_kind: string; direction: "out" | "in"; quarantined: boolean; node: Record<string, unknown> }[] };
        neighbors = j.neighbors.slice(0, 8).map((n) => ({
          id: String(n.node.id ?? ""),
          title: String(n.node.title ?? "untitled"),
          lane: String(n.node.lane ?? "claims"),
          rel: n.quarantined ? "proposed" : n.edge_kind,
          dir: n.direction,
          quarantined: n.quarantined,
        }));
      }
      if (!cancelled) setData({ nodeId: node.id, neighbors, live, down: !any });
    });
    return () => { cancelled = true; };
  }, [node.id, org]);

  const loading = !data || data.nodeId !== node.id;
  const neighbors = loading ? null : data.neighbors;
  const live = loading ? null : data.live;
  const gatewayDown = loading ? false : data.down;

  return (
    <div className="dock-body">
      <div className="insp-pad">
        <div className="insp-kind">
          <span className="kdot" style={{ background: color, color }} />
          <span className="eyebrow" style={{ color }}>{node.kind}</span>
          <span style={{ marginLeft: "auto", cursor: "pointer", color: "var(--ondark-3)" }} onClick={onClose}><MIcon name="x" size={15} /></span>
        </div>
        <div className="insp-title">{node.title}</div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 14 }}>
          <PlaneBadge plane={node.plane} />
          <TrustChip trust={node.trust} />
          <StatusPill status={node.status} />
          {node.belnap ? (
            <span className="tag" style={{ color: "#f08", borderColor: "rgba(210,60,40,.4)", background: "rgba(210,60,40,.08)" }}>
              <span className="d" style={{ background: "#e23a28" }} />Contradiction
            </span>
          ) : null}
        </div>
        <div className="insp-grid">
          <span className="k">Repo</span><span className="v">{node.tenant}</span>
          <span className="k">Material</span><span className="v" style={{ color }}>{lane.label} — {lane.desc}</span>
          <span className="k">Address</span><span className="v" style={{ fontFamily: "var(--font-mono)", fontSize: 11.5, color: "var(--ondark-2)" }}>ctr:{node.id.slice(0, 10)}</span>
        </div>

        <div className={"verdict " + v.cls}>
          <div className="vh" style={{ color: v.cls === "ok" ? "var(--citrate-green)" : v.cls === "warn" ? "#f3d27a" : "#f0907f" }}>
            <MIcon name={v.icon} size={16} />{v.head}
          </div>
          <ul className="vr">
            {v.reasons.map((r, i) => <li key={i}>{r}</li>)}
            {live ? <li style={{ color: live.trustworthy ? "var(--citrate-green)" : "#f0907f" }}>Gateway verify: {live.signature} · {live.trustworthy ? "trustworthy" : "not trustworthy"}</li> : null}
          </ul>
        </div>

        <div className="sec-label">Neighbors · blast radius</div>
        {neighbors === null && !gatewayDown ? (
          <div style={{ fontSize: 12, color: "var(--ondark-3)" }}>Loading blast radius…</div>
        ) : neighbors && neighbors.length ? (
          neighbors.map((x, i) => {
            const c = laneColor(x.lane);
            return (
              <div className="neighbor" key={i} onClick={() => x.id && onNeighbor(x.id)}>
                <span className="ndot" style={{ background: c, boxShadow: `0 0 7px 0 ${c}` }} />
                <span className="nt">{x.title}</span>
                <span className="nk" style={{ color: x.quarantined ? "var(--citrate-yellow)" : "var(--ondark-3)" }}>{x.dir === "out" ? "→" : "←"} {x.rel}</span>
              </div>
            );
          })
        ) : gatewayDown ? (
          <div style={{ fontSize: 12, color: "var(--ondark-3)" }}>Blast radius loads from the gateway (unavailable in sample mode).</div>
        ) : (
          <div style={{ fontSize: 12, color: "var(--ondark-3)" }}>No recorded neighbors.</div>
        )}

        <div className="hops-ctl">
          <span className="hops-l"><MIcon name="target" size={13} /> Blast radius</span>
          <div className="seg-pick" style={{ marginLeft: "auto" }}>
            {[1, 2, 3].map((h) => <button key={h} className={hops === h ? "on" : ""} onClick={() => onHops(h)}>{h} hop{h > 1 ? "s" : ""}</button>)}
          </div>
        </div>
        <div className="act-row">
          <button className="act primary" onClick={() => onAction("discuss", node)}><MIcon name="spark" size={14} />Ask about this</button>
          <button className="act" onClick={() => onAction("neighbors", node)}><MIcon name="target" size={14} />Focus neighborhood</button>
          <button className="act" onClick={() => onAction("analogues", node)}><MIcon name="spark" size={14} />Find analogues</button>
          <button className="act" onClick={() => onAction("propose", node)}><MIcon name="plus" size={14} />Propose edge</button>
        </div>
      </div>
    </div>
  );
}

export function InspectEmpty() {
  return (
    <div className="dock-body" style={{ display: "grid", placeItems: "center", textAlign: "center", padding: 30 }}>
      <div style={{ maxWidth: 240 }}>
        <div style={{ color: "var(--ondark-3)", marginBottom: 12 }}><MIcon name="target" size={28} /></div>
        <div style={{ fontFamily: "var(--font-display)", fontSize: 17, color: "var(--ondark)", marginBottom: 6 }}>
          Nothing selected
        </div>
        <div style={{ fontSize: 12.5, color: "var(--ondark-2)", lineHeight: 1.5 }}>
          Click any point in the constellation — or a citation in an answer — to inspect a
          memory and check how trustworthy it is.
        </div>
      </div>
    </div>
  );
}

export function DockMini({ onOpen, hasSelection }: { onOpen: (t: "ask" | "inspect") => void; hasSelection: boolean }) {
  return (
    <div className="dockmini">
      <button className="tb-btn dm-btn panel" title="Ask" onClick={() => onOpen("ask")}>
        <MIcon name="spark" size={19} />
      </button>
      <button className="tb-btn dm-btn panel" title="Inspect" onClick={() => onOpen("inspect")}>
        <MIcon name="shield" size={19} />
        {hasSelection ? (
          <span
            className="num"
            style={{ position: "absolute", top: 5, right: 5, background: "rgba(255,189,16,.16)", color: "var(--citrate-yellow)", borderRadius: 99, padding: "0 5px", fontFamily: "var(--font-mono)", fontSize: 9 }}
          >
            1
          </span>
        ) : null}
      </button>
    </div>
  );
}
