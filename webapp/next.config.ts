import type { NextConfig } from "next";

/**
 * Static, request-independent security headers (cloned from citrate-explorer,
 * SECREM-02 lineage).
 *
 * The CSP is NOT here: a static header cannot carry a per-request nonce, so the
 * policy is built per request in `src/proxy.ts` from `src/lib/security/csp.ts`
 * (script-src 'nonce-…' 'strict-dynamic', NO 'unsafe-inline'). Only static
 * headers live below. Fonts are self-hosted via next/font, so no
 * fonts.googleapis.com is needed in connect-src/font-src.
 */
const securityHeaders = [
  {
    key: "Strict-Transport-Security",
    value: "max-age=63072000; includeSubDomains; preload",
  },
  { key: "X-Content-Type-Options", value: "nosniff" },
  { key: "X-Frame-Options", value: "DENY" },
  { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
  {
    key: "Permissions-Policy",
    value: "camera=(), microphone=(), geolocation=(), browsing-topics=()",
  },
  { key: "X-DNS-Prefetch-Control", value: "on" },
];

const nextConfig: NextConfig = {
  poweredByHeader: false,
  async headers() {
    return [{ source: "/:path*", headers: securityHeaders }];
  },
};

export default nextConfig;
