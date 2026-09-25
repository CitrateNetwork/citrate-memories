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
});
