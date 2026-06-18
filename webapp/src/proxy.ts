/**
 * Per-request CSP nonce (FUA-EXPLORER-04 — drops `script-src 'unsafe-inline'`).
 * Next 16 proxy (the middleware successor):
 *
 *  1. mint a fresh random nonce for every request,
 *  2. set the nonce'd CSP on the FORWARDED REQUEST headers — Next reads the
 *     `content-security-policy` request header during SSR and stamps the nonce
 *     onto its own framework <script> tags automatically,
 *  3. expose the nonce to server components as `x-nonce`,
 *  4. mirror the CSP onto the RESPONSE so the browser enforces it.
 *
 * The static security headers (HSTS etc.) stay in next.config.ts.
 */
import { NextResponse, type NextRequest } from "next/server";
import { buildCsp } from "@/lib/security/csp";

export default function proxy(request: NextRequest) {
  // 128 bits of entropy, base64 — fine for a CSP nonce.
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const nonce = btoa(String.fromCharCode(...bytes));
  const csp = buildCsp(nonce);

  const requestHeaders = new Headers(request.headers);
  requestHeaders.set("x-nonce", nonce);
  requestHeaders.set("content-security-policy", csp);

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  response.headers.set("content-security-policy", csp);
  return response;
}

export const config = {
  matcher: [
    {
      source: "/((?!_next/static|_next/image|favicon.ico).*)",
      missing: [
        { type: "header", key: "next-router-prefetch" },
        { type: "header", key: "purpose", value: "prefetch" },
      ],
    },
  ],
};
