import { CallbackClient } from "@/components/auth/CallbackClient";

/**
 * OIDC Authorization Code + PKCE callback route.
 *
 * Server component so `force-dynamic` actually takes effect: the per-request
 * nonce CSP (src/proxy.ts) only stamps framework scripts on dynamically-rendered
 * routes. A statically-prerendered page ships scripts with NO nonce, which
 * `script-src 'strict-dynamic'` then blocks — so the client effect never runs
 * and sign-in hangs forever. The exchange logic lives in <CallbackClient/>.
 */
export const dynamic = "force-dynamic";

export default function AuthCallbackPage() {
  return <CallbackClient />;
}
