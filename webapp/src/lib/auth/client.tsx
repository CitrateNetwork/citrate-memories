"use client";

/**
 * Auth seam — client half. `<AuthProvider>` resolves the session from the
 * httpOnly cookie (via GET /api/auth/session) and exposes the normalized
 * {@link AuthContextValue} through `useAuth()`. The rest of the app imports ONLY
 * these — never an issuer URL, never a token.
 *
 * One adapter, two `login()` behaviours selected by AUTH_MODE:
 *  - `oidc`  — Authorization Code + PKCE against the Citrate authority. `login()`
 *    redirects to the issuer; `/auth/callback` exchanges the code and POSTs the
 *    tokens to /api/auth/session, which sets the httpOnly cookies.
 *  - `mock`  — dev only: `login()` mints an unsigned dev token and POSTs it to
 *    the same session route (the server's mock verifier decodes it). No authority.
 *
 * There is NO client-readable bearer: `getToken()` returns null and the cookie
 * rides every same-origin fetch automatically (the BFF reads it server-side).
 */
import {
  createContext,
  useContext,
  useCallback,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { AUTH_MODE, OIDC_PUBLIC, MOCK_DEV_ADDRESS, CLAIM } from "./config";
import { oidcEndpoints, logoutUrl } from "./discovery";
import type { AuthContextValue } from "./types";

const AuthContext = createContext<AuthContextValue | null>(null);

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within <AuthProvider>");
  return ctx;
}

/** One-shot redirect artifacts (NOT bearers): PKCE verifier + CSRF state. */
const VERIFIER_KEY = "memrizz.auth.oidc.verifier";
const STATE_KEY = "memrizz.auth.oidc.state";

/** base64url with no padding — the encoding PKCE + JWT segments use. */
function b64url(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** S256 PKCE challenge for a verifier. Exported for unit testing. */
export async function pkceChallenge(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return b64url(new Uint8Array(digest));
}

interface SessionInfo {
  authenticated: boolean;
  sub?: string;
  walletAddress?: string;
}

function useSession(): AuthContextValue {
  const [session, setSession] = useState<SessionInfo | null>(null);
  const [ready, setReady] = useState(false);

  // Resolve the current session from the httpOnly cookie. The token never
  // reaches page script; the server decodes the claims and returns just these.
  useEffect(() => {
    let cancelled = false;
    void fetch("/api/auth/session", { cache: "no-store" })
      .then((r) => (r.ok ? r.json() : { authenticated: false }))
      .then((j: SessionInfo) => {
        if (!cancelled) setSession(j);
      })
      .catch(() => {
        if (!cancelled) setSession({ authenticated: false });
      })
      .finally(() => {
        if (!cancelled) setReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const login = useCallback(async () => {
    if (AUTH_MODE !== "oidc") {
      // mock dev: post an unsigned token the server's mock verifier decodes,
      // then re-read the session. No authority, no redirect.
      const token = b64url(
        new TextEncoder().encode(
          JSON.stringify({
            [CLAIM.sub]: `mock:${MOCK_DEV_ADDRESS}`,
            [CLAIM.wallet]: MOCK_DEV_ADDRESS.toLowerCase(),
          }),
        ),
      );
      await fetch("/api/auth/session", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ id_token: token }),
      });
      const r = await fetch("/api/auth/session", { cache: "no-store" });
      setSession(r.ok ? await r.json() : { authenticated: false });
      return;
    }
    // oidc: PKCE verifier + CSRF state, both verified on the callback.
    const verifier = crypto.randomUUID() + crypto.randomUUID();
    const state = crypto.randomUUID();
    sessionStorage.setItem(VERIFIER_KEY, verifier);
    sessionStorage.setItem(STATE_KEY, state);
    const challenge = await pkceChallenge(verifier);
    const ep = await oidcEndpoints();
    const url = new URL(ep.authorization);
    url.searchParams.set("response_type", "code");
    url.searchParams.set("client_id", OIDC_PUBLIC.clientId);
    // Runtime origin → works on whatever domain serves the app (vercel.app today,
    // memrizz.citrate.ai once DNS lands), as long as it's a registered redirect.
    url.searchParams.set("redirect_uri", `${location.origin}${OIDC_PUBLIC.redirectPath}`);
    url.searchParams.set("scope", OIDC_PUBLIC.scope);
    url.searchParams.set("code_challenge", challenge);
    url.searchParams.set("code_challenge_method", "S256");
    url.searchParams.set("state", state);
    location.href = url.toString();
  }, []);

  const logout = useCallback(() => {
    // Clear the httpOnly cookies server-side (the client can't read them), then
    // best-effort end the authority session.
    void fetch("/api/auth/session", { method: "DELETE", keepalive: true }).catch(() => {});
    setSession({ authenticated: false });
    if (AUTH_MODE === "oidc") {
      try {
        window.location.assign(logoutUrl());
        return;
      } catch {
        /* best-effort */
      }
    }
  }, []);

  // No client-readable bearer — the cookie is the credential, sent automatically.
  const getToken = useCallback(async () => null, []);

  return {
    ready,
    authenticated: Boolean(session?.authenticated),
    sub: session?.sub,
    address: session?.walletAddress?.toLowerCase() as `0x${string}` | undefined,
    login,
    logout,
    getToken,
  };
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const value = useSession();
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}
