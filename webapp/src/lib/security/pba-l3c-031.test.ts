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
describe("PBA-L3c-031 — session login-CSRF", () => {
  const jwt = (payload: Record<string, unknown>) => {
    const seg = (o: unknown) => Buffer.from(JSON.stringify(o)).toString("base64url");
    return `${seg({ alg: "none" })}.${seg(payload)}.sig`;
  };
  const body = () => JSON.stringify({ id_token: jwt({ sub: "attacker", exp: 9999999999 }) });

  it("refuses a cross-origin POST (attacker page sets its session in the victim browser)", async () => {
    const { POST } = await import("../../app/api/auth/session/route");
    const res = await POST(
      new Request("https://memrizz.example/api/auth/session", {
        method: "POST",
        headers: { "content-type": "application/json", origin: "https://evil.example" },
        body: body(),
      }),
    );
    expect(res.status).toBe(403);
    expect(res.headers.get("set-cookie")).toBeNull();
  });

  it("refuses Sec-Fetch-Site: cross-site even without an Origin header", async () => {
    const { POST } = await import("../../app/api/auth/session/route");
    const res = await POST(
      new Request("https://memrizz.example/api/auth/session", {
        method: "POST",
        headers: { "content-type": "application/json", "sec-fetch-site": "cross-site" },
        body: body(),
      }),
    );
    expect(res.status).toBe(403);
  });

  it("refuses a text/plain body (the no-preflight form-CSRF shape)", async () => {
    const { POST } = await import("../../app/api/auth/session/route");
    const res = await POST(
      new Request("https://memrizz.example/api/auth/session", {
        method: "POST",
        headers: { "content-type": "text/plain" },
        body: body(),
      }),
    );
    expect(res.status).toBe(415);
    expect(res.headers.get("set-cookie")).toBeNull();
  });

  it("accepts the same-origin JSON POST from the callback page", async () => {
    const { POST } = await import("../../app/api/auth/session/route");
    const res = await POST(
      new Request("https://memrizz.example/api/auth/session", {
        method: "POST",
        headers: {
          "content-type": "application/json; charset=utf-8",
          origin: "https://memrizz.example",
          "sec-fetch-site": "same-origin",
        },
        body: body(),
      }),
    );
    expect(res.status).toBe(200);
  });
});
