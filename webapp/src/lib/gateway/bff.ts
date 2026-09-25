/**
 * BFF helper — the bridge between the auth seam and the gateway client. Every
 * read route uses `withCaller`: it runs `requireOwner` (fail closed → 401 on no
 * session), builds the GatewayCaller from the verified OIDC `sub` (+ the bearer
 * for forward-bearer mode), invokes the handler, and maps GatewayError to the
 * right HTTP status. No route talks to the gateway without passing through here.
 */
import "server-only";
import { requireOwner, tokenFromRequest } from "@/lib/auth/session";
import { checkRateLimit, clientIp } from "@/lib/api/ratelimit";
import { GatewayError, type GatewayCaller } from "./client";

function json(data: unknown, status = 200, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

/** Sustained reads/sec per (account, IP) before the BFF sheds load (planset §3.B.3). */
const READ_RATE_PER_SEC = Number(process.env.MEM_BFF_RATE_PER_SEC) || 12;

/**
 * Auth + rate-limit gate for routes that act on the caller's OWN data (the DB —
 * conversations, etc.), where there's no gateway call to forward. Hands the
 * verified OIDC `sub` to `fn`; every owner-scoped query keys on it. DB errors
 * (incl. unset DATABASE_URL) surface as 503 (persistence unavailable), never a
 * silent success.
 */
export async function withOwner<T>(req: Request, fn: (owner: string) => Promise<T>): Promise<Response> {
  const sub = await requireOwner(req);
  if (!sub) return json({ error: "unauthenticated" }, 401);
  const rl = await checkRateLimit(`bff:${sub}:${clientIp(req)}`, READ_RATE_PER_SEC);
  if (!rl.ok) {
    return json({ error: "rate_limited" }, 429, rl.retryAfter ? { "retry-after": String(rl.retryAfter) } : {});
  }
  try {
    return json(await fn(sub));
  } catch {
    return json({ error: "persistence unavailable" }, 503);
  }
}

/**
 * PBA-L3c-038: what of a gateway error the browser may see. A 4xx carries the
 * gateway's own deliberate `error` string (e.g. "not authorized for resource"),
 * trimmed; a 5xx body (internal paths, store errors, upstream detail) and any
 * non-JSON body are replaced with a generic message.
 */
export function publicGatewayMessage(e: GatewayError): string {
  if (e.status >= 400 && e.status < 500) {
    try {
      const parsed = JSON.parse(e.message) as { error?: unknown };
      if (typeof parsed?.error === "string") return parsed.error.slice(0, 200);
    } catch {
      /* not the gateway's JSON error shape */
    }
    if (e.status === 400 && e.message === "invalid path segment") return e.message;
    return `gateway request failed (${e.status})`;
  }
  return e.status === 503 ? "gateway unavailable" : "gateway request failed";
}

export async function withCaller<T>(
  req: Request,
  fn: (caller: GatewayCaller) => Promise<T>,
): Promise<Response> {
  const sub = await requireOwner(req);
  if (!sub) return json({ error: "unauthenticated" }, 401);

  // Per-account + per-IP budget on every gateway-backed read (the
  // sponsored-resource-drain class: WEB-2/3, FUA-GATEWAY-01).
  const rl = await checkRateLimit(`bff:${sub}:${clientIp(req)}`, READ_RATE_PER_SEC);
  if (!rl.ok) {
    return json({ error: "rate_limited" }, 429, rl.retryAfter ? { "retry-after": String(rl.retryAfter) } : {});
  }

  const caller: GatewayCaller = { sub, token: tokenFromRequest(req) };
  try {
    const data = await fn(caller);
    return json(data);
  } catch (e) {
    if (e instanceof GatewayError) {
      // 401/403/404/503 from the gateway pass through; anything else is 502.
      const status = [400, 401, 403, 404, 429, 503].includes(e.status) ? e.status : 502;
      return json({ error: publicGatewayMessage(e) }, status);
    }
    return json({ error: "gateway request failed" }, 502);
  }
}
