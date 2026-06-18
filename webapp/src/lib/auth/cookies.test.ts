/**
 * FUA-EXPLORER-04 — httpOnly auth-cookie plumbing. Covers the serializer
 * attributes (HttpOnly + SameSite=Strict are the load-bearing properties), the
 * request-cookie reader, and `tokenFromRequest` precedence (header wins, cookie
 * is the fallback transport for browsers).
 */
import { describe, expect, it } from "vitest";
import {
  ID_COOKIE,
  ACCESS_COOKIE,
  cookieValue,
  decodeJwtPayload,
  looksLikeJwt,
  serializeAuthCookie,
  clearAuthCookie,
} from "./cookies";
import { tokenFromRequest } from "./session";

function b64url(obj: unknown): string {
  return Buffer.from(JSON.stringify(obj)).toString("base64url");
}
const FAKE_JWT = `${b64url({ alg: "RS256" })}.${b64url({ sub: "user-1", exp: 99 })}.sig`;

describe("auth cookie serialization", () => {
  it("sets HttpOnly + SameSite=Strict + Path=/ + Max-Age", () => {
    const c = serializeAuthCookie(ID_COOKIE, FAKE_JWT, 3600);
    expect(c).toContain(`${ID_COOKIE}=`);
    expect(c).toContain("HttpOnly");
    expect(c).toContain("SameSite=Strict");
    expect(c).toContain("Path=/");
    expect(c).toContain("Max-Age=3600");
  });

  it("clearAuthCookie expires immediately and stays HttpOnly", () => {
    const c = clearAuthCookie(ACCESS_COOKIE);
    expect(c).toContain("Max-Age=0");
    expect(c).toContain("HttpOnly");
    expect(c).toContain("SameSite=Strict");
  });

  it("floors negative max-age to 0", () => {
    expect(serializeAuthCookie(ID_COOKIE, FAKE_JWT, -50)).toContain("Max-Age=0");
  });
});

describe("cookieValue", () => {
  it("reads a named cookie among several", () => {
    const req = new Request("http://x/", {
      headers: { cookie: `a=1; ${ID_COOKIE}=${encodeURIComponent(FAKE_JWT)}; b=2` },
    });
    expect(cookieValue(req, ID_COOKIE)).toBe(FAKE_JWT);
  });

  it("returns null when absent or empty", () => {
    expect(cookieValue(new Request("http://x/"), ID_COOKIE)).toBeNull();
    const req = new Request("http://x/", { headers: { cookie: `${ID_COOKIE}=` } });
    expect(cookieValue(req, ID_COOKIE)).toBeNull();
  });
});

describe("JWT shape helpers", () => {
  it("accepts a three-segment base64url token, rejects junk and oversized", () => {
    expect(looksLikeJwt(FAKE_JWT)).toBe(true);
    expect(looksLikeJwt("not a jwt")).toBe(false);
    expect(looksLikeJwt("a.b")).toBe(false);
    expect(looksLikeJwt("x".repeat(9000) + ".y.z")).toBe(false);
  });

  it("decodes the payload without verifying", () => {
    expect(decodeJwtPayload(FAKE_JWT)).toEqual({ sub: "user-1", exp: 99 });
    expect(decodeJwtPayload("garbage")).toBeNull();
  });
});

describe("tokenFromRequest (verifySession transport)", () => {
  it("prefers the Authorization header", () => {
    const req = new Request("http://x/", {
      headers: { authorization: "Bearer header-token", cookie: `${ID_COOKIE}=cookie-token` },
    });
    expect(tokenFromRequest(req)).toBe("header-token");
  });

  it("falls back to the httpOnly session cookie", () => {
    const req = new Request("http://x/", { headers: { cookie: `${ID_COOKIE}=cookie-token` } });
    expect(tokenFromRequest(req)).toBe("cookie-token");
  });

  it("returns null with neither", () => {
    expect(tokenFromRequest(new Request("http://x/"))).toBeNull();
  });
});
