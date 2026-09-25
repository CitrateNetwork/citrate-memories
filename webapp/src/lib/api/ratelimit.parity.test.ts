/**
 * PBA-L3c-012 variant tripwire (Memrizz BFF): the distributed (Upstash) limiter
 * must enforce the SAME sustained rate as the in-memory token bucket. It used a
 * 1-second window capped at `burst`, so production admitted `burst`/s — the read
 * limiter (12/s, burst 24) ran at 24/s and the LLM limiter (0.2/s, burst 3) at 3/s.
 *
 * Parity rule: over any horizon T of continuous hammering the Redis backend admits
 * at most what the token bucket admits (burst + perSec*T) plus one extra window of
 * burst (fixed-window edge effect), and at least half the sustained rate.
 * Mirrors citrate-explorer 6daa4cc.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.resetModules();
});

/** In-process Upstash REST fake: honours INCR + EXPIRE NX with TTLs. */
function fakeUpstash() {
  const store = new Map<string, { n: number; exp: number }>();
  const calls: unknown[][][] = [];
  const fn = vi.fn(async (url: string, init: RequestInit) => {
    expect(url).toBe("https://upstash.invalid/pipeline");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer t");
    expect((init.headers as Record<string, string>)["content-type"]).toBe("application/json");
    const cmds = JSON.parse(String(init.body)) as unknown[][];
    calls.push(cmds);
    const now = Date.now();
    const out: Array<{ result: number }> = [];
    for (const c of cmds) {
      const key = String(c[1]);
      let e = store.get(key);
      if (e && e.exp <= now) {
        store.delete(key);
        e = undefined;
      }
      if (c[0] === "INCR") {
        const n = (e?.n ?? 0) + 1;
        store.set(key, { n, exp: e?.exp ?? Infinity });
        out.push({ result: n });
      } else if (c[0] === "EXPIRE") {
        expect(c[3]).toBe("NX");
        const cur = store.get(key);
        if (cur && cur.exp === Infinity) cur.exp = now + Number(c[2]) * 1000;
        out.push({ result: 1 });
      } else {
        throw new Error(`unexpected command ${String(c[0])}`);
      }
    }
    return new Response(JSON.stringify(out), { status: 200 });
  });
  return { fn, calls };
}

async function hammer(backend: "redis" | "memory", perSec: number, burst: number, seconds: number, stepMs = 100) {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-24T00:00:00.300Z"));
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  if (backend === "redis") {
    vi.stubEnv("UPSTASH_REDIS_REST_URL", "https://upstash.invalid");
    vi.stubEnv("UPSTASH_REDIS_REST_TOKEN", "t");
    vi.stubGlobal("fetch", fakeUpstash().fn);
  }
  vi.resetModules();
  const { checkRateLimit } = await import("./ratelimit");
  const id = `parity:${backend}:${perSec}:${burst}:${Math.random()}`;
  let ok = 0;
  for (let t = 0; t < seconds * 1000; t += stepMs) {
    const r = await checkRateLimit(id, perSec, burst);
    expect(r.backend).toBe(backend);
    if (r.ok) ok++;
    vi.setSystemTime(Date.now() + stepMs);
  }
  return ok;
}

describe("Upstash limiter parity with the token bucket (PBA-L3c-012 variant)", () => {
  // BFF read (12/s, burst 24), LLM (0.2/s, burst 3), default burst, fractional.
  const cases: Array<[number, number]> = [
    [12, 24],
    [0.2, 3],
    [2, 5],
    [1, 2],
    [0.5, 3],
  ];

  for (const [perSec, burst] of cases) {
    it(`perSec=${perSec} burst=${burst}: redis admits no more than the bucket (+1 window) over 30s`, async () => {
      const T = 30;
      const step = perSec >= 10 ? 20 : 100;
      const redis = await hammer("redis", perSec, burst, T, step);
      const memory = await hammer("memory", perSec, burst, T, step);
      const ceiling = burst + perSec * T + burst;
      expect(redis).toBeLessThanOrEqual(ceiling);
      expect(memory).toBeLessThanOrEqual(ceiling);
      expect(redis).toBeGreaterThanOrEqual(Math.floor(perSec * T * 0.5));
    });
  }

  it("the LLM limiter (0.2/s, burst 3) admits at most 4 in 10s via redis (was 3/s = 30)", async () => {
    expect(await hammer("redis", 0.2, 3, 10, 200)).toBeLessThanOrEqual(4);
  });

  it("window sizing: ceil(burst/perSec) seconds, at least 1", async () => {
    const { redisWindowSec } = await import("./ratelimit");
    expect(redisWindowSec(12, 24)).toBe(2);
    expect(redisWindowSec(0.2, 3)).toBe(15);
    expect(redisWindowSec(0.3, 1)).toBe(4);
    expect(redisWindowSec(100, 5)).toBe(1);
    expect(redisWindowSec(0, 5)).toBe(5000);
  });

  it("a denied redis request reports the seconds left in its window; key and TTL use the window", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-24T00:00:00.000Z"));
    vi.stubEnv("UPSTASH_REDIS_REST_URL", "https://upstash.invalid");
    vi.stubEnv("UPSTASH_REDIS_REST_TOKEN", "t");
    const { fn, calls } = fakeUpstash();
    vi.stubGlobal("fetch", fn);
    vi.resetModules();
    const { checkRateLimit } = await import("./ratelimit");
    // perSec 0.2, burst 3 → a 15 s window.
    for (let i = 0; i < 3; i++) expect((await checkRateLimit("win:1", 0.2, 3)).ok).toBe(true);
    vi.setSystemTime(Date.now() + 10_400);
    expect(await checkRateLimit("win:1", 0.2, 3)).toEqual({ ok: false, retryAfter: 5, backend: "redis" });
    const [incr, expire] = calls[0];
    expect(incr[0]).toBe("INCR");
    expect(String(incr[1])).toMatch(/^rl:win:1:\d+$/);
    expect(expire).toEqual(["EXPIRE", incr[1], 15, "NX"]);
    // Next window admits again.
    vi.setSystemTime(Date.now() + 5_000);
    expect((await checkRateLimit("win:1", 0.2, 3)).ok).toBe(true);
  });

  it("a store error or malformed reply degrades to the in-memory bucket (never locks out, never trusts garbage)", async () => {
    vi.stubEnv("UPSTASH_REDIS_REST_URL", "https://upstash.invalid");
    vi.stubEnv("UPSTASH_REDIS_REST_TOKEN", "t");
    for (const reply of [
      new Response(JSON.stringify([{ result: 1 }, { result: 1 }]), { status: 500 }),
      new Response(JSON.stringify([{ result: "1" }, { result: 1 }]), { status: 200 }),
      new Response(JSON.stringify([{ error: "ERR" }]), { status: 200 }),
    ]) {
      vi.stubGlobal("fetch", async () => reply);
      vi.resetModules();
      const { checkRateLimit } = await import("./ratelimit");
      expect(await checkRateLimit(`deg:${Math.random()}`, 1, 2)).toEqual({ ok: true, backend: "memory" });
    }
  });
});
