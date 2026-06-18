/**
 * CSP lockdown guard (FUA-EXPLORER-04 lineage). The script-src must be
 * nonce-based with strict-dynamic and carry NO 'unsafe-inline'; the policy must
 * keep object-src 'none', base-uri 'self', and frame-ancestors 'none'; and
 * connect-src must include env-configured origins (gateway / OIDC) so the browser
 * can reach them.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { buildCsp } from "./csp";

afterEach(() => vi.unstubAllEnvs());

function directives(csp: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const part of csp.split(";")) {
    const t = part.trim();
    if (!t) continue;
    const sp = t.indexOf(" ");
    if (sp === -1) out[t] = "";
    else out[t.slice(0, sp)] = t.slice(sp + 1);
  }
  return out;
}

describe("CSP — script-src lockdown", () => {
  it("script-src is nonce + strict-dynamic with NO unsafe-inline", () => {
    const d = directives(buildCsp("ABC123"));
    expect(d["script-src"]).toContain("'nonce-ABC123'");
    expect(d["script-src"]).toContain("'strict-dynamic'");
    expect(d["script-src"]).not.toContain("'unsafe-inline'");
  });

  it("keeps the hardening directives", () => {
    const d = directives(buildCsp("n"));
    expect(d["object-src"]).toBe("'none'");
    expect(d["base-uri"]).toBe("'self'");
    expect(d["frame-ancestors"]).toBe("'none'");
    expect(d).toHaveProperty("upgrade-insecure-requests");
  });

  it("connect-src includes 'self' and env-configured gateway + OIDC origins", () => {
    vi.stubEnv("NEXT_PUBLIC_MEM_GATEWAY_ORIGIN", "https://gw.memrizz.ai");
    vi.stubEnv("NEXT_PUBLIC_OIDC_ISSUER", "https://auth.citrate.ai");
    const d = directives(buildCsp("n"));
    expect(d["connect-src"]).toContain("'self'");
    expect(d["connect-src"]).toContain("https://gw.memrizz.ai");
    expect(d["connect-src"]).toContain("https://auth.citrate.ai");
  });
});
