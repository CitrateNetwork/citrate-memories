"use client";

/**
 * React island that owns the constellation <canvas> and the engine lifecycle.
 * Fetches the Org scene from the BFF (`/api/orgs/[org]/layout`); if the gateway
 * is unreachable it falls back to a clearly-labelled deterministic SAMPLE so the
 * field isn't blank in local dev (Rule 11: sample is marked, never passed off as
 * org memory). Engine state (mode/encoding/filters/timeT) is driven from props.
 */
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { ConstellationEngine } from "./engine";
import { buildGraphFromScene } from "./graph";
import { sampleScene } from "./sample";
import type { Encoding, EngineFilters, EngineNode, LayoutMode } from "./types";
import type { Scene } from "@/lib/gateway/types";

/** Imperative controls the Inspector/Ask use to drive the engine. */
export interface ConstellationHandle {
  select: (id: string | null) => void;
  setFocusHops: (n: number) => void;
  showAnalogues: (id: string) => void;
  setCited: (ids: string[]) => void;
  setLassoMode: (on: boolean) => void;
  resetView: () => void;
}

export interface ConstellationProps {
  org: string;
  mode: LayoutMode;
  encoding: Encoding;
  filters: EngineFilters;
  timeT: number;
  reduced?: boolean;
  minimap?: HTMLCanvasElement | null;
  onSelect?: (node: EngineNode | null) => void;
  onHover?: (node: EngineNode | null, x: number, y: number) => void;
  onMarquee?: (ids: string[]) => void;
  onSource?: (source: "gateway" | "sample") => void;
}

export const Constellation = forwardRef<ConstellationHandle, ConstellationProps>(function Constellation(
  { org, mode, encoding, filters, timeT, reduced, minimap, onSelect, onHover, onMarquee, onSource },
  ref,
) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<ConstellationEngine | null>(null);
  const [scene, setScene] = useState<{ data: Scene; source: "gateway" | "sample" } | null>(null);

  useImperativeHandle(ref, () => ({
    select: (id) => engineRef.current?.select(id),
    setFocusHops: (n) => engineRef.current?.setFocusHops(n),
    showAnalogues: (id) => { engineRef.current?.showAnalogues(id); },
    setCited: (ids) => engineRef.current?.setCited(ids),
    setLassoMode: (on) => engineRef.current?.setLassoMode(on),
    resetView: () => engineRef.current?.resetView(),
  }), []);

  // Fetch the scene once per Org (falls back to sample on any failure).
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch(`/api/orgs/${encodeURIComponent(org)}/layout`, { cache: "no-store" });
        if (!res.ok) throw new Error(String(res.status));
        const data = (await res.json()) as Scene;
        if (!cancelled) setScene({ data, source: "gateway" });
      } catch {
        if (!cancelled) setScene({ data: sampleScene(), source: "sample" });
      }
    })();
    return () => { cancelled = true; };
  }, [org]);

  // (Re)build the engine when a scene arrives.
  useEffect(() => {
    if (!scene || !canvasRef.current) return;
    onSource?.(scene.source);
    const graph = buildGraphFromScene(scene.data);
    const engine = new ConstellationEngine(canvasRef.current, graph, {
      onSelect: (n) => onSelect?.(n),
      onHover: (n, x, y) => onHover?.(n, x, y),
      onMarquee: (ids) => onMarquee?.(ids),
    });
    engineRef.current = engine;
    return () => { engine.dispose(); engineRef.current = null; };
    // onSelect/onHover/onMarquee are stable callbacks from the parent; rebuild only on scene.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scene]);

  // Drive engine state from props.
  useEffect(() => { engineRef.current?.setState({ mode, encoding, reduced: !!reduced }); }, [mode, encoding, reduced]);
  useEffect(() => { engineRef.current?.setState({ filters }); }, [filters]);
  useEffect(() => { engineRef.current?.setState({ timeT }); }, [timeT]);
  useEffect(() => { if (minimap) engineRef.current?.setMinimap(minimap); }, [minimap, scene]);

  return <canvas ref={canvasRef} className="constellation" />;
});
