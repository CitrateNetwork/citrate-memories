/**
 * Rate limiter for the BFF (cloned from citrate-explorer, planset §3.B.3).
 *
 * Two backends behind one async entry point ({@link checkRateLimit}):
 *  1. **Distributed** — Upstash Redis REST (`UPSTASH_REDIS_REST_URL` + `_TOKEN`):
 *     a fixed-window counter (`burst` per `ceil(burst / perSec)` s, see
 *     {@link redisWindowSec}) shared across every instance/region.
 *  2. **In-memory token bucket** ({@link rateLimit}) — fallback when no store is
 *     configured (local dev). Per-instance, resets on cold start; best-effort.
 *
 * If the store is configured but errors, we DO NOT lock out — we fall back to the
 * in-memory bucket so a Redis blip can't take the API down.
 */
interface Bucket {
  tokens: number;
  last: number;
}

const buckets = new Map<string, Bucket>();

export interface RateResult {
  ok: boolean;
  retryAfter?: number;
  backend?: "redis" | "memory";
}

const defaultBurst = (perSec: number) => Math.max(perSec * 2, 5);

/** In-memory token bucket. `id` buckets per key or IP. Synchronous + local. */
export function rateLimit(id: string, perSec: number, burst = defaultBurst(perSec)): RateResult {
  const now = Date.now();
  let b = buckets.get(id);
  if (!b) {
    b = { tokens: burst, last: now };
    buckets.set(id, b);
  }
  b.tokens = Math.min(burst, b.tokens + ((now - b.last) / 1000) * perSec);
  b.last = now;
  if (b.tokens < 1) {
    return { ok: false, retryAfter: Math.ceil((1 - b.tokens) / Math.max(perSec, 0.1)), backend: "memory" };
  }
  b.tokens -= 1;
  return { ok: true, backend: "memory" };
}

const REDIS_URL = process.env.UPSTASH_REDIS_REST_URL;
const REDIS_TOKEN = process.env.UPSTASH_REDIS_REST_TOKEN;

export function isDistributed(): boolean {
  return Boolean(REDIS_URL && REDIS_TOKEN);
}

/**
 * PBA-L3c-012 (variant of the citrate-explorer fix): the Upstash window is sized
 * so its SUSTAINED rate matches the in-memory token bucket — `burst` requests per
 * `ceil(burst / perSec)` seconds. It used to be a 1-second window capped at
 * `burst`, i.e. `burst`/s: the BFF read limiter (12/s, burst 24) admitted 24/s and
 * the LLM limiter (0.2/s, burst 3) admitted 3/s — 15x the intended rate.
 */
export function redisWindowSec(perSec: number, burst: number): number {
  return Math.max(1, Math.ceil(burst / Math.max(perSec, 0.001)));
}

/**
 * Fixed-window counter in Upstash (`INCR` + `EXPIRE windowSec NX`), windows
 * aligned to epoch multiples of `windowSec`. A denial reports the seconds left in
 * the current window. Throws on any transport/store error so callers can degrade.
 */
async function redisFixedWindow(id: string, limit: number, windowSec: number): Promise<RateResult> {
  const nowMs = Date.now();
  const windowStart = Math.floor(nowMs / 1000 / windowSec);
  const key = `rl:${id}:${windowStart}`;
  const res = await fetch(`${REDIS_URL}/pipeline`, {
    method: "POST",
    headers: { authorization: `Bearer ${REDIS_TOKEN}`, "content-type": "application/json" },
    body: JSON.stringify([
      ["INCR", key],
      ["EXPIRE", key, windowSec, "NX"],
    ]),
    signal: AbortSignal.timeout(1500),
  });
  if (!res.ok) throw new Error(`upstash ${res.status}`);
  const body = (await res.json()) as Array<{ result?: number; error?: string }>;
  const count = body?.[0]?.result;
  if (typeof count !== "number") throw new Error("upstash malformed response");
  if (count > limit) {
    const windowEndMs = (windowStart + 1) * windowSec * 1000;
    return { ok: false, retryAfter: Math.max(1, Math.ceil((windowEndMs - nowMs) / 1000)), backend: "redis" };
  }
  return { ok: true, backend: "redis" };
}

export async function checkRateLimit(
  id: string,
  perSec: number,
  burst = defaultBurst(perSec),
): Promise<RateResult> {
  if (isDistributed()) {
    try {
      return await redisFixedWindow(id, burst, redisWindowSec(perSec, burst));
    } catch {
      // Store unreachable — degrade to the local bucket rather than lock out.
    }
  }
  return rateLimit(id, perSec, burst);
}

/**
 * The client IP, taken from the PLATFORM-TRUSTED right-most forwarded hop — never
 * the left-most (FUA-EXPLORER-02): the left-most XFF entry is attacker-controlled
 * and would let a caller forge a fresh limiter key per request. Vercel's
 * `x-vercel-forwarded-for` is already the trusted client IP; otherwise we take the
 * last `x-forwarded-for` hop the platform appended.
 */
export function clientIp(req: Request): string {
  const vercel = req.headers.get("x-vercel-forwarded-for");
  if (vercel) return vercel.trim();
  const xff = req.headers.get("x-forwarded-for");
  if (xff) {
    const hops = xff.split(",").map((s) => s.trim()).filter(Boolean);
    if (hops.length) return hops[hops.length - 1];
  }
  return req.headers.get("x-real-ip")?.trim() || "unknown";
}

const windows = new Map<string, { start: number; count: number }>();

/**
 * Fixed-window quota: at most `limit` calls per `windowSec` for `id` (e.g. an
 * hourly LLM budget per account, PBA-L3c-015). Distributed when Upstash is
 * configured, else a per-instance in-memory window (same degrade-not-lock-out
 * policy as {@link checkRateLimit}).
 */
export async function checkWindowLimit(id: string, limit: number, windowSec: number): Promise<RateResult> {
  if (isDistributed()) {
    try {
      return await redisFixedWindow(`w${windowSec}:${id}`, limit, windowSec);
    } catch {
      // Store unreachable — degrade to the local window rather than lock out.
    }
  }
  const start = Math.floor(Date.now() / 1000 / windowSec);
  let w = windows.get(id);
  if (!w || w.start !== start) {
    w = { start, count: 0 };
    windows.set(id, w);
  }
  if (w.count >= limit) {
    return { ok: false, retryAfter: (start + 1) * windowSec - Math.floor(Date.now() / 1000), backend: "memory" };
  }
  w.count += 1;
  return { ok: true, backend: "memory" };
}
