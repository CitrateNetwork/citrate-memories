import { describe, it, expect } from "vitest";
import { POST, GET, DELETE } from "./route";
import { ID_COOKIE } from "@/lib/auth/cookies";

/** A syntactically-valid JWT (header.payload.sig) with the given payload object. */
function jwt(payload: Record<string, unknown>): string {
  const seg = (o: unknown) => Buffer.from(JSON.stringify(o)).toString("base64url");
  return `${seg({ alg: "none" })}.${seg(payload)}.sig`;
}

function postReq(body: unknown): Request {
  return new Request("https://app/api/auth/session", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

describe("POST /api/auth/session", () => {
  it("sets an httpOnly SameSite=Strict id cookie for a well-formed JWT", async () => {
    const res = await POST(postReq({ id_token: jwt({ sub: "u1", exp: 9999999999 }) }));
    expect(res.status).toBe(200);
    const cookie = res.headers.get("set-cookie") ?? "";
    expect(cookie).toContain(`${ID_COOKIE}=`);
    expect(cookie).toContain("HttpOnly");
    expect(cookie).toContain("SameSite=Strict");
  });

  it("rejects a non-JWT credential in a verifying mode (fail closed)", async () => {
    const res = await POST(postReq({ id_token: "not-a-jwt" }));
    expect(res.status).toBe(400);
  });

  it("rejects an already-expired token", async () => {
    const res = await POST(postReq({ id_token: jwt({ sub: "u1", exp: 100 }) }));
    expect(res.status).toBe(400);
  });

  it("rejects a missing id_token", async () => {
    const res = await POST(postReq({}));
    expect(res.status).toBe(400);
  });

  it("rejects invalid JSON", async () => {
    const res = await POST(
      // Declared JSON (PBA-L3c-031 refuses undeclared/text bodies with 415 before
      // parsing — see pba-l3c-031.test.ts); the malformed body is still a 400.
      new Request("https://app/api/auth/session", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{",
      }),
    );
    expect(res.status).toBe(400);
  });
});

describe("GET /api/auth/session", () => {
  it("reports unauthenticated with no cookie (oidc fail-closed default)", async () => {
    const res = await GET(new Request("https://app/api/auth/session"));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { authenticated: boolean };
    expect(body.authenticated).toBe(false);
  });
});

describe("DELETE /api/auth/session", () => {
  it("clears the cookie (Max-Age=0)", async () => {
    const res = await DELETE();
    expect(res.status).toBe(200);
    const cookie = res.headers.get("set-cookie") ?? "";
    expect(cookie).toContain(`${ID_COOKIE}=`);
    expect(cookie).toContain("Max-Age=0");
  });
});
