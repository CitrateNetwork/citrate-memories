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
import { decodeJwt } from "jose";

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
describe("PBA-L3c-032 — BYOM connect token", () => {
  const params = { params: Promise.resolve({ org: "citrate-federation" }) };
  const url = "http://app/api/orgs/citrate-federation/connect/token";

  it("mints scope `read` by default and binds the org", async () => {
    process.env.MEM_CONNECT_SECRET = "test-secret";
    const { POST } = await import("../../app/api/orgs/[org]/connect/token/route");
    const res = await POST(new Request(url, { method: "POST", headers: auth("did:citrate:mallory") }), params);
    expect(res.status).toBe(200);
    const claims = decodeJwt(((await res.json()) as { token: string }).token);
    expect(claims.scope, "PBA-L3c-032: default connect token must be read-only").toBe("read");
    expect(claims.org).toBe("citrate-federation");
    expect(claims.sub).toBe("did:citrate:mallory");
  });

  it("mints a write-capable (`read,propose`) token only on explicit opt-in", async () => {
    process.env.MEM_CONNECT_SECRET = "test-secret";
    const { POST } = await import("../../app/api/orgs/[org]/connect/token/route");
    const res = await POST(
      new Request(url, {
        method: "POST",
        headers: { ...auth("did:citrate:alice"), "content-type": "application/json" },
        body: JSON.stringify({ write: true }),
      }),
      params,
    );
    expect(res.status).toBe(200);
    const body = (await res.json()) as { token: string; scope: string };
    expect(decodeJwt(body.token).scope).toBe("read,propose");
    expect(body.scope).toBe("read,propose");
  });
});
