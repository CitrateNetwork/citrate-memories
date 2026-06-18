/**
 * Auth seam configuration. Client-safe values (NEXT_PUBLIC_*) live here; the
 * server reads its own secrets/claim names in `session.ts`.
 *
 * PROVISIONAL: issuer/endpoint URLs and claim names. Isolated here so a rename
 * is one line (the rest of the app never references these strings).
 */
import type { AuthMode } from "./types";

/**
 * Selected backend. WEB-1 (SECREM-01): the default is `oidc`, never `mock` — an
 * unset NEXT_PUBLIC_AUTH_MODE must fail closed, not open the forged-token dev
 * gate. Local dev sets NEXT_PUBLIC_AUTH_MODE=mock explicitly (.env.local). The
 * server-side enforcement matrix lives in `session.ts` (`resolveServerAuthMode`).
 */
export const AUTH_MODE: AuthMode =
  (process.env.NEXT_PUBLIC_AUTH_MODE as AuthMode) || "oidc";

/** Public OIDC client config (Authorization Code + PKCE; public client, no secret). */
export const OIDC_PUBLIC = {
  /** Discovery base, e.g. https://auth.citrate.ai (provisional). */
  issuer: process.env.NEXT_PUBLIC_OIDC_ISSUER || "",
  authorizeUrl: process.env.NEXT_PUBLIC_OIDC_AUTHORIZE_URL || "",
  tokenUrl: process.env.NEXT_PUBLIC_OIDC_TOKEN_URL || "",
  clientId: process.env.NEXT_PUBLIC_OIDC_CLIENT_ID || "memrizz",
  scope: process.env.NEXT_PUBLIC_OIDC_SCOPE || "openid profile wallet",
  redirectPath: "/auth/callback",
} as const;

/**
 * PROVISIONAL claim names. The ID token's subject + wallet claims. Override via
 * env when the identity ADR lands; the rest of the app never references these.
 */
export const CLAIM = {
  sub: process.env.NEXT_PUBLIC_AUTH_CLAIM_SUB || "sub",
  wallet: process.env.NEXT_PUBLIC_AUTH_CLAIM_WALLET || "wallet_address",
} as const;

/** A deterministic dev wallet for the mock issuer (NOT a real key — dev identity only). */
export const MOCK_DEV_ADDRESS =
  (process.env.NEXT_PUBLIC_AUTH_MOCK_ADDRESS as `0x${string}`) ||
  "0xf78c4b2091ad55e0a7c0e3b8f4a1d9c62b0d2915";
