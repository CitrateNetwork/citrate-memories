/**
 * Server-side session verification — the server half of the auth seam.
 *
 * `verifySession(req)` is the ONLY identity check API routes / server actions
 * call. It returns a normalized {@link AuthSession} regardless of the concrete
 * issuer:
 *  - `oidc`  — verify a Citrate-issued JWT against the authority's JWKS (jose),
 *    checking issuer + audience, then read the (provisional) sub + wallet claims.
 *  - `mock`  — dev only: decode an unsigned mock token / dev header. `required:false`
 *    so the app runs locally without an authority.
 *  - `privy` — optional: dynamically imported so no Privy code is referenced
 *    unless explicitly selected.
 *
 * Data source (Rule 11): the Bearer JWT verified against the configured JWKS.
 * Org resolution happens AFTER this, in the gateway control-plane (PLANSET/07).
 */
import { createRemoteJWKSet, jwtVerify } from "jose";
import { ID_COOKIE, cookieValue } from "./cookies";
import { parseKycStatus, parseEntitlement } from "./entitlement";
import type { AuthSession } from "./types";

/**
 * WEB-1 (SECREM-01): server-side mode resolution is fail-closed. An
 * undeployed/unset NEXT_PUBLIC_AUTH_MODE must NOT silently accept forged,
 * unsigned bearer tokens. Therefore:
 *  - unset or unrecognized mode  → `oidc` (rejects everything until a JWKS is
 *    configured — fail closed, never fail open);
 *  - `mock` in production        → disabled (every request unauthenticated)
 *    unless the operator explicitly sets ALLOW_MOCK_AUTH=1.
 * Exported for direct unit testing of the resolution matrix.
 */
export type ServerAuthMode = "oidc" | "privy" | "mock" | "mock-disabled";
export function resolveServerAuthMode(
  env: Record<string, string | undefined> = process.env,
): ServerAuthMode {
  const requested = env.NEXT_PUBLIC_AUTH_MODE;
  if (requested === "oidc" || requested === "privy") return requested;
  if (requested === "mock") {
    if (env.NODE_ENV === "production" && env.ALLOW_MOCK_AUTH !== "1") {
      return "mock-disabled";
    }
    return "mock";
  }
  // Unset or unknown value: fail closed onto the verifying path.
  return "oidc";
}

const MODE = resolveServerAuthMode();

let warnedMockDisabled = false;

const claimSub = () =>
  process.env.AUTH_CLAIM_SUB || process.env.NEXT_PUBLIC_AUTH_CLAIM_SUB || "sub";
const claimWallet = () =>
  process.env.AUTH_CLAIM_WALLET ||
  process.env.NEXT_PUBLIC_AUTH_CLAIM_WALLET ||
  "wallet_address";

/**
 * The caller's credential: the Authorization Bearer header when present,
 * otherwise the httpOnly session cookie (FUA-EXPLORER-04 — tokens are NOT in
 * localStorage; the browser carries them as SameSite=Strict cookies and sends no
 * header). Both paths feed the SAME verification below; the cookie is transport,
 * not trust.
 */
export function tokenFromRequest(req: Request): string | null {
  const header = req.headers.get("authorization")?.replace(/^Bearer\s+/i, "");
  if (header) return header;
  return cookieValue(req, ID_COOKIE);
}

function bearer(req: Request): string | null {
  return tokenFromRequest(req);
}

// --- OIDC (real authority) --------------------------------------------------

let jwks: ReturnType<typeof createRemoteJWKSet> | null = null;
function jwkSet() {
  const url = process.env.OIDC_JWKS_URL;
  if (!url) throw new Error("OIDC_JWKS_URL is not set");
  if (!jwks) jwks = createRemoteJWKSet(new URL(url));
  return jwks;
}

let warnedOidcConfig = false;

/**
 * FUA-EXPLORER-01 (SECREM-02): issuer + audience MUST be enforced. Never pass
 * `undefined` to `jwtVerify` — when an env is unset that claim is NOT checked,
 * so any token signed by a key in the configured JWKS would be accepted
 * regardless of which relying party it was minted for (auth.citrate.ai is a
 * shared authority across the federation). We require both and fail closed
 * (reject every token, loudly) when either is missing.
 */
function requiredOidcConfig(): { issuer: string; audience: string } | null {
  const issuer = process.env.OIDC_ISSUER;
  const audience = process.env.OIDC_AUDIENCE;
  if (!issuer || !audience) return null;
  return { issuer, audience };
}

async function verifyOidc(req: Request): Promise<AuthSession> {
  const token = bearer(req);
  if (!token) return { required: true, authenticated: false };
  const cfg = requiredOidcConfig();
  if (!cfg) {
    if (!warnedOidcConfig) {
      warnedOidcConfig = true;
      console.error(
        "[auth] OIDC_ISSUER and OIDC_AUDIENCE must both be set in oidc mode; " +
          "refusing all tokens until configured (fail closed, FUA-EXPLORER-01). " +
          "A token minted for a different relying party must never be accepted here.",
      );
    }
    return { required: true, authenticated: false };
  }
  try {
    const { payload } = await jwtVerify(token, jwkSet(), {
      issuer: cfg.issuer,
      audience: cfg.audience,
    });
    const sub = payload[claimSub()] as string | undefined;
    const walletAddress = (payload[claimWallet()] as string | undefined)?.toLowerCase();
    const claims = payload as Record<string, unknown>;
    return { required: true, authenticated: Boolean(sub), sub, walletAddress, kycStatus: parseKycStatus(claims), entitlement: parseEntitlement(claims) };
  } catch {
    return { required: true, authenticated: false };
  }
}

// --- Mock issuer (dev) ------------------------------------------------------

function verifyMock(req: Request): AuthSession {
  // Accept either the client mock token (unsigned base64url JSON) or a dev header.
  const token = bearer(req);
  if (token) {
    try {
      const json = JSON.parse(Buffer.from(token, "base64url").toString("utf8"));
      const sub = json[claimSub()] ?? json.sub;
      const walletAddress = (json[claimWallet()] ?? json.wallet_address)?.toLowerCase();
      if (sub || walletAddress) {
        return { required: false, authenticated: true, sub, walletAddress, kycStatus: parseKycStatus(json), entitlement: parseEntitlement(json) };
      }
    } catch {
      /* fall through to header */
    }
  }
  const devAddr = req.headers.get("x-citrate-dev-address")?.toLowerCase();
  if (devAddr) {
    return { required: false, authenticated: true, sub: `dev:${devAddr}`, walletAddress: devAddr };
  }
  // Dev gate is open: unauthenticated calls are allowed (no enforcement).
  return { required: false, authenticated: false };
}

export async function verifySession(req: Request): Promise<AuthSession> {
  if (MODE === "oidc") return verifyOidc(req);
  if (MODE === "privy") {
    const { verifyPrivy } = await import("./adapters/privy.server");
    return verifyPrivy(req);
  }
  if (MODE === "mock-disabled") {
    // WEB-1: mock requested in production without the explicit ALLOW_MOCK_AUTH=1
    // opt-in. Hard-fail every request rather than trust unsigned tokens.
    if (!warnedMockDisabled) {
      warnedMockDisabled = true;
      console.error(
        "[auth] NEXT_PUBLIC_AUTH_MODE=mock is DISABLED in production " +
          "(forged-token risk, WEB-1). All requests are unauthenticated. Set " +
          "NEXT_PUBLIC_AUTH_MODE=oidc, or ALLOW_MOCK_AUTH=1 only for a non-public " +
          "staging deploy.",
      );
    }
    return { required: true, authenticated: false };
  }
  return verifyMock(req);
}

/** Convenience for routes: the caller's wallet address, or null when unauthenticated. */
export function sessionAddress(s: AuthSession): string | null {
  return s.walletAddress ?? null;
}

/**
 * The canonical owner key for all per-user data (SR-0): the stable OIDC `subject`,
 * NOT the wallet address. Returned VERBATIM — `sub` is a case-sensitive opaque
 * identifier; never lower-case it. Null only when unauthenticated, so
 * email/social/passkey identities with no wallet are first-class owners.
 */
export function sessionOwner(s: AuthSession): string | null {
  return s.sub ?? null;
}

/**
 * Resolve the owner for a request in one call — the pattern every authed route
 * uses. Returns the subject string, or null when the caller is unauthenticated.
 * Routes uniformly 401 on null.
 */
export async function requireOwner(req: Request): Promise<string | null> {
  const auth = await verifySession(req);
  if (auth.required && !auth.authenticated) return null;
  return sessionOwner(auth);
}
