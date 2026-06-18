/**
 * Rate limiter + trusted client-IP extraction (FUA-EXPLORER-02). The IP that
 * keys the limiter MUST come from the platform-trusted right-most forwarded hop,
 * never the spoofable left-most one — otherwise a caller forges a fresh key per
 * request and the limit is meaningless.
 */
import { describe, expect, it } from "vitest";
import { clientIp, rateLimit } from "./ratelimit";

describe("clientIp — trusted right-most hop", () => {
  it("prefers x-vercel-forwarded-for (the platform-trusted client IP)", () => {
    const req = new Request("http://x", {
      headers: {
        "x-vercel-forwarded-for": "203.0.113.9",
        "x-forwarded-for": "1.2.3.4, 203.0.113.9",
      },
    });
    expect(clientIp(req)).toBe("203.0.113.9");
  });

  it("takes the RIGHT-most XFF hop, ignoring an attacker-prepended left value", () => {
    const req = new Request("http://x", {
      headers: { "x-forwarded-for": "9.9.9.9 (spoofed), 198.51.100.7" },
    });
    expect(clientIp(req)).toBe("198.51.100.7");
  });

  it("falls back to 'unknown' with no forwarding headers", () => {
    expect(clientIp(new Request("http://x"))).toBe("unknown");
  });
});

describe("rateLimit — in-memory token bucket", () => {
  it("admits up to the burst, then sheds with a retryAfter", () => {
    const id = "test-key-" + Math.random();
    const burst = 5;
    let admitted = 0;
    let shed: ReturnType<typeof rateLimit> | null = null;
    for (let i = 0; i < burst + 3; i++) {
      const r = rateLimit(id, 1, burst);
      if (r.ok) admitted++;
      else shed = r;
    }
    expect(admitted).toBe(burst);
    expect(shed?.ok).toBe(false);
    expect(shed?.retryAfter).toBeGreaterThanOrEqual(1);
  });

  it("keys independently so two callers don't share a bucket", () => {
    expect(rateLimit("a-" + Math.random(), 1, 1).ok).toBe(true);
    expect(rateLimit("b-" + Math.random(), 1, 1).ok).toBe(true);
  });
});
