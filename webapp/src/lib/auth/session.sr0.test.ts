/**
 * SR-0: per-user data is owned by the stable OIDC `subject`, NOT the wallet.
 * Guards that an email/social/passkey identity with no wallet is a first-class
 * owner, and that `sub` is opaque + case-sensitive (never lower-cased). In
 * Memrizz the `sub` is also the key the gateway maps to Org membership.
 */
import { afterAll, beforeAll, describe, it, expect, vi } from "vitest";
import { sessionOwner } from "./session";

function mockToken(claims: Record<string, unknown>): string {
  return Buffer.from(JSON.stringify(claims)).toString("base64url");
}
function req(token?: string): Request {
  return new Request("http://x/api/x", {
    headers: token ? { authorization: `Bearer ${token}` } : {},
  });
}

let requireOwner: (typeof import("./session"))["requireOwner"];
const savedMode = process.env.NEXT_PUBLIC_AUTH_MODE;

beforeAll(async () => {
  process.env.NEXT_PUBLIC_AUTH_MODE = "mock";
  vi.resetModules();
  ({ requireOwner } = await import("./session"));
});

afterAll(() => {
  if (savedMode === undefined) delete process.env.NEXT_PUBLIC_AUTH_MODE;
  else process.env.NEXT_PUBLIC_AUTH_MODE = savedMode;
});

describe("SR-0 — owner is the OIDC subject", () => {
  it("sessionOwner returns sub, ignoring the wallet, verbatim (no lower-casing)", () => {
    expect(
      sessionOwner({ required: true, authenticated: true, sub: "google-oauth2|abc", walletAddress: "0xDEAD" }),
    ).toBe("google-oauth2|abc");
    expect(sessionOwner({ required: true, authenticated: true, sub: "Email|XyZ-123" })).toBe("Email|XyZ-123");
    expect(sessionOwner({ required: true, authenticated: true, walletAddress: "0xabc" })).toBeNull();
  });

  it("authorizes a wallet-LESS identity (sub only)", async () => {
    expect(await requireOwner(req(mockToken({ sub: "email-user-1" })))).toBe("email-user-1");
  });

  it("still resolves the owner when a wallet is also present", async () => {
    expect(await requireOwner(req(mockToken({ sub: "user-2", wallet_address: "0xfeed" })))).toBe("user-2");
  });

  it("yields null when unauthenticated", async () => {
    expect(await requireOwner(req())).toBeNull();
  });
});
