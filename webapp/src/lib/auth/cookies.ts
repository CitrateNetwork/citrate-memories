/**
 * Auth-cookie plumbing (FUA-EXPLORER-04 lineage). The OIDC id/access tokens live
 * in httpOnly cookies — never in web storage, never readable by page script.
 * Shared by the /api/auth/session route (set/clear) and `verifySession` (read
 * fallback when no Authorization header is present).
 *
 * Web-standard Request/Headers only (no next/server import) so every piece is
 * unit-testable in the node vitest environment.
 */

/** httpOnly cookie carrying the OIDC ID token (the app's session credential). */
export const ID_COOKIE = "citrate_oidc_id";
/** httpOnly cookie carrying the authority access token (logout/userinfo). */
export const ACCESS_COOKIE = "citrate_oidc_access";

/** Loose JWT shape check (three base64url segments, bounded size). */
export function looksLikeJwt(token: string): boolean {
  return (
    token.length > 0 &&
    token.length <= 8192 &&
    /^[\w-]+\.[\w-]+\.[\w-]*$/.test(token)
  );
}

/** Decode (NOT verify) a JWT payload. Verification stays in verifySession. */
export function decodeJwtPayload(jwt: string): Record<string, unknown> | null {
  try {
    const payload = jwt.split(".")[1];
    return JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  } catch {
    return null;
  }
}

/** Read one cookie from a standard Request. */
export function cookieValue(req: Request, name: string): string | null {
  const header = req.headers.get("cookie");
  if (!header) return null;
  for (const part of header.split(";")) {
    const eq = part.indexOf("=");
    if (eq === -1) continue;
    if (part.slice(0, eq).trim() === name) {
      try {
        return decodeURIComponent(part.slice(eq + 1).trim()) || null;
      } catch {
        return part.slice(eq + 1).trim() || null;
      }
    }
  }
  return null;
}

/**
 * Serialize the httpOnly auth cookie. SameSite=Strict: the cookie rides only on
 * same-site requests (all the app's API fetches), never cross-site — the CSRF
 * mitigation for cookie auth. `Secure` outside dev.
 */
export function serializeAuthCookie(name: string, value: string, maxAge: number): string {
  const secure = process.env.NODE_ENV === "production" ? "; Secure" : "";
  return (
    `${name}=${encodeURIComponent(value)}; Path=/; HttpOnly; SameSite=Strict` +
    `; Max-Age=${Math.max(0, Math.floor(maxAge))}${secure}`
  );
}

/** Expire the cookie immediately (server-side logout). */
export function clearAuthCookie(name: string): string {
  const secure = process.env.NODE_ENV === "production" ? "; Secure" : "";
  return `${name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0${secure}`;
}
