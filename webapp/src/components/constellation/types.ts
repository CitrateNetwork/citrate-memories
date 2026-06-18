/** Engine data model — the decoupled shape the ConstellationEngine consumes. */

export interface Vec3 {
  x: number;
  y: number;
  z: number;
}

/** A node as the engine needs it (derived from a gateway SceneNode + indices). */
export interface EngineNode {
  id: string;
  idx: number;
  kind: string;
  lane: string;
  laneIdx: number;
  plane: string;
  trust: string;
  trustIdx: number;
  status: string;
  tenant: string;
  tenantIdx: number;
  title: string;
  tf: number; // time fraction 0..1
  when: number; // absolute ms
  belnap: boolean; // contradiction
  deg: number;
  sizeW: number;
  seed: number; // deterministic 0..1
  sx: number;
  sy: number;
  sz: number; // semantic jitter (from the server projection)
  supersededBy?: string;
  // runtime (assigned by the engine)
  pos: Vec3;
  tpos: Vec3;
  _set: boolean;
  _eff?: string;
  _vis?: boolean;
  _sx?: number;
  _sy?: number;
  _r?: number;
}

export interface EngineEdge {
  a: string;
  b: string;
  ai: number;
  bi: number;
  kind: string;
  quarantined: boolean;
  e: number; // edge index
}

export interface Graph {
  nodes: EngineNode[];
  edges: EngineEdge[];
  byId: Record<string, EngineNode>;
  /** adjacency: node id → [{ o: otherId, e: edgeIdx }] */
  adj: Record<string, { o: string; e: number }[]>;
  tenants: string[];
  t0: number;
  span: number;
  /** galaxy cluster centers, keyed by `${lane}|${tenantIdx}` */
  clusterCenter: Record<string, Vec3>;
}

export type LayoutMode = "lattice" | "galaxy" | "islands" | "river";
export type Encoding = "kind" | "trust" | "tenant";

export interface EngineFilters {
  tenants: Set<string> | null;
  lanes: Set<string> | null;
  trusts: Set<string> | null;
  statuses: Set<string> | null;
  showQuarantined: boolean;
}

export interface EngineOptions {
  onSelect?: (node: EngineNode | null) => void;
  onHover?: (node: EngineNode | null, x: number, y: number) => void;
  onMarquee?: (ids: string[]) => void;
  onReady?: (engine: unknown) => void;
}
