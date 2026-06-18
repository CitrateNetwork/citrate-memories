/**
 * WEB-1 regression suite (SECREM-01 lineage), cloned for Memrizz.
 *
 * The finding: defaulting NEXT_PUBLIC_AUTH_MODE to "mock" and trusting the
 * claims inside an UNSIGNED base64url token means a public deploy with the env
 * var unset accepts `Authorization: Bearer base64url({"sub":"<victim>"})` and
 * returns the victim's identity — full account takeover with no key material.
 *
 * Red state (pre-fix): the "forged token" tests FAIL (the forged subject comes
 * back authenticated). Post-fix they pass: unset → oidc, mock-in-production →
 * disabled unless ALLOW_MOCK_AUTH=1.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

const VICTIM = "did:citrate:victim-7f3a";
const VICTIM_WALLET = "0xAb5801a7D398351b8bE11C439e05C5b3259aec9B";

function forgedToken(sub: string, wallet: string): string {
  return Buffer.from(JSON.stringify({ sub, wallet_address: wallet })).toString("base64url");
}
function forgedRequest(): Request {
  return new Request("http://x", {
    headers: { authorization: `Bearer ${forgedToken(VICTIM, VICTIM_WALLET)}` },
  });
}

const ENV_KEYS = ["NEXT_PUBLIC_AUTH_MODE", "ALLOW_MOCK_AUTH", "NODE_ENV"] as const;

function setEnv(env: Partial<Record<(typeof ENV_KEYS)[number], string | undefined>>) {
  for (const k of ENV_KEYS) {
    const v = env[k];
    if (v === undefined) {
      vi.stubEnv(k, "");
      delete (process.env as Record<string, string | undefined>)[k];
    } else {
      vi.stubEnv(k, v);
    }
  }
}

afterEach(() => {
  vi.unstubAllEnvs();
});

async function freshSession() {
  vi.resetModules();
  return await import("./session");
}

describe("WEB-1 — mode resolution matrix (resolveServerAuthMode)", () => {
  it("unset mode resolves to oidc, never mock", async () => {
    const { resolveServerAuthMode } = await freshSession();
    expect(resolveServerAuthMode({})).toBe("oidc");
    expect(resolveServerAuthMode({ NODE_ENV: "production" })).toBe("oidc");
    expect(resolveServerAuthMode({ NODE_ENV: "development" })).toBe("oidc");
  });

  it("unknown mode value fails closed to oidc", async () => {
    const { resolveServerAuthMode } = await freshSession();
    expect(resolveServerAuthMode({ NEXT_PUBLIC_AUTH_MODE: "mokc" })).toBe("oidc");
    expect(resolveServerAuthMode({ NEXT_PUBLIC_AUTH_MODE: "" })).toBe("oidc");
  });

  it("mock in production is disabled without the explicit opt-in", async () => {
    const { resolveServerAuthMode } = await freshSession();
    expect(
      resolveServerAuthMode({ NEXT_PUBLIC_AUTH_MODE: "mock", NODE_ENV: "production" }),
    ).toBe("mock-disabled");
  });

  it("mock in production with ALLOW_MOCK_AUTH=1 is honored (explicit staging opt-in)", async () => {
    const { resolveServerAuthMode } = await freshSession();
    expect(
      resolveServerAuthMode({
        NEXT_PUBLIC_AUTH_MODE: "mock",
        NODE_ENV: "production",
        ALLOW_MOCK_AUTH: "1",
      }),
    ).toBe("mock");
  });

  it("mock outside production stays available for dev", async () => {
    const { resolveServerAuthMode } = await freshSession();
    expect(resolveServerAuthMode({ NEXT_PUBLIC_AUTH_MODE: "mock", NODE_ENV: "test" })).toBe("mock");
    expect(
      resolveServerAuthMode({ NEXT_PUBLIC_AUTH_MODE: "mock", NODE_ENV: "development" }),
    ).toBe("mock");
  });
});

describe("WEB-1 — forged unsigned tokens are rejected end-to-end", () => {
  it("default deploy (mode unset): forged token yields NO identity", async () => {
    setEnv({ NEXT_PUBLIC_AUTH_MODE: undefined, NODE_ENV: "production" });
    const { verifySession, requireOwner } = await freshSession();
    const s = await verifySession(forgedRequest());
    expect(s.authenticated).toBe(false);
    expect(s.sub).toBeUndefined();
    expect(await requireOwner(forgedRequest())).toBeNull();
  });

  it("mock mode in production (no opt-in): forged token yields NO identity", async () => {
    setEnv({ NEXT_PUBLIC_AUTH_MODE: "mock", NODE_ENV: "production", ALLOW_MOCK_AUTH: undefined });
    const { verifySession, requireOwner } = await freshSession();
    const s = await verifySession(forgedRequest());
    expect(s.required).toBe(true);
    expect(s.authenticated).toBe(false);
    expect(await requireOwner(forgedRequest())).toBeNull();
  });

  it("mock mode in production (no opt-in): dev header backdoor is dead too", async () => {
    setEnv({ NEXT_PUBLIC_AUTH_MODE: "mock", NODE_ENV: "production", ALLOW_MOCK_AUTH: undefined });
    const { requireOwner } = await freshSession();
    const req = new Request("http://x", { headers: { "x-citrate-dev-address": VICTIM_WALLET } });
    expect(await requireOwner(req)).toBeNull();
  });

  it("unauthenticated request in a defaulted production deploy is required+rejected", async () => {
    setEnv({ NEXT_PUBLIC_AUTH_MODE: undefined, NODE_ENV: "production" });
    const { verifySession } = await freshSession();
    const s = await verifySession(new Request("http://x"));
    expect(s.required).toBe(true);
    expect(s.authenticated).toBe(false);
  });
});
