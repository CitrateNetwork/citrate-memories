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

  it("mutation-hardening: Sec-Fetch-Site none (user navigation) and configured origins are accepted", async () => {
    process.env.MEMRIZZ_APP_ORIGINS = " https://app.memrizz.example/ , https://alt.memrizz.example";
    const { POST } = await import("../../app/api/auth/session/route");
    const mk = (headers: Record<string, string>) =>
      POST(new Request("https://internal-host/api/auth/session", { method: "POST", headers: { "content-type": "application/json", ...headers }, body: body() }));
    expect((await mk({ "sec-fetch-site": "none" })).status).toBe(200);
    expect((await mk({ origin: "https://app.memrizz.example" })).status, "trailing slash + spaces trimmed").toBe(200);
    expect((await mk({ origin: "https://alt.memrizz.example" })).status).toBe(200);
    expect((await mk({ origin: "https://evil.example" })).status).toBe(403);
    expect((await mk({ "sec-fetch-site": "same-site" })).status, "same-SITE is not same-origin").toBe(403);
    expect((await mk({ "content-type": "Application/JSON" })).status, "media type is case-insensitive").toBe(200);
  });

  it("mutation-hardening: guard verdicts carry the right status and message", async () => {
    const { checkSameOriginJson } = await import("./same-origin");
    const r = (h: Record<string, string>) => checkSameOriginJson(new Request("https://a.example/x", { method: "POST", headers: h }));
    expect(r({ "sec-fetch-site": "cross-site", "content-type": "application/json" })).toEqual({ ok: false, status: 403, error: "cross-site request refused" });
    expect(r({ origin: "https://b.example", "content-type": "application/json" })).toEqual({ ok: false, status: 403, error: "cross-origin request refused" });
    expect(r({ "content-type": "text/plain" })).toEqual({ ok: false, status: 415, error: "content-type must be application/json" });
    expect(r({})).toEqual({ ok: false, status: 415, error: "content-type must be application/json" });
    expect(r({ origin: "https://a.example", "content-type": "application/json" })).toEqual({ ok: true });
  });
});
