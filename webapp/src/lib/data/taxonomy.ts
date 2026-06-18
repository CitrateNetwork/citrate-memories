/**
 * The static ontology taxonomy + chrome constants, ported 1:1 from the design
 * prototype's data.js / extras.js. These are deterministic (kinds, lanes, trust
 * tiers, statuses, edge styles) and live in the engine's `mem-core` ontology too
 * (planset §1.4) — they are NOT graph data. Live graph data (nodes/edges/counts)
 * comes from mem-gateway in WP-7.2/7.4; the `stats` below are PLACEHOLDERS shaped
 * to the design's default screenshots until that wiring lands.
 */

export interface Lane {
  id: string;
  label: string;
  color: string;
  desc: string;
}
export interface TrustTier {
  id: string;
  label: string;
  short: string;
  ring: string;
  tip: string;
}
export interface Model {
  id: string;
  name: string;
  provider: string;
  kind: string;
  cost: string;
  latency: string;
  note: string;
}
export interface Notif {
  id: string;
  icon: string;
  text: string;
  sub: string;
  when: string;
  kind: string;
}

/** Six material lanes (node color = kind). */
export const LANES: Lane[] = [
  { id: "code", label: "Code", color: "#8ecc09", desc: "the built thing" },
  { id: "docs", label: "Docs", color: "#ffc83a", desc: "the written word" },
  { id: "specs", label: "Specs", color: "#5fa8e6", desc: "the plan" },
  { id: "config", label: "Configs", color: "#34c7b0", desc: "the wiring" },
  { id: "audit", label: "Tests/Audit", color: "#f0743a", desc: "the checks" },
  { id: "claims", label: "Claims", color: "#b58cff", desc: "what someone said" },
];
export const LANE_IDX: Record<string, number> = Object.fromEntries(
  LANES.map((l, i) => [l.id, i]),
);
export const laneColor = (id: string): string => LANES[LANE_IDX[id]]?.color ?? "#cccccc";

/** Four trust tiers (rendered as node rings). */
export const TRUST: TrustTier[] = [
  {
    id: "DerivedDeterministic",
    label: "Derived — deterministic",
    short: "Derived",
    ring: "#cfe9b0",
    tip: "Rebuilt from git/markdown. Trusted by construction — no signature needed.",
  },
  {
    id: "HumanConfirmed",
    label: "Human-confirmed",
    short: "Confirmed",
    ring: "#ffd24a",
    tip: "A person reviewed and confirmed this. Load-bearing.",
  },
  {
    id: "AgentAsserted",
    label: "Agent-asserted",
    short: "Asserted",
    ring: "#9fc0e8",
    tip: "Signed by an agent or teammate. Authentic, but a claim — not the record.",
  },
  {
    id: "InferredAdvisory",
    label: "Inferred — advisory",
    short: "Proposed",
    ring: "#b58cff",
    tip: "An AI guess, quarantined. Advisory only until a human confirms it.",
  },
];
export const TRUST_IDX: Record<string, number> = Object.fromEntries(
  TRUST.map((t, i) => [t.id, i]),
);

/** Status enum (order matters for the filter rail). */
export const STATUS = ["Active", "Superseded", "Archived"] as const;

/** The 14 repo-tenants of the Citrate Federation (the design's sample Org). */
export const TENANTS: string[] = [
  "citrate-memories", "mem-gateway", "citrate-identity", "citrate-explorer",
  "lattice-vm", "mcp-orchestrator", "ghostdag-consensus", "citrate-inference",
  "salt-tokenomics", "belnap-logic", "federated-learning", "x402-precompiles",
  "citrate-dashboard", "citrate-docs",
];

export const MODELS: Model[] = [
  { id: "sonnet", name: "Claude Sonnet 4.6", provider: "Anthropic", kind: "in-app", cost: "$$", latency: "fast", note: "Best reasoning over the graph" },
  { id: "gpt", name: "GPT-5.1", provider: "OpenAI", kind: "in-app", cost: "$$$", latency: "medium", note: "Strong general recall" },
  { id: "citrate", name: "Citrate-LM", provider: "Citrate", kind: "on-prem", cost: "$", latency: "fast", note: "Runs inside your tenancy" },
  { id: "local", name: "Local · Llama 4", provider: "Self-hosted", kind: "BYOM", cost: "free", latency: "varies", note: "Connected over MCP" },
];

/** The signed-in user (placeholder until the auth seam populates it from OIDC). */
export const ME = { initials: "A", color: "#ffbd10", name: "Aleia Mercer", role: "Org Owner", title: "Head of Operations" };

export const NOTIFS: Notif[] = [
  { id: "nt0", icon: "branch", text: "3 proposals are awaiting your witness", sub: "Review Center · proposals", when: "12m ago", kind: "proposal" },
  { id: "nt1", icon: "shield", text: "Contradiction detected in citrate-memories", sub: "Belnap “Both” on a root anchor", when: "1h ago", kind: "contradiction" },
  { id: "nt2", icon: "check", text: "Ingestion finished for lattice-vm", sub: "+212 nodes · +148 edges", when: "3h ago", kind: "ingest" },
  { id: "nt3", icon: "spark", text: "Priya asserted a memory in citrate-docs", sub: "“Onboarding for non-technical teammates”", when: "5h ago", kind: "assert" },
];

export const SUGGESTIONS = [
  { label: "Why per-Org isolation?", q: "Why did we move to per-Org isolation?" },
  { label: "What's blocking the gateway?", q: "What's still blocking the gateway launch?" },
  { label: "Open security findings", q: "Show me the open security findings." },
  { label: "Recent in mem-gateway", q: "What changed in mem-gateway lately?" },
];

/** Timeline window (matches the scrubber ticks). */
export const T0 = Date.UTC(2025, 0, 1);
export const T1 = Date.UTC(2026, 5, 13);
export const span = T1 - T0;

export function fmtAgo(ms: number): string {
  const days = Math.max(0, Math.round((T1 - ms) / 86400000));
  if (days === 0) return "today";
  if (days < 7) return days + "d ago";
  if (days < 60) return Math.round(days / 7) + "w ago";
  return Math.round(days / 30) + "mo ago";
}
export function fmtDate(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/**
 * PLACEHOLDER counts shaped to the design's default screenshots. WP-7.2 replaces
 * these with live figures from the gateway (`/orgs/:org/tenants` + layout/stats).
 */
export const stats = { nodes: 749, edges: 1423, tenants: TENANTS.length };
export const reviewCount = 3;
