"use client";

/**
 * OIDC endpoint discovery (client-side). NEVER hardcode endpoint paths — read
 * the authority's `/.well-known/openid-configuration` (panva's authorization
 * endpoint is `/auth`, not `/authorize`). Explicit env overrides win; the panva
 * defaults are only a last-resort fallback if discovery is unreachable. Cached
 * for the page lifetime.
 *
 * The Citrate-specific session endpoints (`/logout`, `/sessions/events`) are NOT
 * in the OIDC discovery doc — they're derived from the issuer origin.
 */
import { OIDC_PUBLIC } from "./config";

export interface OidcEndpoints {
  authorization: string;
  token: string;
  userinfo: string;
  endSession?: string;
}

let cached: OidcEndpoints | null = null;

export async function oidcEndpoints(): Promise<OidcEndpoints> {
  if (cached) return cached;
  const issuer = OIDC_PUBLIC.issuer.replace(/\/$/, "");
  let disc: Record<string, string> = {};
  try {
    const r = await fetch(`${issuer}/.well-known/openid-configuration`, { cache: "no-store" });
    if (r.ok) disc = await r.json();
  } catch {
    /* fall back to panva defaults below */
  }
  cached = {
    authorization: OIDC_PUBLIC.authorizeUrl || disc.authorization_endpoint || `${issuer}/auth`,
    token: OIDC_PUBLIC.tokenUrl || disc.token_endpoint || `${issuer}/token`,
    userinfo: disc.userinfo_endpoint || `${issuer}/me`,
    endSession: disc.end_session_endpoint,
  };
  return cached;
}

/** Citrate session bus (SSE) for the logout cascade. */
export function sessionEventsUrl(): string {
  return `${OIDC_PUBLIC.issuer.replace(/\/$/, "")}/sessions/events`;
}

/** Citrate logout endpoint — ends the authority session + publishes a logout event. */
export function logoutUrl(): string {
  return `${OIDC_PUBLIC.issuer.replace(/\/$/, "")}/logout`;
}
