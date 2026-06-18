/** MIcon — the unified SVG icon set, ported 1:1 from the prototype's ui.jsx. */
import type { JSX } from "react";

export type IconName =
  | "search" | "bell" | "chevdown" | "chevright" | "chevleft" | "layers"
  | "filter" | "clock" | "history" | "spark" | "sliders" | "eye" | "send"
  | "x" | "play" | "pause" | "check" | "target" | "branch" | "shield" | "plus"
  | "reset" | "info" | "grid" | "cube" | "waves" | "help" | "link" | "ledger"
  | "copy" | "download" | "refresh";

export function MIcon({
  name,
  size = 16,
  stroke = 1.6,
}: {
  name: IconName;
  size?: number;
  stroke?: number;
}) {
  const paths: Record<IconName, JSX.Element> = {
    search: <g><circle cx="10" cy="10" r="6" /><path d="M15 15 L21 21" /></g>,
    bell: <g><path d="M18 16 V11 A6 6 0 0 0 6 11 V16 L4 18 H20 L18 16Z" /><path d="M10 21 A2 2 0 0 0 14 21" /></g>,
    chevdown: <path d="M6 9 L12 15 L18 9" />,
    chevright: <path d="M9 6 L15 12 L9 18" />,
    chevleft: <path d="M15 6 L9 12 L15 18" />,
    layers: <g><path d="M12 3 L21 8 L12 13 L3 8 Z" /><path d="M3 13 L12 18 L21 13" /></g>,
    filter: <path d="M4 5 H20 M7 12 H17 M10 19 H14" />,
    clock: <g><circle cx="12" cy="12" r="9" /><path d="M12 7 V12 L15 14" /></g>,
    history: <g><path d="M3 12 a9 9 0 1 0 3-6.7 M3 4 V8 H7" /><path d="M12 8 V12 L15 14" /></g>,
    spark: <path d="M12 3 L13.6 9.4 L20 11 L13.6 12.6 L12 19 L10.4 12.6 L4 11 L10.4 9.4 Z" />,
    sliders: <g><path d="M4 8 H20 M4 16 H20" /><circle cx="9" cy="8" r="2.2" /><circle cx="15" cy="16" r="2.2" /></g>,
    eye: <g><path d="M2 12 S6 5 12 5 S22 12 22 12 S18 19 12 19 S2 12 2 12Z" /><circle cx="12" cy="12" r="3" /></g>,
    send: <path d="M4 12 L20 4 L14 20 L11 13 Z" />,
    x: <path d="M6 6 L18 18 M18 6 L6 18" />,
    play: <path d="M7 5 L19 12 L7 19 Z" />,
    pause: <g><path d="M8 5 V19 M16 5 V19" /></g>,
    check: <path d="M5 12 L10 17 L19 8" />,
    target: <g><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="3" /><path d="M12 2 V5 M12 19 V22 M2 12 H5 M19 12 H22" /></g>,
    branch: <g><circle cx="6" cy="6" r="2.4" /><circle cx="6" cy="18" r="2.4" /><circle cx="18" cy="10" r="2.4" /><path d="M6 8.4 V15.6 M6 12 H13 a3 3 0 0 0 3-3 V12.4" /></g>,
    shield: <g><path d="M12 3 L20 6 V12 C20 17 16 21 12 22 C8 21 4 17 4 12 V6 Z" /><path d="M9 12 L11 14 L15 9" /></g>,
    plus: <path d="M12 4 V20 M4 12 H20" />,
    reset: <g><path d="M3 12 a9 9 0 1 1 3 6.7 M3 16 V20 M3 20 H7" /></g>,
    info: <g><circle cx="12" cy="12" r="9" /><path d="M12 11 V16 M12 8 H12.01" /></g>,
    grid: <g><rect x="3" y="3" width="7" height="7" /><rect x="14" y="3" width="7" height="7" /><rect x="3" y="14" width="7" height="7" /><rect x="14" y="14" width="7" height="7" /></g>,
    cube: <g><path d="M12 3 L21 8 V16 L12 21 L3 16 V8 Z" /><path d="M3 8 L12 13 L21 8 M12 13 V21" /></g>,
    waves: <path d="M3 8 Q7 4 12 8 T21 8 M3 14 Q7 10 12 14 T21 14" />,
    help: <g><circle cx="12" cy="12" r="9" /><path d="M9.5 9.5 A2.5 2.5 0 1 1 12 13 V14.5 M12 18 H12.01" /></g>,
    link: <g><path d="M10 13 a4 4 0 0 0 6 0 l3-3 a4 4 0 0 0-6-6 l-1 1" /><path d="M14 11 a4 4 0 0 0-6 0 l-3 3 a4 4 0 0 0 6 6 l1-1" /></g>,
    ledger: <g><rect x="5" y="3" width="14" height="18" rx="1.5" /><path d="M9 7 H15 M9 11 H15 M9 15 H13" /></g>,
    copy: <g><rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15 V5 a2 2 0 0 1 2-2 H15" /></g>,
    download: <g><path d="M12 3 V15 M7 11 L12 16 L17 11" /><path d="M4 20 H20" /></g>,
    refresh: <g><path d="M3 12 a9 9 0 0 1 15-6.7 L21 8 M21 3 V8 H16" /><path d="M21 12 a9 9 0 0 1-15 6.7 L3 16 M3 21 V16 H8" /></g>,
  };
  return (
    <svg
      viewBox="0 0 24 24"
      style={{
        width: size,
        height: size,
        strokeWidth: stroke,
        fill: "none",
        stroke: "currentColor",
        strokeLinecap: "round",
        strokeLinejoin: "round",
      }}
    >
      {paths[name] ?? null}
    </svg>
  );
}
