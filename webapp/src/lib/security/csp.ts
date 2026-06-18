/**
 * Content-Security-Policy builder (FUA-EXPLORER-04 lineage). Built per request in
 * `src/proxy.ts` with a fresh nonce, so script-src needs no 'unsafe-inline':
 *
 *  - `'nonce-<random>'` — first-party inline scripts carry this nonce, as do
 *    Next's own framework scripts (Next reads the CSP request header and injects
 *    it).
 *  - `'strict-dynamic'` — scripts loaded BY nonce'd scripts are trusted
 *    transitively; host allowances and 'self' are ignored by CSP3 browsers and
 *    kept only as a CSP2 fallback.
 *
 * Known, documented relaxation: `style-src 'unsafe-inline'` remains — the ported
 * Memrizz design uses inline `style=` ATTRIBUTES and dynamic custom properties
 * (--accent, per-node colors), and nonces cannot cover style attributes. Style
 * injection is not script execution; the script-src lockdown is the XSS half.
 *
 * connect-src is env-derived: the browser talks to the app's own origin ('self',
 * which fronts the gateway via the BFF), the OIDC authority, and — when a deploy
 * lets the browser reach SSE directly — the mem-gateway origin. Fonts are
 * self-hosted (next/font) so no fonts.googleapis.com is needed. No chain RPC.
 */

function originOf(url: string | undefined): string | null {
  if (!url) return null;
  try {
    return new URL(url).origin;
  } catch {
    return null;
  }
}

/** Build the full CSP for one request. `nonce` MUST be unique per request. */
export function buildCsp(nonce: string): string {
  // Origins the browser may connect to (fetch / SSE), from config. 'self' covers
  // the BFF, which is the primary path to the gateway (browser never holds a
  // gateway credential). A direct-SSE deploy can add the gateway origin.
  const connect = new Set<string>(["'self'"]);
  for (const u of [
    process.env.NEXT_PUBLIC_OIDC_ISSUER,
    process.env.NEXT_PUBLIC_OIDC_AUTHORIZE_URL,
    process.env.NEXT_PUBLIC_OIDC_TOKEN_URL,
    process.env.NEXT_PUBLIC_MEM_GATEWAY_ORIGIN,
  ]) {
    const o = originOf(u);
    if (o) connect.add(o);
  }
  // Privy talks to *.privy.io over https + wss when an app id is configured.
  if (process.env.NEXT_PUBLIC_PRIVY_APP_ID) {
    connect.add("https://*.privy.io");
    connect.add("wss://*.privy.io");
  }

  const frame = new Set<string>(["'self'"]);
  if (process.env.NEXT_PUBLIC_PRIVY_APP_ID) {
    frame.add("https://*.privy.io");
    frame.add("https://auth.privy.io");
  }

  const directives: Record<string, string> = {
    "default-src": "'self'",
    // Nonce + strict-dynamic; never 'unsafe-inline' (FUA-EXPLORER-04).
    "script-src": `'self' 'nonce-${nonce}' 'strict-dynamic'`,
    "style-src": "'self' 'unsafe-inline'",
    "img-src": "'self' data: blob:",
    "font-src": "'self' data:",
    "connect-src": [...connect].join(" "),
    "frame-src": [...frame].join(" "),
    "worker-src": "'self' blob:",
    "manifest-src": "'self'",
    "object-src": "'none'",
    "base-uri": "'self'",
    "form-action": "'self'",
    "frame-ancestors": "'none'",
    "upgrade-insecure-requests": "",
  };

  return Object.entries(directives)
    .map(([k, v]) => (v ? `${k} ${v}` : k))
    .join("; ");
}
