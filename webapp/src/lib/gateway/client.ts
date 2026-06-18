/**
 * Server-only client for mem-gateway's HTTP/JSON read API. The webapp BFF (route
 * handlers in src/app/api/*) is the ONLY thing that calls this; the browser never
 * holds a gateway credential (planset §2).
 *
 * Auth to the gateway: the gateway today runs dev-auth (`x-dev-sub` header behind
 * MEM_GATEWAY_ALLOW_DEV_AUTH=1) and fails closed otherwise — real OIDC bearer
 * verification is gateway gap G-2 (planset §3.A). So this client forwards the
 * caller's already-verified OIDC `sub` as `x-dev-sub` by default. The seam to
 * switch to bearer-forwarding (once G-2 lands) is one branch: set
 * MEM_GATEWAY_FORWARD_BEARER=1 and pass the caller's token.
 *
 * Fail closed: gatewayOrigin() throws if MEM_GATEWAY_ORIGIN is unset, so a
 * misconfigured deploy errors loudly rather than silently calling localhost.
 */
import "server-only";
import type {
  AnalogyResult, AuditResult, NeighborsResult, OpsSnapshot, OrgSummary, RecallResult, Scene, VerifyResult,
} from "./types";

export class GatewayError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "GatewayError";
  }
}

function gatewayOrigin(): string {
  const o = process.env.MEM_GATEWAY_ORIGIN;
  if (!o) {
    throw new GatewayError(
      503,
      "MEM_GATEWAY_ORIGIN is not set — refusing to call the memory gateway (fail closed).",
    );
  }
  return o.replace(/\/$/, "");
}

export interface GatewayCaller {
  /** The verified OIDC subject (from the auth seam's requireOwner). */
  sub: string;
  /** The caller's bearer token, forwarded only when MEM_GATEWAY_FORWARD_BEARER=1. */
  token?: string | null;
}

/** Auth headers for a gateway call — bearer-forward (G-2) or dev-sub. */
function callerHeaders(caller: GatewayCaller): Record<string, string> {
  const headers: Record<string, string> = { accept: "application/json" };
  if (process.env.MEM_GATEWAY_FORWARD_BEARER === "1" && caller.token) {
    headers.authorization = `Bearer ${caller.token}`;
  } else {
    headers["x-dev-sub"] = caller.sub;
  }
  return headers;
}

async function gatewayGet<T>(
  path: string,
  caller: GatewayCaller,
  query?: Record<string, string | number | undefined>,
): Promise<T> {
  const url = new URL(gatewayOrigin() + path);
  if (query) {
    for (const [k, v] of Object.entries(query)) {
      if (v !== undefined && v !== "") url.searchParams.set(k, String(v));
    }
  }
  const res = await fetch(url, { headers: callerHeaders(caller), cache: "no-store" });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new GatewayError(res.status, body || `gateway ${res.status}`);
  }
  return (await res.json()) as T;
}

async function gatewayBody<T>(method: "POST" | "DELETE", path: string, caller: GatewayCaller, body?: unknown): Promise<T> {
  const res = await fetch(gatewayOrigin() + path, {
    method,
    headers: { ...callerHeaders(caller), ...(body !== undefined ? { "content-type": "application/json" } : {}) },
    body: body !== undefined ? JSON.stringify(body) : undefined,
    cache: "no-store",
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new GatewayError(res.status, text || `gateway ${res.status}`);
  }
  return (await res.json()) as T;
}
const gatewayPost = <T>(path: string, caller: GatewayCaller, body: unknown) => gatewayBody<T>("POST", path, caller, body);

/** POST a RAW (already-serialized) body — used for merge_diff, where the body is the diff JSON itself. */
async function gatewayPostRaw<T>(path: string, caller: GatewayCaller, raw: string): Promise<T> {
  const res = await fetch(gatewayOrigin() + path, {
    method: "POST",
    headers: { ...callerHeaders(caller), "content-type": "application/json" },
    body: raw,
    cache: "no-store",
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new GatewayError(res.status, text || `gateway ${res.status}`);
  }
  return (await res.json()) as T;
}

/** Liveness — no caller scope needed (the gateway's own health route). */
export async function health(): Promise<{ ok: boolean; service?: string }> {
  const res = await fetch(gatewayOrigin() + "/api/health", { cache: "no-store" });
  if (!res.ok) throw new GatewayError(res.status, `gateway health ${res.status}`);
  return (await res.json()) as { ok: boolean; service?: string };
}

const enc = encodeURIComponent;

export const gateway = {
  health,
  listOrgs: (c: GatewayCaller) =>
    gatewayGet<{ orgs: OrgSummary[] }>("/api/orgs", c),
  /** Create a new Org (initializes its isolated store + binds the caller as Owner). */
  createOrg: (c: GatewayCaller, body: { name: string; id?: string }) =>
    gatewayPost<{ id: string; name: string; status: string; store_path: string; owner: string }>("/api/orgs", c, body),
  listTenants: (c: GatewayCaller, org: string) =>
    gatewayGet<{ tenants: string[] }>(`/api/orgs/${enc(org)}/tenants`, c),
  layout: (c: GatewayCaller, org: string) =>
    gatewayGet<Scene>(`/api/orgs/${enc(org)}/layout`, c),
  recall: (c: GatewayCaller, org: string, tenant: string, budget?: number) =>
    gatewayGet<RecallResult>(`/api/orgs/${enc(org)}/tenants/${enc(tenant)}/recall`, c, { budget }),
  search: (c: GatewayCaller, org: string, tenant: string, q: string, budget?: number) =>
    gatewayGet<RecallResult>(`/api/orgs/${enc(org)}/tenants/${enc(tenant)}/search`, c, { q, budget }),
  asOf: (c: GatewayCaller, org: string, tenant: string, t: number, budget?: number) =>
    gatewayGet<RecallResult>(`/api/orgs/${enc(org)}/tenants/${enc(tenant)}/as_of`, c, { t, budget }),
  node: (c: GatewayCaller, org: string, id: string) =>
    gatewayGet<Record<string, unknown>>(`/api/orgs/${enc(org)}/nodes/${enc(id)}`, c),
  verify: (c: GatewayCaller, org: string, id: string) =>
    gatewayGet<VerifyResult>(`/api/orgs/${enc(org)}/nodes/${enc(id)}/verify`, c),
  neighbors: (c: GatewayCaller, org: string, id: string, budget?: number) =>
    gatewayGet<NeighborsResult>(`/api/orgs/${enc(org)}/nodes/${enc(id)}/neighbors`, c, { budget }),
  analogy: (c: GatewayCaller, org: string, repo: string, id: string, budget?: number) =>
    gatewayGet<AnalogyResult>(`/api/orgs/${enc(org)}/analogy`, c, { repo, id, budget }),

  // ---- write routes (gap G-1): write-scoped + audited at the gateway ----
  assert: (c: GatewayCaller, org: string, tenant: string, body: { content: string; kind?: string }) =>
    gatewayPost<{ id: string; kind: string; repo: string }>(`/api/orgs/${enc(org)}/tenants/${enc(tenant)}/assert`, c, body),
  proposeEdge: (c: GatewayCaller, org: string, body: { from: string; to: string; kind?: string; evidence?: string }) =>
    gatewayPost<{ from: string; to: string; kind: string; quarantined: boolean }>(`/api/orgs/${enc(org)}/edges/propose`, c, body),
  confirmEdge: (c: GatewayCaller, org: string, body: { from: string; to: string; kind: string }) =>
    gatewayPost<{ from: string; to: string; kind: string; status: string }>(`/api/orgs/${enc(org)}/edges/confirm`, c, body),
  /** Merge a signed memory-diff (the "git for agents" handoff). `diffJson` is the serialized MemoryDiff. */
  mergeDiff: (c: GatewayCaller, org: string, diffJson: string) =>
    gatewayPostRaw<{ nodes: number; edges: number; superseded: number }>(`/api/orgs/${enc(org)}/merge_diff`, c, diffJson),

  // ---- provisioning (gap G-4): the delegation tree ----
  listMembers: (c: GatewayCaller, org: string) =>
    gatewayGet<{ members: Member[] }>(`/api/orgs/${enc(org)}/members`, c),
  addMember: (c: GatewayCaller, org: string, body: { sub: string; role: string; scopes?: ScopeInput[] }) =>
    gatewayPost<{ sub: string; org: string; role: string; parent: string }>(`/api/orgs/${enc(org)}/members`, c, body),
  revokeMember: (c: GatewayCaller, org: string, sub: string) =>
    gatewayBody<{ revoked: string[] }>("DELETE", `/api/orgs/${enc(org)}/members/${enc(sub)}`, c),

  // ---- audit log (the tamper-evident chain) ----
  audit: (c: GatewayCaller, org: string, limit?: number) =>
    gatewayGet<AuditResult>(`/api/orgs/${enc(org)}/audit`, c, { limit }),

  // ---- ops / durability (WP-6.5) ----
  ops: (c: GatewayCaller, org: string) =>
    gatewayGet<OpsSnapshot>(`/api/orgs/${enc(org)}/ops`, c),
  createCheckpoint: (c: GatewayCaller, org: string, keep?: number) =>
    gatewayPost<{ created: string; count: number }>(`/api/orgs/${enc(org)}/ops/checkpoint${keep ? `?keep=${keep}` : ""}`, c, {}),
};

export interface ScopeInput { resource_id: string; can_read: boolean; can_write: boolean }
export interface Member { sub: string; role: string; parent: string | null; scopes: ScopeInput[] }
