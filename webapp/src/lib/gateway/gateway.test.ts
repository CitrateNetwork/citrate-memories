/**
 * BFF ⇄ mem-gateway wiring (WP-7.2). Hermetic: a loopback HTTP server stands in
 * for the Rust gateway, returning its real JSON shapes. Proves the client builds
 * correct URLs, forwards the verified bearer by default (MEM-B-013), parses typed
 * responses, FAILS CLOSED when MEM_GATEWAY_ORIGIN is unset, and that the
 * authenticated route handler 401s without a session and proxies with one.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

let server: Server;
let origin: string;
let lastDevSub: string | null = null;
let lastAuth: string | null = null;
let lastBody: string | null = null;

function send(res: ServerResponse, status: number, body: unknown) {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(body));
}

beforeAll(async () => {
  server = createServer((req: IncomingMessage, res: ServerResponse) => {
    lastDevSub = (req.headers["x-dev-sub"] as string) ?? null;
    lastAuth = (req.headers["authorization"] as string) ?? null;
    const url = new URL(req.url ?? "/", "http://x");
    const p = url.pathname;
    // write route (gap G-1)
    if (req.method === "POST" && p === "/api/orgs/citrate-federation/tenants/mem-gateway/assert") {
      let raw = "";
      req.on("data", (c) => (raw += c)).on("end", () => {
        lastBody = raw;
        send(res, 200, { id: "newnode01", kind: "Rationale", repo: "mem-gateway" });
      });
      return;
    }
    if (p === "/api/health") return send(res, 200, { ok: true, service: "mem-gateway" });
    if (req.method === "POST" && p === "/api/orgs") {
      let raw = "";
      req.on("data", (c) => (raw += c)).on("end", () => {
        lastBody = raw;
        const b = JSON.parse(raw || "{}");
        send(res, 200, { id: "acme-research", name: b.name, status: "Active", store_path: "data/orgs/acme-research.memdag", owner: "u" });
      });
      return;
    }
    if (p === "/api/orgs")
      return send(res, 200, { orgs: [{ id: "citrate-federation", name: "Citrate Federation", status: "Active" }] });
    if (p === "/api/orgs/citrate-federation/layout")
      return send(res, 200, {
        nodes: [{ id: "abc123", pos: [1, 2, 3], projected: true, repo: "mem-gateway", kind: "Adr", lane: "specs", plane: "derived", trust: "DerivedDeterministic", status: "active", contradicted: false, degree: 4, title: "per-Org isolation" }],
        edges: [{ from: "abc123", to: "def456", kind: "DependsOn", quarantined: false }],
      });
    if (p === "/api/orgs/citrate-federation/tenants")
      return send(res, 200, { tenants: ["mem-gateway"] });
    // search 503 (no bge index) → /api/chat falls back to recall
    if (p === "/api/orgs/citrate-federation/tenants/mem-gateway/search")
      return send(res, 503, "no embedder");
    if (p === "/api/orgs/citrate-federation/tenants/mem-gateway/recall")
      return send(res, 200, {
        repo: "mem-gateway",
        total_in_tenant: 42,
        watermark: null,
        items: [{ id: "node-isolation", kind: "Adr", lane: "specs", repo: "mem-gateway", title: "per-Org isolation", plane: "derived", trust: "DerivedDeterministic", status: "active", valid_from: 1, score: 0.9 }],
      });
    // provisioning routes (gap G-4)
    if (req.method === "GET" && p === "/api/orgs/citrate-federation/members")
      return send(res, 200, { members: [{ sub: "u", role: "OrgOwner", parent: null, scopes: [] }] });
    if (req.method === "POST" && p === "/api/orgs/citrate-federation/members") {
      let raw = "";
      req.on("data", (c) => (raw += c)).on("end", () => {
        lastBody = raw;
        const b = JSON.parse(raw || "{}");
        send(res, 200, { sub: b.sub, org: "citrate-federation", role: b.role, parent: "u" });
      });
      return;
    }
    if (req.method === "DELETE" && p === "/api/orgs/citrate-federation/members/newbie")
      return send(res, 200, { revoked: ["newbie"] });
    if (req.method === "GET" && p === "/api/orgs/citrate-federation/ops")
      return send(res, 200, { org: "citrate-federation", store: { nodes: 749, edges: 1423 }, checkpoints: { count: 3, latest_ms: 1, dir: "data/orgs/x.memdag.checkpoints" }, tenants: [{ repo: "mem-gateway", anchor: { root: "abc", node_count: 10, edge_count: 5, anchored_at_ms: 1 }, anchor_valid: true, live_root: "abc" }], chain_anchor: null });
    if (req.method === "POST" && p === "/api/orgs/citrate-federation/ops/checkpoint")
      return send(res, 200, { created: "ckpt-00000000000000000002", count: 3 });
    if (req.method === "GET" && p === "/api/orgs/citrate-federation/audit")
      return send(res, 200, { intact: true, length: 2, verified_through: 2, records: [{ seq: 2, ts: 1, event: "Write", actor: "u", resource_id: "repo:a/memory", detail: "assert x" }] });
    if (req.method === "POST" && p === "/api/orgs/citrate-federation/merge_diff") {
      let raw = "";
      req.on("data", (c) => (raw += c)).on("end", () => { lastBody = raw; send(res, 200, { nodes: 2, edges: 1, superseded: 0 }); });
      return;
    }
    if (req.method === "POST" && p === "/api/orgs/citrate-federation/edges/confirm") {
      let raw = "";
      req.on("data", (c) => (raw += c)).on("end", () => {
        lastBody = raw;
        send(res, 200, { from: "a", to: "b", kind: "AnalogousTo", status: "confirmed" });
      });
      return;
    }
    if (p.startsWith("/api/orgs/forbidden")) return send(res, 403, "not a member");
    return send(res, 404, "no route");
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  process.env.MEM_GATEWAY_ORIGIN = origin;
});

afterAll(async () => {
  await new Promise<void>((r) => server.close(() => r()));
});

describe("gateway client", () => {
  it("health() parses the gateway liveness shape", async () => {
    const { gateway } = await import("./client");
    expect(await gateway.health()).toEqual({ ok: true, service: "mem-gateway" });
  });

  // MEM-B-013: bearer-forward is the SECURE default. The caller's verified OIDC
  // bearer is forwarded; the unauthenticated x-dev-sub impersonation header is
  // NOT sent unless explicitly opted into via MEM_GATEWAY_DEV_SUB=1.
  it("forwards the verified bearer by default and never x-dev-sub", async () => {
    delete process.env.MEM_GATEWAY_DEV_SUB;
    lastDevSub = null;
    lastAuth = null;
    const { gateway } = await import("./client");
    const r = await gateway.listOrgs({ sub: "did:citrate:aleia", token: "tok-123" });
    expect(lastAuth).toBe("Bearer tok-123");
    expect(lastDevSub).toBeNull();
    expect(r.orgs[0].name).toBe("Citrate Federation");
  });

  it("uses x-dev-sub ONLY when MEM_GATEWAY_DEV_SUB=1 is set", async () => {
    process.env.MEM_GATEWAY_DEV_SUB = "1";
    lastDevSub = null;
    lastAuth = null;
    try {
      const { gateway } = await import("./client");
      await gateway.listOrgs({ sub: "did:citrate:aleia" });
      expect(lastDevSub).toBe("did:citrate:aleia");
      expect(lastAuth).toBeNull();
    } finally {
      delete process.env.MEM_GATEWAY_DEV_SUB;
    }
  });

  it("parses the constellation scene (nodes + edges)", async () => {
    const { gateway } = await import("./client");
    const scene = await gateway.layout({ sub: "s" }, "citrate-federation");
    expect(scene.nodes[0].pos).toEqual([1, 2, 3]);
    expect(scene.nodes[0].trust).toBe("DerivedDeterministic");
    expect(scene.edges[0].kind).toBe("DependsOn");
  });

  it("surfaces a gateway 403 as a GatewayError with status", async () => {
    const { gateway, GatewayError } = await import("./client");
    await expect(gateway.listTenants({ sub: "s" }, "forbidden")).rejects.toMatchObject({
      name: "GatewayError",
    });
    await expect(gateway.listTenants({ sub: "s" }, "forbidden")).rejects.toBeInstanceOf(GatewayError);
  });

  it("assert() POSTs the body and forwards the verified bearer (gap G-1)", async () => {
    delete process.env.MEM_GATEWAY_DEV_SUB;
    lastDevSub = null;
    lastAuth = null;
    const { gateway } = await import("./client");
    const r = await gateway.assert(
      { sub: "did:citrate:aleia", token: "tok-abc" },
      "citrate-federation",
      "mem-gateway",
      { content: "we chose per-Org isolation", kind: "rationale" },
    );
    expect(r.id).toBe("newnode01");
    expect(lastAuth).toBe("Bearer tok-abc");
    expect(lastDevSub).toBeNull();
    expect(JSON.parse(lastBody ?? "{}").content).toBe("we chose per-Org isolation");
  });

  it("confirmEdge() POSTs the edge to the write route (WP-7.8 → G-1)", async () => {
    const { gateway } = await import("./client");
    const r = await gateway.confirmEdge({ sub: "u" }, "citrate-federation", { from: "a", to: "b", kind: "AnalogousTo" });
    expect(r.status).toBe("confirmed");
    expect(JSON.parse(lastBody ?? "{}").kind).toBe("AnalogousTo");
  });

  it("createOrg() POSTs the name and returns the provisioned Org", async () => {
    const { gateway } = await import("./client");
    const o = await gateway.createOrg({ sub: "u" }, { name: "Acme Research" });
    expect(o.id).toBe("acme-research");
    expect(o.status).toBe("Active");
    expect(JSON.parse(lastBody ?? "{}").name).toBe("Acme Research");
  });

  it("member provisioning (gap G-4): list / onboard / cascade-revoke", async () => {
    const { gateway } = await import("./client");
    const c = { sub: "u" };
    const roster = await gateway.listMembers(c, "citrate-federation");
    expect(roster.members[0].role).toBe("OrgOwner");

    const added = await gateway.addMember(c, "citrate-federation", { sub: "newbie", role: "member", scopes: [{ resource_id: "repo:a/memory", can_read: true, can_write: false }] });
    expect(added.sub).toBe("newbie");
    expect(added.parent).toBe("u");
    expect(JSON.parse(lastBody ?? "{}").role).toBe("member");

    const revoked = await gateway.revokeMember(c, "citrate-federation", "newbie");
    expect(revoked.revoked).toContain("newbie");
  });

  it("mergeDiff() POSTs the raw diff JSON and returns the merge report", async () => {
    const { gateway } = await import("./client");
    const diff = JSON.stringify({ author: "u", created_at_ms: 1, nodes: [], edges: [] });
    const r = await gateway.mergeDiff({ sub: "u" }, "citrate-federation", diff);
    expect(r.nodes).toBe(2);
    expect(lastBody).toBe(diff);
  });

  it("ops() returns the durability snapshot; createCheckpoint() POSTs", async () => {
    const { gateway } = await import("./client");
    const o = await gateway.ops({ sub: "u" }, "citrate-federation");
    expect(o.store.nodes).toBe(749);
    expect(o.checkpoints.count).toBe(3);
    expect(o.tenants[0].anchor_valid).toBe(true);
    const cp = await gateway.createCheckpoint({ sub: "u" }, "citrate-federation");
    expect(cp.count).toBe(3);
  });

  it("audit() returns the integrity verdict + records", async () => {
    const { gateway } = await import("./client");
    const a = await gateway.audit({ sub: "u" }, "citrate-federation");
    expect(a.intact).toBe(true);
    expect(a.length).toBe(2);
    expect(a.records[0].event).toBe("Write");
  });

  it("FAILS CLOSED when MEM_GATEWAY_ORIGIN is unset", async () => {
    const saved = process.env.MEM_GATEWAY_ORIGIN;
    delete process.env.MEM_GATEWAY_ORIGIN;
    try {
      const { gateway, GatewayError } = await import("./client");
      await expect(gateway.health()).rejects.toBeInstanceOf(GatewayError);
      await expect(gateway.health()).rejects.toMatchObject({ status: 503 });
    } finally {
      process.env.MEM_GATEWAY_ORIGIN = saved;
    }
  });
});

describe("BFF route handler — auth gate", () => {
  async function loadOrgsRoute() {
    process.env.NEXT_PUBLIC_AUTH_MODE = "mock";
    vi.resetModules();
    return import("../../app/api/orgs/route");
  }
  function mockToken(claims: Record<string, unknown>): string {
    return Buffer.from(JSON.stringify(claims)).toString("base64url");
  }

  it("401s without a session", async () => {
    const { GET } = await loadOrgsRoute();
    const res = await GET(new Request("http://app/api/orgs"));
    expect(res.status).toBe(401);
  });

  it("proxies to the gateway with a valid session and returns Org data", async () => {
    const { GET } = await loadOrgsRoute();
    const req = new Request("http://app/api/orgs", {
      headers: { authorization: `Bearer ${mockToken({ sub: "did:citrate:aleia" })}` },
    });
    const res = await GET(req);
    expect(res.status).toBe(200);
    const body = (await res.json()) as { orgs: { name: string }[] };
    expect(body.orgs[0].name).toBe("Citrate Federation");
    // MEM-B-013: the BFF forwards the caller's verified bearer, not x-dev-sub.
    expect(lastAuth).toMatch(/^Bearer /);
    expect(lastDevSub).toBeNull();
  });

  it("BYOM connect token (G-7 client): mints a scoped token, 503 without a secret", async () => {
    process.env.NEXT_PUBLIC_AUTH_MODE = "mock";
    const token = mockToken({ sub: "did:citrate:aleia" });
    const params = { params: Promise.resolve({ org: "citrate-federation" }) };

    // configured → mints a token reflecting readable tenants
    process.env.MEM_CONNECT_SECRET = "test-secret";
    vi.resetModules();
    let mod = await import("../../app/api/orgs/[org]/connect/token/route");
    const ok = await mod.POST(new Request("http://app/api/orgs/citrate-federation/connect/token", { method: "POST", headers: { authorization: `Bearer ${token}` } }), params);
    expect(ok.status).toBe(200);
    const body = (await ok.json()) as { token: string; endpoint: string; tenants: string[] };
    expect(body.token.split(".").length).toBe(3); // a JWT
    expect(body.tenants).toContain("mem-gateway");
    expect(body.endpoint).toContain("/u/");

    // no session → 401
    const unauth = await mod.POST(new Request("http://app/api/orgs/citrate-federation/connect/token", { method: "POST" }), params);
    expect(unauth.status).toBe(401);

    // unconfigured → 503
    delete process.env.MEM_CONNECT_SECRET;
    vi.resetModules();
    mod = await import("../../app/api/orgs/[org]/connect/token/route");
    const off = await mod.POST(new Request("http://app/api/orgs/citrate-federation/connect/token", { method: "POST", headers: { authorization: `Bearer ${token}` } }), params);
    expect(off.status).toBe(503);
  });

  it("grounded Ask (WP-7.6): streams a UI-message response carrying the citations", async () => {
    process.env.NEXT_PUBLIC_AUTH_MODE = "mock";
    vi.resetModules();
    const { POST } = await import("../../app/api/chat/route");
    // no session → 401
    const unauth = await POST(new Request("http://app/api/chat", { method: "POST", body: JSON.stringify({ messages: [] }) }));
    expect(unauth.status).toBe(401);
    // with session: search 503 → recall fallback → grounded; citations ride in the stream
    const req = new Request("http://app/api/chat", {
      method: "POST",
      headers: { authorization: `Bearer ${mockToken({ sub: "did:citrate:aleia" })}`, "content-type": "application/json" },
      body: JSON.stringify({
        messages: [{ id: "1", role: "user", parts: [{ type: "text", text: "why per-org isolation?" }] }],
        org: "citrate-federation",
      }),
    });
    const res = await POST(req);
    expect(res.status).toBe(200);
    const streamed = await res.text();
    // the data-citations part + a grounded text part are serialized into the UI-message stream
    expect(streamed).toContain("node-isolation");
    expect(streamed.toLowerCase()).toContain("grounded");
  });
});
