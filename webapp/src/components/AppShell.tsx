"use client";

/**
 * AppShell — the command-deck stage that assembles the chrome (NavRail, TopBar,
 * LeftRail, RightDock, TimeScrubber, HUD), ported 1:1 from app.jsx's <App/>.
 *
 * WP-7.3 scope: the full shell + interactions that don't need the graph engine
 * (layout/filter state, dock + scrubber collapse, model cycling, the guided
 * overlay, ⌘K stub). The <canvas> is a real element on the dark field; the
 * ConstellationEngine that draws into it is WP-7.4, and the non-constellation
 * routes (Review/Connect/Audit/Settings/Profile) are M1/M2 — rendered here as
 * honest "lands in …" stubs so the nav is wired without faking content.
 */
import { useEffect, useRef, useState } from "react";
import { MIcon } from "@/components/icons";
import {
  NavRail, TopBar, LeftRail, TimeScrubber, ScrubMini, Hud, GuidedOverlay,
  MODEL_LIST, type View, type Mode, type Filters,
} from "@/components/shell/chrome";
import { DockTabs, Ask, InspectEmpty, InspectNode, DockMini } from "@/components/shell/dock";
import { ReviewCenter } from "@/components/shell/review";
import { AdminConsole } from "@/components/shell/admin";
import { AuditViewer } from "@/components/shell/audit";
import { ConnectPage } from "@/components/shell/connect";
import { Constellation, type ConstellationHandle } from "@/components/constellation/Constellation";
import type { EngineNode } from "@/components/constellation/types";
import { laneColor, stats, TRUST, TRUST_IDX, fmtAgo } from "@/lib/data/taxonomy";

const ORG = "citrate-federation";

function Tooltip({ tip }: { tip: { node: EngineNode; x: number; y: number } | null }) {
  if (!tip) return null;
  const n = tip.node;
  const color = laneColor(n.lane);
  return (
    <div className="tip" style={{ left: Math.min(tip.x + 16, (typeof window !== "undefined" ? window.innerWidth : 1440) - 260), top: tip.y + 16 }}>
      <div className="tt">{n.title}</div>
      <div className="tm">
        <span><b style={{ color }}>{n.kind}</b></span>
        <span>{n.tenant}</span>
        <span>{TRUST[TRUST_IDX[n.trust] ?? 2].short}</span>
        <span>{fmtAgo(n.when)}</span>
      </div>
    </div>
  );
}

const ROUTE_STUB: Partial<Record<View, { title: string; wp: string }>> = {
  review: { title: "Review Center", wp: "M1 · WP-7.8 (proposals · contradictions · supersession · self-critic)" },
  connect: { title: "Connect a model (BYOM)", wp: "M2 · WP-7.9 (MCP-over-HTTP endpoint + tokens)" },
  audit: { title: "Audit log", wp: "M2 · WP-7.10 (integrity-verified, live tail)" },
  settings: { title: "Settings", wp: "M2 · WP-7.9 (org, models, delegation, danger zone)" },
  profile: { title: "Profile", wp: "M2 (your Orgs, grants, connected models, audit)" },
};

export function AppShell() {
  const [guided, setGuided] = useState(true);
  const [view, setView] = useState<View>("constellation");
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [dockOpen, setDockOpen] = useState(true);
  const [scrubOpen, setScrubOpen] = useState(true);
  const [tab, setTab] = useState<"ask" | "inspect">("ask");
  const [mode, setMode] = useState<Mode>("lattice");
  const [askMode, setAskMode] = useState<"plain" | "tech">("plain");
  const [filters, setFilters] = useState<Filters>({
    tenants: null, lanes: null, trusts: null, statuses: null, showQuarantined: true,
  });
  const [timeT, setTimeT] = useState(1);
  const [playing, setPlaying] = useState(false);
  const [modelIdx, setModelIdx] = useState(0);
  const [cmdHint, setCmdHint] = useState(false);
  const [selected, setSelected] = useState<EngineNode | null>(null);
  const [tip, setTip] = useState<{ node: EngineNode; x: number; y: number } | null>(null);
  const [source, setSource] = useState<"gateway" | "sample" | null>(null);
  const [minimapEl, setMinimapEl] = useState<HTMLCanvasElement | null>(null);
  const [hops, setHops] = useState(1);
  const [askSeed, setAskSeed] = useState<EngineNode | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const conRef = useRef<ConstellationHandle>(null);
  const flash = (m: string) => { setToast(m); setTimeout(() => setToast(null), 2600); };

  function onNodeAction(kind: "discuss" | "neighbors" | "analogues" | "propose", node: EngineNode) {
    if (kind === "discuss") { setAskSeed(node); setTab("ask"); setDockOpen(true); }
    else if (kind === "neighbors") conRef.current?.select(node.id);
    else if (kind === "analogues") { conRef.current?.showAnalogues(node.id); flash("Beaming in cross-repo analogues…"); }
    else if (kind === "propose") flash("Propose-edge flow lives in the Review Center (WP-7.8).");
  }

  const modelName = MODEL_LIST[modelIdx].name;
  const cycleModel = () => setModelIdx((i) => (i + 1) % MODEL_LIST.length);
  const setFilter = (key: keyof Filters, val: Filters[keyof Filters]) =>
    setFilters((f) => ({ ...f, [key]: val }));

  // ⌘K — palette lands in M1; for now flash an honest hint.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setCmdHint(true);
        setTimeout(() => setCmdHint(false), 2400);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Replay animation drives the scrubber (engine-independent).
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    let last = performance.now();
    const tick = (now: number) => {
      const dt = (now - last) / 1000;
      last = now;
      setTimeT((p) => {
        const nx = p + dt * 0.18;
        if (nx >= 1) {
          setPlaying(false);
          return 1;
        }
        return nx;
      });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]);

  function onView(id: View) {
    if (id === "ask") {
      setView("constellation");
      setTab("ask");
      setDockOpen(true);
    } else {
      setView(id);
    }
  }
  const navView: View =
    view === "constellation" ? (dockOpen && tab === "ask" ? "ask" : "constellation") : view;
  const inConst = view === "constellation";

  return (
    <div className="stage">
      <div className="canvas-wrap">
        <Constellation
          ref={conRef}
          org={ORG}
          mode={mode}
          encoding="kind"
          filters={filters}
          timeT={timeT}
          minimap={minimapEl}
          onSource={setSource}
          onHover={(node, x, y) => setTip(node ? { node, x, y } : null)}
          onSelect={(node) => {
            setSelected(node);
            if (node) { setView("constellation"); setTab("inspect"); setDockOpen(true); }
          }}
        />
      </div>

      <NavRail view={navView} onView={onView} />
      <TopBar
        onOmni={() => { setCmdHint(true); setTimeout(() => setCmdHint(false), 2400); }}
        model={modelName}
        onModel={cycleModel}
        onProfile={() => setView("profile")}
        onGoReview={() => setView("review")}
      />

      {inConst ? (
        <>
          <LeftRail
            collapsed={leftCollapsed}
            onCollapse={() => setLeftCollapsed((c) => !c)}
            mode={mode}
            onMode={setMode}
            filters={filters}
            setFilter={setFilter}
            encoding="kind"
          />

          {dockOpen ? (
            <div className="rightdock panel">
              <DockTabs tab={tab} onTab={setTab} hasSelection={!!selected} onCollapse={() => setDockOpen(false)} />
              {tab === "ask" ? (
                <Ask
                  org={ORG}
                  model={modelName}
                  onModel={cycleModel}
                  mode={askMode}
                  onMode={setAskMode}
                  seed={askSeed}
                  onCite={(id) => { conRef.current?.setCited([id]); conRef.current?.select(id); }}
                />
              ) : selected ? (
                <InspectNode
                  node={selected}
                  org={ORG}
                  hops={hops}
                  onHops={(h) => { setHops(h); conRef.current?.setFocusHops(h); }}
                  onClose={() => { setSelected(null); conRef.current?.select(null); }}
                  onNeighbor={(id) => conRef.current?.select(id)}
                  onAction={onNodeAction}
                />
              ) : (
                <InspectEmpty />
              )}
            </div>
          ) : (
            <DockMini onOpen={(which) => { setTab(which); setDockOpen(true); }} hasSelection={!!selected} />
          )}

          <div className="minimap-wrap panel">
            <div className="minimap-l">Overview</div>
            <canvas className="minimap" width={132} height={132} ref={setMinimapEl} />
          </div>

          {scrubOpen ? (
            <TimeScrubber t={timeT} onT={setTimeT} playing={playing} onPlay={setPlaying} onCollapse={() => setScrubOpen(false)} />
          ) : (
            <ScrubMini t={timeT} onOpen={() => setScrubOpen(true)} />
          )}
          <Hud visible={stats.nodes} nodes={stats.nodes} edges={stats.edges} />
          <Tooltip tip={tip} />
          {source === "sample" ? (
            <div
              className="panel"
              style={{ position: "absolute", left: 74, top: 70, zIndex: 27, padding: "7px 11px", display: "flex", alignItems: "center", gap: 8, fontSize: 11.5, color: "var(--ondark-2)" }}
            >
              <MIcon name="info" size={14} />
              Sample data — the memory gateway isn&rsquo;t connected. Set MEM_GATEWAY_ORIGIN.
            </div>
          ) : null}
        </>
      ) : view === "review" ? (
        <ReviewCenter
          org={ORG}
          onToast={flash}
          onCite={(id) => { setView("constellation"); conRef.current?.setCited([id]); conRef.current?.select(id); }}
        />
      ) : view === "settings" ? (
        <AdminConsole org={ORG} onToast={flash} />
      ) : view === "audit" ? (
        <AuditViewer org={ORG} />
      ) : view === "connect" ? (
        <ConnectPage org={ORG} onToast={flash} />
      ) : (
        <div className="route">
          <div className="route-inner" style={{ display: "grid", placeItems: "center", minHeight: "60vh", textAlign: "center" }}>
            <div style={{ maxWidth: 460 }}>
              <div className="eyebrow" style={{ color: "var(--ondark-3)", marginBottom: 10 }}>Coming surface</div>
              <h2 style={{ color: "var(--ondark)" }}>{ROUTE_STUB[view]?.title ?? view}</h2>
              <p style={{ color: "var(--ondark-2)" }}>
                This surface lands in {ROUTE_STUB[view]?.wp ?? "a later milestone"}. The shell,
                nav, and design system are in place; the content wires to the gateway per the
                implementation planset.
              </p>
              <button className="btn-primary-lg" style={{ marginTop: 18 }} onClick={() => setView("constellation")}>
                Back to the constellation
              </button>
            </div>
          </div>
        </div>
      )}

      {cmdHint ? (
        <div className="toast">
          <MIcon name="search" size={16} />
          <span className="tk">Command palette (⌘K) and search land in M1 · WP-7.6.</span>
        </div>
      ) : null}
      {toast ? (
        <div className="toast">
          <MIcon name="spark" size={16} />
          <span className="tk">{toast}</span>
        </div>
      ) : null}
      {guided ? <GuidedOverlay onStart={() => setGuided(false)} /> : null}
    </div>
  );
}
