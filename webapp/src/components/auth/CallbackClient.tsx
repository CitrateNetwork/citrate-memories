"use client";

/**
 * Client half of the OIDC callback — the actual PKCE code→token exchange. Kept
 * separate from the route's `page.tsx` (a server component) so the page can be
 * `force-dynamic`: the per-request nonce CSP only stamps framework scripts on
 * dynamically-rendered routes, and a statically-prerendered client page ships
 * scripts with no nonce → `strict-dynamic` blocks them → this effect never runs
 * → sign-in silently hangs. (Same trap the home route hit; same fix.)
 */
import { useEffect, useState } from "react";
import Link from "next/link";
import { OIDC_PUBLIC } from "@/lib/auth/config";
import { oidcEndpoints } from "@/lib/auth/discovery";

const VERIFIER_KEY = "memrizz.auth.oidc.verifier";
const STATE_KEY = "memrizz.auth.oidc.state";

export function CallbackClient() {
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      const params = new URLSearchParams(location.search);
      const code = params.get("code");
      const returnedState = params.get("state");
      const verifier = sessionStorage.getItem(VERIFIER_KEY);
      const expectedState = sessionStorage.getItem(STATE_KEY);
      if (params.get("error")) {
        setError(params.get("error_description") || params.get("error") || "authorization denied");
        return;
      }
      if (!code || !verifier) {
        setError("Missing authorization code or PKCE verifier.");
        return;
      }
      if (!returnedState || returnedState !== expectedState) {
        setError("State mismatch — possible CSRF; sign-in aborted.");
        return;
      }
      try {
        const ep = await oidcEndpoints();
        const res = await fetch(ep.token, {
          method: "POST",
          headers: { "content-type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({
            grant_type: "authorization_code",
            code,
            client_id: OIDC_PUBLIC.clientId,
            redirect_uri: `${location.origin}${OIDC_PUBLIC.redirectPath}`,
            code_verifier: verifier,
          }),
        });
        if (!res.ok) throw new Error("token exchange failed");
        const tok = await res.json();
        const idToken = tok.id_token || tok.access_token;
        if (!idToken) throw new Error("no id_token in response");
        const set = await fetch("/api/auth/session", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            id_token: idToken,
            access_token: tok.access_token || undefined,
          }),
        });
        if (!set.ok) throw new Error("session cookie could not be set");
        sessionStorage.removeItem(VERIFIER_KEY);
        sessionStorage.removeItem(STATE_KEY);
        location.replace("/");
      } catch (e) {
        setError((e as Error).message);
      }
    })();
  }, []);

  return (
    <main
      style={{
        display: "grid",
        placeItems: "center",
        height: "100vh",
        background: "var(--field)",
        color: "var(--ondark)",
        fontFamily: "var(--font-sans)",
        padding: 40,
        textAlign: "center",
      }}
    >
      <div>
        <div style={{ fontFamily: "var(--font-display)", fontSize: 22, marginBottom: 8 }}>
          {error ? "Sign-in failed" : "Completing sign-in…"}
        </div>
        <div style={{ color: "var(--ondark-2)", fontSize: 14, maxWidth: 420 }}>
          {error ?? "Verifying your identity with the Citrate authority."}
        </div>
        {error ? (
          <Link
            href="/"
            style={{ color: "var(--citrate-green)", fontSize: 14, display: "inline-block", marginTop: 16 }}
          >
            ← Back to Memrizz
          </Link>
        ) : null}
      </div>
    </main>
  );
}
