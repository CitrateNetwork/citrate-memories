import { describe, it, expect } from "vitest";
import { pkceChallenge } from "./client";

describe("pkceChallenge (RFC 7636 S256)", () => {
  it("matches the canonical RFC 7636 Appendix B test vector", async () => {
    // verifier + expected challenge straight from RFC 7636 Appendix B.
    const verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const challenge = await pkceChallenge(verifier);
    expect(challenge).toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  });

  it("is base64url with no padding and stable for a given verifier", async () => {
    const a = await pkceChallenge("verifier-123");
    const b = await pkceChallenge("verifier-123");
    expect(a).toBe(b);
    expect(a).not.toMatch(/[+/=]/);
  });
});
