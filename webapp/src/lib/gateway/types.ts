/**
 * Typed mem-gateway HTTP/JSON contracts. These mirror the Rust gateway's
 * response shapes verbatim (crates/mem-gateway/src/http.rs + layout.rs):
 *  - the layout scene feeds the constellation (WP-7.4),
 *  - recall/search/as_of feed Ask + storyline,
 *  - node/verify/neighbors feed the Inspector.
 * The webapp BFF is the ONLY caller of these (planset §2).
 */

export interface OrgSummary {
  id: string;
  name: string;
  status: string;
}

export interface Watermark {
  head: string | null;
  head_count: number;
  ingested_at_ms: number;
}

/** One memory in a recall/search/as_of result list. */
export interface RecallItem {
  id: string;
  kind: string;
  lane: string;
  repo: string;
  title: string;
  plane: string;
  trust: string;
  status: string;
  valid_from: number;
  score: number;
}

export interface RecallResult {
  repo: string;
  total_in_tenant: number;
  watermark: Watermark | null;
  items: RecallItem[];
}

/** A node in the 3D constellation scene (server-positioned). */
export interface SceneNode {
  id: string;
  pos: [number, number, number];
  projected: boolean;
  repo: string;
  kind: string;
  lane: string;
  plane: string;
  trust: string;
  status: string;
  contradicted: boolean;
  degree: number;
  title: string;
}

export interface SceneEdge {
  from: string;
  to: string;
  kind: string;
  quarantined: boolean;
}

export interface Scene {
  nodes: SceneNode[];
  edges: SceneEdge[];
}

export interface VerifyResult {
  id: string;
  trustworthy: boolean;
  signature: string;
  superseded_by: string | null;
  refuted_by: string[];
  contradicted: boolean;
}

export interface Neighbor {
  edge_kind: string;
  direction: "out" | "in";
  quarantined: boolean;
  node: Record<string, unknown>;
}

export interface NeighborsResult {
  id: string;
  neighbors: Neighbor[];
}

export interface AnalogyResult {
  analogues: { node: Record<string, unknown>; cosine: number; structural: number; score: number }[];
}

export interface AuditRecord {
  seq: number;
  ts: number;
  event: "Read" | "Write" | "Denied";
  actor: string;
  resource_id: string;
  detail: string;
}

export interface AuditResult {
  intact: boolean;
  length: number;
  verified_through: number;
  records: AuditRecord[];
}

export interface OpsTenant {
  repo: string;
  anchor: { root: string; node_count: number; edge_count: number; anchored_at_ms: number } | null;
  anchor_valid: boolean | null;
  live_root: string | null;
}
export interface OpsSnapshot {
  org: string;
  store: { nodes: number; edges: number };
  checkpoints: { count: number; latest_ms: number | null; dir: string };
  tenants: OpsTenant[];
  chain_anchor: { root: string; block_number: number; block_hash: string; chain_id: number; fetched_at_ms: number } | null;
}
