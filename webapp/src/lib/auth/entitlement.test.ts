/**
 * Entitlement/KYC claim contract (AUTHSPINE S2-WP2 — explorer as the reference RP).
 * Mirrors @citrate/oidc-client; keep in sync.
 */
import { describe, it, expect } from "vitest";
import {
  ENTITLEMENT_CLAIM,
  parseKycStatus,
  parseEntitlement,
  effectiveTier,
  requireTier,
  requireRole,
  isKycVerified,
  accountHubUrl,
} from "./entitlement";

const ent = (o: Record<string, unknown>) => ({ [ENTITLEMENT_CLAIM]: o });

describe("entitlement claim parsing", () => {
  it("parses kyc_status, defaulting unknown/absent to none", () => {
    expect(parseKycStatus({ kyc_status: "verified" })).toBe("verified");
    expect(parseKycStatus({ kyc_status: "bogus" })).toBe("none");
    expect(parseKycStatus({})).toBe("none");
  });

  it("parses the entitlement claim; unknown tier or absent ⇒ null (Public)", () => {
    expect(parseEntitlement(ent({ tier: "confidential", orgId: null, citrateRole: "admin" }))).toEqual({
      tier: "confidential",
      orgId: null,
      citrateRole: "admin",
      milestone: undefined,
      expiresAt: null,
    });
    expect(parseEntitlement(ent({ tier: "superadmin" }))).toBeNull();
    expect(parseEntitlement({})).toBeNull();
  });
});

describe("RBAC gates", () => {
  it("requireTier uses the ladder; commercial.kyc ≥ commercial; absent ⇒ public", () => {
    const e = parseEntitlement(ent({ tier: "commercial.kyc", orgId: null }));
    expect(requireTier(e, "commercial")).toBe(true);
    expect(requireTier(e, "confidential")).toBe(false);
    expect(requireTier(null, "commercial")).toBe(false);
    expect(requireTier(null, "public")).toBe(true);
  });

  it("expired entitlement collapses to public", () => {
    const e = parseEntitlement(ent({ tier: "confidential", orgId: null, expiresAt: Date.now() - 1000 }));
    expect(effectiveTier(e)).toBe("public");
  });

  it("requireRole matches citrateRole", () => {
    const e = parseEntitlement(ent({ tier: "confidential", orgId: null, citrateRole: "auditor" }));
    expect(requireRole(e, "auditor")).toBe(true);
    expect(requireRole(e, "admin")).toBe(false);
  });

  it("isKycVerified honors expiry", () => {
    expect(isKycVerified("verified")).toBe(true);
    expect(isKycVerified("verified", "2000-01-01T00:00:00Z")).toBe(false);
    expect(isKycVerified("pending")).toBe(false);
  });
});

describe("account hub link", () => {
  it("builds the hub URL with return_to", () => {
    expect(accountHubUrl("https://auth.citrate.ai", "https://explorer.citrate.ai")).toBe(
      "https://auth.citrate.ai/account?return_to=https%3A%2F%2Fexplorer.citrate.ai",
    );
  });
});
