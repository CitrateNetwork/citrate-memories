/**
 * Entitlement/KYC claim contract — mirrors `@citrate/oidc-client` (AUTHSPINE
 * S2-WP1, ADR-2026-06-18-rbac-entitlement-claim-contract). Vendored here because the
 * federation has no shared package registry yet; replace with the published
 * `@citrate/oidc-client` import once it's available. Keep the two in sync.
 *
 * Lets CitrateScan read the same kyc_status + `https://citrate.ai/entitlement` claims
 * every Citrate RP reads, and gate API routes/features by tier/role identically.
 */

export const ENTITLEMENT_CLAIM = "https://citrate.ai/entitlement";

export type KycStatus = "verified" | "pending" | "revoked" | "expired" | "none";
export type Tier = "public" | "commercial" | "commercial.kyc" | "academic" | "confidential";

export const TIER_ORDER: Record<Tier, number> = {
  public: 0,
  commercial: 1,
  "commercial.kyc": 2,
  academic: 3,
  confidential: 4,
};

export interface Entitlement {
  tier: Tier;
  orgId: string | null;
  citrateRole?: string;
  milestone?: string;
  expiresAt?: number | null;
}

const TIERS = new Set<string>(Object.keys(TIER_ORDER));
function asTier(v: unknown): Tier | null {
  return typeof v === "string" && TIERS.has(v) ? (v as Tier) : null;
}

/** Parse the kyc_status claim from raw OIDC claims (id_token payload or /userinfo). */
export function parseKycStatus(raw: Record<string, unknown> | null | undefined): KycStatus {
  const k = raw?.kyc_status;
  return k === "verified" || k === "pending" || k === "revoked" || k === "expired" ? k : "none";
}

/** Parse the entitlement claim; absent/unknown ⇒ null (Public, fail-safe). */
export function parseEntitlement(raw: Record<string, unknown> | null | undefined): Entitlement | null {
  const e = raw?.[ENTITLEMENT_CLAIM];
  if (!e || typeof e !== "object") return null;
  const o = e as Record<string, unknown>;
  const tier = asTier(o.tier);
  if (!tier) return null;
  return {
    tier,
    orgId: typeof o.orgId === "string" ? o.orgId : null,
    citrateRole: typeof o.citrateRole === "string" ? o.citrateRole : undefined,
    milestone: typeof o.milestone === "string" ? o.milestone : undefined,
    expiresAt: typeof o.expiresAt === "number" ? o.expiresAt : null,
  };
}

/** Effective tier (honors expiry; absent ⇒ public). */
export function effectiveTier(ent: Entitlement | null | undefined, now: number = Date.now()): Tier {
  if (!ent) return "public";
  if (ent.expiresAt != null && now > ent.expiresAt) return "public";
  return ent.tier;
}

/** RBAC: does the entitlement meet `min` tier? (absent ⇒ public) */
export function requireTier(ent: Entitlement | null | undefined, min: Tier, now: number = Date.now()): boolean {
  return TIER_ORDER[effectiveTier(ent, now)] >= TIER_ORDER[min];
}

/** RBAC: does the entitlement carry `role`? */
export function requireRole(ent: Entitlement | null | undefined, role: string): boolean {
  return ent?.citrateRole === role;
}

/** Is KYC effectively verified (verified AND not past expiry)? */
export function isKycVerified(kyc: KycStatus, expiresAt?: string, now: number = Date.now()): boolean {
  if (kyc !== "verified") return false;
  if (expiresAt) {
    const exp = Date.parse(expiresAt);
    if (!Number.isNaN(exp) && exp <= now) return false;
  }
  return true;
}

/** The Account Hub URL ("Manage account / Upgrade") on the issuer. */
export function accountHubUrl(issuer: string, returnTo?: string): string {
  const base = issuer.replace(/\/+$/, "") + "/account";
  return returnTo ? `${base}?return_to=${encodeURIComponent(returnTo)}` : base;
}
