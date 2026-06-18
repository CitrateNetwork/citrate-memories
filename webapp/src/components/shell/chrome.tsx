"use client";

/**
 * App-shell chrome — NavRail, TopBar, LeftRail, TimeScrubber, HUD, GuidedOverlay.
 * Ported 1:1 from the prototype's ui.jsx: same DOM + class names so memrizz.css
 * styles it pixel-for-pixel. Behaviour/state is lifted into AppShell.
 */
import { useState } from "react";
import { MIcon, type IconName } from "@/components/icons";
import { useAuth } from "@/lib/auth/client";
import {
  LANES, TRUST, STATUS, MODELS, ME, NOTIFS, T0, span, reviewCount,
} from "@/lib/data/taxonomy";

/** Two-letter initials from an OIDC subject (best-effort, for the avatar chip). */
function subInitials(sub: string): string {
  const tail = sub.includes(":") ? sub.slice(sub.lastIndexOf(":") + 1) : sub;
  const cleaned = tail.replace(/^0x/, "");
  return (cleaned.slice(0, 2) || "ME").toUpperCase();
}

export type View =
  | "constellation" | "ask" | "review" | "connect" | "audit" | "settings" | "profile";
export type Mode = "lattice" | "galaxy" | "islands" | "river";
export type Encoding = "kind" | "trust" | "tenant";

export interface Filters {
  tenants: Set<string> | null;
  lanes: Set<string> | null;
  trusts: Set<string> | null;
  statuses: Set<string> | null;
  showQuarantined: boolean;
}

/* ---------------- Nav rail ---------------- */
export function NavRail({
  view,
  onView,
}: {
  view: View;
  onView: (id: View) => void;
}) {
  const items: { id: View; icon: IconName; label: string; badge?: number }[] = [
    { id: "constellation", icon: "layers", label: "Constellation" },
    { id: "ask", icon: "spark", label: "Ask" },
    { id: "review", icon: "shield", label: "Review", badge: reviewCount },
    { id: "connect", icon: "link", label: "Connect a model" },
    { id: "audit", icon: "ledger", label: "Audit log" },
  ];
  const bottom: { id: View; icon: IconName; label: string; avatar?: boolean }[] = [
    { id: "settings", icon: "sliders", label: "Settings" },
    { id: "profile", icon: "target", label: "Profile", avatar: true },
  ];
  const Btn = (it: { id: View; icon: IconName; label: string; badge?: number; avatar?: boolean }) => (
    <button
      key={it.id}
      className={"nav-btn" + (view === it.id ? " on" : "")}
      onClick={() => onView(it.id)}
    >
      {it.avatar ? (
        <span className="avatar" style={{ width: 24, height: 24, borderRadius: 7, background: ME.color, fontSize: 12 }}>
          {ME.initials}
        </span>
      ) : (
        <MIcon name={it.icon} size={20} />
      )}
      {it.badge ? <span className="nb-badge">{it.badge}</span> : null}
      <span className="nav-tip">{it.label}</span>
    </button>
  );
  return (
    <div className="navrail">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img className="nav-mark" src="/assets/citrate_mark_green.svg" alt="" />
      {items.map(Btn)}
      <div className="nav-sp" />
      {bottom.map(Btn)}
    </div>
  );
}

function NotifPop({ onClose, onGoReview }: { onClose: () => void; onGoReview: () => void }) {
  return (
    <div className="notif-pop" onMouseLeave={onClose}>
      <div className="np-h">
        <span className="t">Notifications</span>
        <span className="eyebrow" style={{ marginLeft: "auto" }}>calm feed</span>
      </div>
      {NOTIFS.map((n) => (
        <div
          className="notif-item"
          key={n.id}
          onClick={() => {
            if (n.kind === "proposal" || n.kind === "contradiction") onGoReview();
            onClose();
          }}
        >
          <span className="ni-ic"><MIcon name={n.icon as IconName} size={15} /></span>
          <div>
            <div className="ni-t">{n.text}</div>
            <div className="ni-s">{n.sub}</div>
            <div className="ni-w">{n.when}</div>
          </div>
        </div>
      ))}
    </div>
  );
}

/* ---------------- Top bar ---------------- */
export function TopBar({
  onOmni,
  model,
  onModel,
  onProfile,
  onGoReview,
}: {
  onOmni: () => void;
  model: string;
  onModel: () => void;
  onProfile: () => void;
  onGoReview: () => void;
}) {
  const [notif, setNotif] = useState(false);
  const auth = useAuth();
  return (
    <div className="topbar">
      <div className="brand">
        <div className="name"><b>Memrizz</b></div>
      </div>
      <div className="divider-v" />
      <div className="switcher" title="Workspace — everything below is scoped to this Org">
        <span className="sw-dot">C</span>
        <div>
          <div className="sw-l1">Citrate Federation</div>
          <div className="sw-l2">Org · 14 repos</div>
        </div>
        <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={14} /></span>
      </div>
      <div className="omnibox" onClick={onOmni}>
        <MIcon name="search" size={15} />
        <span style={{ fontSize: 13 }}>Search memory — filings, decisions, repos…</span>
        <span className="kbd">⌘K</span>
      </div>
      <div className="spacer" />
      <div className="modelpick" onClick={onModel} title="Model used to answer">
        <span className="mdot" />{model}
        <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={13} /></span>
      </div>
      <button className="tb-btn" style={{ position: "relative" }} title="Notifications" onClick={() => setNotif((v) => !v)}>
        <MIcon name="bell" size={16} /><span className="tb-badge" />
      </button>
      {auth.ready && !auth.authenticated ? (
        <button
          className="tb-btn"
          style={{ width: "auto", padding: "0 12px", gap: 6, fontWeight: 600, color: "var(--ondark)" }}
          title="Sign in with your Citrate identity"
          onClick={() => void auth.login()}
        >
          <MIcon name="shield" size={15} /> Sign in
        </button>
      ) : (
        <>
          <div
            className="switcher"
            style={{ padding: "0 6px 0 9px" }}
            title={auth.authenticated ? `Signed in${auth.sub ? ` · ${auth.sub}` : ""}` : "You"}
            onClick={onProfile}
          >
            <span className="sw-dot" style={{ background: ME.color }}>
              {auth.authenticated && auth.sub ? subInitials(auth.sub) : ME.initials}
            </span>
            <span style={{ color: "var(--ondark-3)" }}><MIcon name="chevdown" size={14} /></span>
          </div>
          {auth.authenticated ? (
            <button
              className="tb-btn"
              style={{ width: "auto", padding: "0 10px", fontSize: 12, fontWeight: 600 }}
              title="Sign out"
              onClick={() => void auth.logout()}
            >
              Sign out
            </button>
          ) : null}
        </>
      )}
      {notif ? <NotifPop onClose={() => setNotif(false)} onGoReview={onGoReview} /> : null}
    </div>
  );
}

/* ---------------- Left rail ---------------- */
export const LAYOUTS: { id: Mode; s1: string; s2: string; icon: IconName }[] = [
  { id: "lattice", s1: "Lattice", s2: "Type · Time · Trust", icon: "grid" },
  { id: "galaxy", s1: "Galaxy", s2: "By meaning", icon: "spark" },
  { id: "islands", s1: "Islands", s2: "By repo", icon: "cube" },
  { id: "river", s1: "River", s2: "By time", icon: "waves" },
];

function FilterChip({
  on,
  color,
  round,
  label,
  onClick,
}: {
  on: boolean;
  color?: string;
  round?: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <span className={"fchip" + (on ? "" : " off") + (round ? " round" : "")} onClick={onClick}>
      {color ? (
        <span className="sw" style={{ background: color, boxShadow: on ? `0 0 7px 0 ${color}` : "none" }} />
      ) : null}
      {label}
    </span>
  );
}

export function LeftRail({
  collapsed,
  onCollapse,
  mode,
  onMode,
  filters,
  setFilter,
  encoding,
}: {
  collapsed: boolean;
  onCollapse: () => void;
  mode: Mode;
  onMode: (m: Mode) => void;
  filters: Filters;
  setFilter: (key: keyof Filters, val: Filters[keyof Filters]) => void;
  encoding: Encoding;
}) {
  function toggleSet(key: "lanes" | "trusts" | "statuses", val: string) {
    let full: string[];
    if (key === "lanes") full = LANES.map((l) => l.id);
    else if (key === "trusts") full = TRUST.map((t) => t.id);
    else full = [...STATUS];
    const cur = filters[key];
    const s = cur ? new Set(cur) : new Set(full);
    if (s.has(val)) s.delete(val);
    else s.add(val);
    setFilter(key, s.size === full.length ? null : s);
  }
  const isOn = (key: "lanes" | "trusts" | "statuses", val: string) => {
    const c = filters[key];
    return c ? c.has(val) : true;
  };

  if (collapsed) {
    return (
      <div className="leftrail collapsed panel" style={{ position: "absolute" }}>
        <div className="rail-toggle" onClick={onCollapse}><MIcon name="chevright" size={15} /></div>
        <div className="rail-icon-col">
          {LAYOUTS.map((l) => (
            <button key={l.id} className={mode === l.id ? "on" : ""} title={l.s1} onClick={() => onMode(l.id)}>
              <MIcon name={l.icon} size={18} />
            </button>
          ))}
          <div style={{ height: 1, background: "var(--hair)", width: 28, margin: "6px 0" }} />
          <button title="Filters" onClick={onCollapse}><MIcon name="filter" size={18} /></button>
        </div>
      </div>
    );
  }
  return (
    <div className="leftrail panel">
      <div className="rail-toggle" onClick={onCollapse}><MIcon name="chevleft" size={15} /></div>
      <div className="panel-h">
        <MIcon name="layers" size={16} />
        <div className="t">Constellation</div>
      </div>
      <div className="rail-scroll">
        <div className="rail-sec">
          <div className="lbl"><span className="eyebrow">View</span></div>
          <div className="seg">
            {LAYOUTS.map((l) => (
              <button key={l.id} className={"seg-btn" + (mode === l.id ? " on" : "")} onClick={() => onMode(l.id)}>
                <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  <MIcon name={l.icon} size={14} /><span className="s1">{l.s1}</span>
                </span>
                <span className="s2">{l.s2}</span>
              </button>
            ))}
          </div>
        </div>

        <div className="rail-sec">
          <div className="lbl">
            <span className="eyebrow">Material type</span>
            <span className="eyebrow" style={{ color: encoding === "kind" ? "var(--citrate-green)" : "var(--ondark-3)" }}>
              {encoding === "kind" ? "● color" : ""}
            </span>
          </div>
          <div className="chips">
            {LANES.map((l) => (
              <FilterChip key={l.id} on={isOn("lanes", l.id)} color={l.color} label={l.label} onClick={() => toggleSet("lanes", l.id)} />
            ))}
          </div>
        </div>

        <div className="rail-sec">
          <div className="lbl">
            <span className="eyebrow">Trust</span>
            <span className="eyebrow" style={{ color: encoding === "trust" ? "var(--citrate-green)" : "var(--ondark-3)" }}>
              {encoding === "trust" ? "● color" : ""}
            </span>
          </div>
          <div className="chips">
            {TRUST.map((t) => (
              <FilterChip key={t.id} on={isOn("trusts", t.id)} color={t.ring} round label={t.short} onClick={() => toggleSet("trusts", t.id)} />
            ))}
          </div>
        </div>

        <div className="rail-sec">
          <div className="lbl"><span className="eyebrow">Status</span></div>
          <div className="chips">
            {STATUS.map((s) => (
              <FilterChip key={s} on={isOn("statuses", s)} label={s} onClick={() => toggleSet("statuses", s)} />
            ))}
          </div>
          <div className="toggle-row" style={{ marginTop: 10 }}>
            <span className="tl">Show proposed (advisory)</span>
            <span
              className={"sw-toggle" + (filters.showQuarantined ? " on" : "")}
              onClick={() => setFilter("showQuarantined", !filters.showQuarantined)}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

/* ---------------- Time scrubber ---------------- */
export function TimeScrubber({
  t,
  onT,
  playing,
  onPlay,
  onCollapse,
}: {
  t: number;
  onT: (v: number) => void;
  playing: boolean;
  onPlay: (v: boolean) => void;
  onCollapse: () => void;
}) {
  const cutoff = T0 + t * span;
  const plain = new Date(cutoff).toLocaleDateString("en-US", { month: "long", day: "numeric", year: "numeric" });
  const isNow = t > 0.992;
  function fromEvent(e: PointerEvent, el: HTMLElement) {
    const r = el.getBoundingClientRect();
    return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
  }
  function down(e: React.PointerEvent<HTMLElement>) {
    onPlay(false);
    const rail = e.currentTarget.closest(".scrub-track")?.querySelector(".scrub-rail") as HTMLElement | null;
    const el = rail ?? (e.currentTarget as HTMLElement);
    const move = (ev: PointerEvent) => onT(fromEvent(ev, el));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    onT(fromEvent(e.nativeEvent, el));
  }
  const ticks = ["Jan ’25", "Apr", "Jul", "Oct", "Jan ’26", "Jun"];
  return (
    <div className="scrubber panel">
      <div className="scrub-head">
        <div className="plain">
          {isNow ? <span>Memory as it is <b>now</b></span> : <span>Rewound to <b>{plain}</b></span>}
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
          <div className="scrub-play" onClick={() => onPlay(!playing)}>
            <MIcon name={playing ? "pause" : "history"} size={14} />{playing ? "Pause" : "Replay history"}
          </div>
          <span className="collapse-btn" title="Hide timeline" onClick={onCollapse}><MIcon name="x" size={14} /></span>
        </div>
      </div>
      <div className="scrub-track">
        <div className="scrub-ticks">{ticks.map((tk, i) => <span key={i} className="scrub-tick">{tk}</span>)}</div>
        <div className="scrub-rail" onPointerDown={down}>
          <div className="scrub-fill" style={{ width: t * 100 + "%" }} />
        </div>
        <div className="scrub-knob" style={{ left: t * 100 + "%" }} onPointerDown={down} />
      </div>
    </div>
  );
}

export function ScrubMini({ t, onOpen }: { t: number; onOpen: () => void }) {
  const cutoff = T0 + t * span;
  const isNow = t > 0.992;
  const d = new Date(cutoff).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "2-digit" });
  return (
    <div className="scrubmini panel" onClick={onOpen} title="Show memory timeline">
      <MIcon name="history" size={15} />
      <span className="sm-d">{isNow ? <span>Memory · <b>now</b></span> : <span><b>{d}</b></span>}</span>
      <MIcon name="chevright" size={13} />
    </div>
  );
}

/* ---------------- HUD + guide ---------------- */
export function Hud({ visible, nodes, edges }: { visible: number; nodes: number; edges: number }) {
  return (
    <div className="hud">
      <span className="stat"><b className="tnum">{nodes.toLocaleString()}</b> nodes</span>
      <span className="stat">·</span>
      <span className="stat"><b className="tnum">{edges.toLocaleString()}</b> edges</span>
      <span className="stat">·</span>
      <span className="stat"><b className="tnum">{visible.toLocaleString()}</b> in view</span>
    </div>
  );
}

export function GuidedOverlay({ onStart }: { onStart: () => void }) {
  return (
    <div className="guide-back">
      <div className="guide panel">
        <div className="ge eyebrow">Citrate Federation · Organization memory</div>
        <h2>Everything your org remembers, as one living constellation.</h2>
        <p>
          This is the memory of 14 repositories — every decision, document, commit and
          claim, held as a map you can fly through and simply <i>ask</i>. Nothing here is
          guessed at you: each point shows how sure it is and how current.
        </p>
        <div className="gsteps">
          <div className="gstep">
            <div className="gi" style={{ color: "var(--citrate-green)" }}><MIcon name="target" size={16} /></div>
            <div><div className="gtt">See it</div><div className="gtd">Drag to orbit, scroll to zoom. Each glowing point is a memory; brighter means more trusted.</div></div>
          </div>
          <div className="gstep">
            <div className="gi" style={{ color: "var(--citrate-yellow)" }}><MIcon name="spark" size={16} /></div>
            <div><div className="gtt">Ask it</div><div className="gtd">Ask a plain-language question. The answer cites real memories — click a citation and it flies into view.</div></div>
          </div>
          <div className="gstep">
            <div className="gi" style={{ color: "#9fc0e8" }}><MIcon name="history" size={16} /></div>
            <div><div className="gtt">Rewind it</div><div className="gtd">Drag the timeline at the bottom to watch the memory breathe through history.</div></div>
          </div>
        </div>
        <div className="gactions">
          <button className="btn-primary-lg" onClick={onStart}>Enter the constellation</button>
          <button className="link-muted" onClick={onStart}>Skip the tour</button>
        </div>
      </div>
    </div>
  );
}

export const MODEL_LIST = MODELS;
