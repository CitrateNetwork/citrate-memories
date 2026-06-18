/**
 * FUA-EXPLORER-04 — OIDC bearer tokens must NEVER live in localStorage/
 * sessionStorage. They are httpOnly cookies set by the server; any injected
 * script must not be able to read a long-lived bearer credential.
 *
 * Source-level tripwire over the whole app source. The PKCE `verifier` and CSRF
 * `state` in sessionStorage are explicitly ALLOWED — one-shot, non-bearer flow
 * artifacts consumed by the callback. (The client provider + /auth/callback page
 * land in the login WP; this tripwire already guards them once they exist.)
 */
import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const SRC = join(__dirname, "..", "..");

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) out.push(...walk(p));
    else if (/\.(ts|tsx)$/.test(name) && !name.includes(".test.")) out.push(p);
  }
  return out;
}

/** localStorage/sessionStorage touching an OIDC token key (constant or literal). */
const TOKEN_STORAGE =
  /(localStorage|sessionStorage)\s*\.\s*(get|set|remove)Item\(\s*(OIDC_TOKEN_KEY|OIDC_ACCESS_KEY|["'`]citrate\.auth\.oidc\.(idtoken|accesstoken))/;

describe("FUA-EXPLORER-04 — no OIDC bearer tokens in web storage", () => {
  it("no source file reads/writes an OIDC id/access token via web storage", () => {
    const offenders: string[] = [];
    for (const file of walk(SRC)) {
      const text = readFileSync(file, "utf8");
      if (TOKEN_STORAGE.test(text)) offenders.push(file.slice(SRC.length + 1));
    }
    expect(offenders).toEqual([]);
  });
});
