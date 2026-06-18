/**
 * Auth seam — provider-agnostic identity contract.
 *
 * Memrizz is a GENERIC OIDC relying party behind this single seam (the same
 * pattern as citrate-explorer; see PLANSET/07 §3.B). The rest of the app depends
 * ONLY on these types + `useAuth()` (client) and `verifySession()` (server). The
 * concrete backend — the Citrate authority (auth.citrate.ai) via Authorization
 * Code + PKCE, the mock issuer in dev, or Privy as an optional adapter — is
 * swappable with zero changes outside `src/lib/auth/`.
 *
 * Org resolution (sub → Org+role) is layered ON TOP of this seam in the gateway
 * control-plane (PLANSET/07 §3.B); the identity authority stays Org-agnostic.
 *
 * NOTE: the claim names (`sub`, `wallet_address`) and issuer/endpoint URLs are
 * PROVISIONAL — they live in one place (`config.ts` + server env) so a rename is
 * trivial.
 */

/** The verified identity, normalized away from any provider's claim shape. */
export interface AuthSession {
  /** Whether this deployment enforces auth (false in local/mock dev). */
  required: boolean;
  /** Whether the caller presented a valid session. */
  authenticated: boolean;
  /** OIDC subject (stable user id) — the canonical owner key. Provisional claim. */
  sub?: string;
  /** The user's wallet address, if the identity carries one. Provisional claim. */
  walletAddress?: string;
}

/** Client-side auth state + actions exposed by `useAuth()`. */
export interface AuthContextValue {
  /** True once the adapter has resolved its initial state. */
  ready: boolean;
  authenticated: boolean;
  sub?: string;
  /** Lower-cased wallet address, or undefined when logged out. */
  address?: `0x${string}`;
  /** Begin login (mock: instant dev session; oidc: PKCE redirect). */
  login: () => void | Promise<void>;
  logout: () => void | Promise<void>;
  /** A bearer token for `Authorization` on API calls, or null when logged out. */
  getToken: () => Promise<string | null>;
}

export type AuthMode = "mock" | "oidc" | "privy";
