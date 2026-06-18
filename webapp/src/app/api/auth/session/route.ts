/**
 * Session cookie endpoint — the server half of the OIDC login round-trip.
 *
 * The browser never stores the bearer (FUA-EXPLORER-04 lineage): the
 * `/auth/callback` page exchanges the auth code for tokens, POSTs them here, and
 * we set httpOnly + SameSite=Strict cookies that page script can never read. The
 * cookie is *transport*, not *trust* — every protected route still re-verifies it
 * against the authority JWKS via `verifySession` (see session.ts). So this route
 * intentionally does NOT trust the token it stores; it only shape-checks it.
 *
 *  - POST   { id_token, access_token? } → set the httpOnly session cookies.
 *  - GET                                → the verified session (for UI state).
 *  - DELETE                             → clear the cookies (server-side logout).
 *
 * Web-standard Request/Response only, so the whole thing is unit-testable.
 */
import {
  ID_COOKIE,
  ACCESS_COOKIE,
  looksLikeJwt,
  decodeJwtPayload,
  serializeAuthCookie,
  clearAuthCookie,
} from "@/lib/auth/cookies";
import { resolveServerAuthMode } from "@/lib/auth/session";
import { verifySession } from "@/lib/auth/session";

/** Default cookie lifetime when the token carries no usable `exp` (1h). */
const DEFAULT_MAX_AGE = 60 * 60;
/** Hard ceiling so a hostile/oversized `exp` can't pin a year-long cookie. */
const MAX_COOKIE_AGE = 60 * 60 * 24 * 7;

/** Seconds until the token's `exp`, clamped; falls back to the default. */
function maxAgeFromToken(idToken: string): number {
  const payload = decodeJwtPayload(idToken);
  const exp = payload && typeof payload.exp === "number" ? payload.exp : null;
  if (!exp) return DEFAULT_MAX_AGE;
  const now = Math.floor(Date.now() / 1000);
  const ttl = exp - now;
  if (ttl <= 0) return 0;
  return Math.min(ttl, MAX_COOKIE_AGE);
}

/**
 * Accept the credential the callback obtained. In `mock` dev the "token" is the
 * unsigned base64url JSON the mock verifier decodes, so we don't force JWT shape
 * there — but in every verifying mode (oidc/privy) we require a real JWT shape
 * before it ever becomes a cookie.
 */
function acceptableCredential(token: string): boolean {
  if (looksLikeJwt(token)) return true;
  return resolveServerAuthMode() === "mock";
}

export async function POST(req: Request): Promise<Response> {
  let body: { id_token?: unknown; access_token?: unknown };
  try {
    body = (await req.json()) as typeof body;
  } catch {
    return Response.json({ error: "invalid json" }, { status: 400 });
  }
  const idToken = typeof body.id_token === "string" ? body.id_token : "";
  const accessToken = typeof body.access_token === "string" ? body.access_token : "";
  if (!idToken || !acceptableCredential(idToken)) {
    return Response.json({ error: "missing or malformed id_token" }, { status: 400 });
  }

  const maxAge = maxAgeFromToken(idToken);
  if (maxAge <= 0) {
    return Response.json({ error: "token already expired" }, { status: 400 });
  }

  const headers = new Headers({ "content-type": "application/json" });
  headers.append("set-cookie", serializeAuthCookie(ID_COOKIE, idToken, maxAge));
  if (accessToken && looksLikeJwt(accessToken)) {
    headers.append("set-cookie", serializeAuthCookie(ACCESS_COOKIE, accessToken, maxAge));
  }
  return new Response(JSON.stringify({ ok: true }), { status: 200, headers });
}

/** The verified session — the client uses this to render logged-in state. */
export async function GET(req: Request): Promise<Response> {
  const s = await verifySession(req);
  return Response.json({
    authenticated: s.authenticated,
    sub: s.sub,
    walletAddress: s.walletAddress,
  });
}

/** Clear both cookies. The authority-side end-session is best-effort client-side. */
export async function DELETE(): Promise<Response> {
  const headers = new Headers({ "content-type": "application/json" });
  headers.append("set-cookie", clearAuthCookie(ID_COOKIE));
  headers.append("set-cookie", clearAuthCookie(ACCESS_COOKIE));
  return new Response(JSON.stringify({ ok: true }), { status: 200, headers });
}
