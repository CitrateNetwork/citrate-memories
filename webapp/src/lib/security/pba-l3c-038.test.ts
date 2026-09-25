/**
 * PBA R2 regression tests (2026-09-24 pre-bounty adversarial audit, lane L3c,
 * Memrizz items). Each drives the real route handler / client export.
 *
 *  - PBA-L3c-015  /api/chat: dedicated LLM budget + bounded, sanitized history
 *  - PBA-L3c-031  /api/auth/session: login-CSRF (Origin / Sec-Fetch-Site / JSON)
 *  - PBA-L3c-032  BYOM connect token: read-only by default, org-bound
 *  - PBA-L3c-038  dot-segment path params, gateway error pass-through, health echo
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockToken = (claims: Record<string, unknown>) => Buffer.from(JSON.stringify(claims)).toString("base64url");
const auth = (sub: string) => ({ authorization: `Bearer ${mockToken({ sub })}` });

const savedEnv = { ...process.env };
beforeEach(() => {
  process.env.NEXT_PUBLIC_AUTH_MODE = "mock";
  delete process.env.MEM_GATEWAY_ORIGIN;
  delete process.env.UPSTASH_REDIS_REST_URL;
  delete process.env.UPSTASH_REDIS_REST_TOKEN;
  vi.resetModules();
});
afterEach(() => {
  process.env = { ...savedEnv };
});

// ---------------------------------------------------------------------------
describe("PBA-L3c-038 — path segments, error pass-through, health echo", () => {
  it("refuses dot-segment path params instead of letting them normalize in the gateway URL", async () => {
    process.env.MEM_GATEWAY_ORIGIN = "http://127.0.0.1:9";
    const { gateway, GatewayError } = await import("../gateway/client");
    for (const bad of ["..", ".", ""]) {
      // Called the way every BFF handler calls it: inside an async function.
      const err = await (async () => gateway.recall({ sub: "u", token: "t" }, "citrate-federation", bad))().catch((e) => e);
      expect(err, `segment ${JSON.stringify(bad)}`).toBeInstanceOf(GatewayError);
      expect((err as InstanceType<typeof GatewayError>).status).toBe(400);
    }
  });

  it("does not pass a gateway 5xx body through to the browser", async () => {
    const { withCaller } = await import("../gateway/bff");
    const { GatewayError } = await import("../gateway/client");
    const res = await withCaller(new Request("http://app/x", { headers: auth("did:citrate:u") }), async () => {
      throw new GatewayError(500, '{"error":"internal: rocksdb /srv/mem/store.db: IO error"}');
    });
    expect(res.status).toBe(502);
    expect(await res.text()).not.toContain("/srv/mem");
  });

  it("passes only the gateway's own `error` string for a 4xx", async () => {
    const { withCaller } = await import("../gateway/bff");
    const { GatewayError } = await import("../gateway/client");
    const res = await withCaller(new Request("http://app/x", { headers: auth("did:citrate:u") }), async () => {
      throw new GatewayError(403, '{"error":"not authorized for resource"}');
    });
    expect(res.status).toBe(403);
    expect(((await res.json()) as { error: string }).error).toBe("not authorized for resource");
  });

  it("health does not echo configuration errors", async () => {
    const { GET } = await import("../../app/api/health/route");
    const res = await GET();
    expect(res.status).toBe(503);
    const text = await res.text();
    expect(text).not.toContain("MEM_GATEWAY_ORIGIN");
    expect(JSON.parse(text).ok).toBe(false);
  });

  it("mutation-hardening: status mapping and message shaping", async () => {
    const { publicGatewayMessage } = await import("../gateway/bff");
    const { GatewayError } = await import("../gateway/client");
    const G = (s: number, m: string) => new GatewayError(s, m);
    expect(publicGatewayMessage(G(400, '{"error":"repo query param required"}'))).toBe("repo query param required");
    expect(publicGatewayMessage(G(404, '{"error":"' + "x".repeat(300) + '"}')).length, "trimmed to 200").toBe(200);
    expect(publicGatewayMessage(G(404, "<html>nginx</html>"))).toBe("gateway request failed (404)");
    expect(publicGatewayMessage(G(404, '{"detail":"x"}'))).toBe("gateway request failed (404)");
    expect(publicGatewayMessage(G(400, "invalid path segment"))).toBe("invalid path segment");
    expect(publicGatewayMessage(G(400, "<html>bad request</html>"))).toBe("gateway request failed (400)");
    expect(publicGatewayMessage(G(404, "invalid path segment"))).toBe("gateway request failed (404)");
    expect(publicGatewayMessage(G(503, "MEM_GATEWAY_ORIGIN is not set"))).toBe("gateway unavailable");
    expect(publicGatewayMessage(G(500, '{"error":"internal: x"}'))).toBe("gateway request failed");
    expect(publicGatewayMessage(G(302, '{"error":"moved"}'))).toBe("gateway request failed");
  });

  it("mutation-hardening: a refused path segment surfaces as a 400 through the BFF", async () => {
    process.env.MEM_GATEWAY_ORIGIN = "http://127.0.0.1:9";
    const { withCaller } = await import("../gateway/bff");
    const { gateway } = await import("../gateway/client");
    const res = await withCaller(new Request("http://app/x", { headers: auth("did:citrate:u") }), (c) =>
      gateway.recall(c, "citrate-federation", ".."),
    );
    expect(res.status).toBe(400);
    expect(((await res.json()) as { error: string }).error).toBe("invalid path segment");
  });

  it("mutation-hardening: health passes the gateway's liveness through when up", async () => {
    const { createServer } = await import("node:http");
    const srv = createServer((_q, res) => {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ ok: true, service: "mem-gateway" }));
    });
    await new Promise<void>((r) => srv.listen(0, "127.0.0.1", r));
    process.env.MEM_GATEWAY_ORIGIN = `http://127.0.0.1:${(srv.address() as { port: number }).port}`;
    try {
      const { GET } = await import("../../app/api/health/route");
      const res = await GET();
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({ ok: true, gateway: { ok: true, service: "mem-gateway" } });
    } finally {
      srv.close();
    }
  });

  it("mutation-hardening: health reports a fixed message", async () => {
    const { GET } = await import("../../app/api/health/route");
    expect(await (await GET()).json()).toEqual({ ok: false, error: "gateway unavailable" });
  });
});
