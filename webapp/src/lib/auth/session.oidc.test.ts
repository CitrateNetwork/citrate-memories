/**
 * OIDC relying-party verification seam — the gate for wiring Memrizz as a
 * registered RP of the Citrate identity authority.
 *
 * HERMETIC, headless proof that `verifySession` (oidc mode) verifies
 * authority-shaped ID tokens against a JWKS: mint an RS256 keypair with jose,
 * serve its public half over a real loopback JWKS endpoint, point the verifier at
 * it via the production env seam (OIDC_JWKS_URL / OIDC_ISSUER / OIDC_AUDIENCE),
 * and assert ACCEPT on valid + REJECT on wrong aud / wrong iss / bad sig /
 * expired / missing config.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { type CryptoKey, exportJWK, generateKeyPair, SignJWT } from "jose";

const ISSUER = "http://localhost:3000";
const AUDIENCE = "memrizz";
const WALLET = "0xF78C4B2091Ad55E0A7c0E3b8F4a1D9C62B0d2915"; // mixed case on purpose
const SUB = "did:citrate:user-1";
const KID = "citrate-authority-test-1";

let privateKey: CryptoKey;
let foreignKey: CryptoKey;
let jwksServer: Server;
let jwksUrl: string;
let jwksBody: { keys: Record<string, unknown>[] };

async function mintToken(
  opts: {
    iss?: string;
    aud?: string;
    sub?: string;
    wallet?: string;
    expSec?: number;
    signer?: CryptoKey;
  } = {},
): Promise<string> {
  const {
    iss = ISSUER,
    aud = AUDIENCE,
    sub = SUB,
    wallet = WALLET,
    expSec = 300,
    signer = privateKey,
  } = opts;
  const now = Math.floor(Date.now() / 1000);
  return new SignJWT({ wallet_address: wallet })
    .setProtectedHeader({ alg: "RS256", kid: KID })
    .setIssuer(iss)
    .setAudience(aud)
    .setSubject(sub)
    .setIssuedAt(now - 60)
    .setExpirationTime(now + expSec)
    .sign(signer);
}

function bearerReq(token: string): Request {
  return new Request("http://memrizz.local/api/whoami", {
    headers: { authorization: `Bearer ${token}` },
  });
}

async function loadVerifier() {
  vi.resetModules();
  process.env.NEXT_PUBLIC_AUTH_MODE = "oidc";
  process.env.OIDC_ISSUER = ISSUER;
  process.env.OIDC_AUDIENCE = AUDIENCE;
  process.env.OIDC_JWKS_URL = jwksUrl;
  return import("./session");
}

beforeAll(async () => {
  ({ privateKey } = await generateKeyPair("RS256", { extractable: true }));
  ({ privateKey: foreignKey } = await generateKeyPair("RS256", { extractable: true }));

  const jwk = await exportJWK(privateKey);
  const publicJwk = { kty: jwk.kty, n: jwk.n, e: jwk.e, alg: "RS256", use: "sig", kid: KID };
  jwksBody = { keys: [publicJwk] };

  jwksServer = createServer((_req, res) => {
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify(jwksBody));
  });
  await new Promise<void>((resolve) => jwksServer.listen(0, "127.0.0.1", () => resolve()));
  const { port } = jwksServer.address() as AddressInfo;
  jwksUrl = `http://127.0.0.1:${port}/jwks`;
});

afterAll(async () => {
  await new Promise<void>((resolve) => jwksServer.close(() => resolve()));
});

describe("OIDC RP verification — Memrizz ⇄ Citrate authority", () => {
  it("ACCEPTS a valid authority-shaped ID token (sub preserved, wallet lowercased)", async () => {
    const { verifySession, sessionAddress, sessionOwner } = await loadVerifier();
    const token = await mintToken();
    const s = await verifySession(bearerReq(token));

    expect(s.required).toBe(true);
    expect(s.authenticated).toBe(true);
    expect(s.sub).toBe(SUB);
    expect(s.walletAddress).toBe(WALLET.toLowerCase());
    expect(sessionAddress(s)).toBe(WALLET.toLowerCase());
    expect(sessionOwner(s)).toBe(SUB);
  });

  it("REJECTS a token with the wrong audience", async () => {
    const { verifySession } = await loadVerifier();
    const s = await verifySession(bearerReq(await mintToken({ aud: "some-other-rp" })));
    expect(s.authenticated).toBe(false);
  });

  it("REJECTS a token with the wrong issuer", async () => {
    const { verifySession } = await loadVerifier();
    const s = await verifySession(bearerReq(await mintToken({ iss: "https://evil.example" })));
    expect(s.authenticated).toBe(false);
  });

  it("REJECTS a token with a bad signature (foreign key)", async () => {
    const { verifySession } = await loadVerifier();
    const s = await verifySession(bearerReq(await mintToken({ signer: foreignKey })));
    expect(s.authenticated).toBe(false);
  });

  it("REJECTS an expired token", async () => {
    const { verifySession } = await loadVerifier();
    const s = await verifySession(bearerReq(await mintToken({ expSec: -120 })));
    expect(s.authenticated).toBe(false);
  });

  it("REJECTS when no Bearer credential is presented", async () => {
    const { verifySession } = await loadVerifier();
    const s = await verifySession(new Request("http://memrizz.local/api/whoami"));
    expect(s.required).toBe(true);
    expect(s.authenticated).toBe(false);
  });

  // FUA-EXPLORER-01: audience/issuer enforcement must not be optional.
  async function loadVerifierMissing(which: "aud" | "iss") {
    vi.resetModules();
    process.env.NEXT_PUBLIC_AUTH_MODE = "oidc";
    process.env.OIDC_JWKS_URL = jwksUrl;
    process.env.OIDC_ISSUER = ISSUER;
    process.env.OIDC_AUDIENCE = AUDIENCE;
    if (which === "aud") delete process.env.OIDC_AUDIENCE;
    if (which === "iss") delete process.env.OIDC_ISSUER;
    return import("./session");
  }

  it("FAILS CLOSED when OIDC_AUDIENCE is unset (no silent skip of aud check)", async () => {
    const { verifySession } = await loadVerifierMissing("aud");
    const s = await verifySession(bearerReq(await mintToken()));
    expect(s.authenticated).toBe(false);
  });

  it("FAILS CLOSED when OIDC_ISSUER is unset (no silent skip of iss check)", async () => {
    const { verifySession } = await loadVerifierMissing("iss");
    const s = await verifySession(bearerReq(await mintToken()));
    expect(s.authenticated).toBe(false);
  });
});
