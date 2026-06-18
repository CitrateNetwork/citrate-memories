/**
 * Conversation persistence routes — the security property: every route is
 * fail-closed (401 without a session) and scopes EVERY query to the authenticated
 * OIDC `sub`, so a caller can only touch their own conversations. The DB layer is
 * mocked (no Postgres needed) — what's under test is the owner-scoping wiring.
 */
import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/db/conversations", () => ({
  listConversations: vi.fn(async () => [{ id: "c1", title: "first", updatedAt: 2 }]),
  getConversation: vi.fn(async () => null),
  upsertConversation: vi.fn(async () => undefined),
  deleteConversation: vi.fn(async () => undefined),
}));

const OWNER = "did:citrate:aleia";
function token(sub: string): string {
  return Buffer.from(JSON.stringify({ sub })).toString("base64url");
}
function authHeaders(): Record<string, string> {
  return { authorization: `Bearer ${token(OWNER)}`, "content-type": "application/json" };
}

async function loadMock() {
  process.env.NEXT_PUBLIC_AUTH_MODE = "mock";
  vi.resetModules();
  const conv = await import("@/lib/db/conversations");
  return conv;
}

describe("conversation routes — owner-scoped + fail-closed", () => {
  it("401 without a session", async () => {
    await loadMock();
    const { GET } = await import("../../app/api/conversations/route");
    expect((await GET(new Request("http://app/api/conversations"))).status).toBe(401);
  });

  it("list scopes to the authenticated owner + requested org", async () => {
    const conv = await loadMock();
    const { GET } = await import("../../app/api/conversations/route");
    const res = await GET(new Request("http://app/api/conversations?org=acme", { headers: authHeaders() }));
    expect(res.status).toBe(200);
    expect(conv.listConversations).toHaveBeenCalledWith(OWNER, "acme");
  });

  it("upsert + get + delete all key on the authenticated owner", async () => {
    const conv = await loadMock();
    const list = await import("../../app/api/conversations/route");
    await list.POST(new Request("http://app/api/conversations", { method: "POST", headers: authHeaders(), body: JSON.stringify({ id: "c1", org: "acme", messages: [] }) }));
    expect(conv.upsertConversation).toHaveBeenCalledWith(OWNER, "acme", "c1", []);

    const byId = await import("../../app/api/conversations/[id]/route");
    const params = { params: Promise.resolve({ id: "c1" }) };
    await byId.GET(new Request("http://app/api/conversations/c1", { headers: authHeaders() }), params);
    expect(conv.getConversation).toHaveBeenCalledWith(OWNER, "c1");

    await byId.DELETE(new Request("http://app/api/conversations/c1", { method: "DELETE", headers: authHeaders() }), params);
    expect(conv.deleteConversation).toHaveBeenCalledWith(OWNER, "c1");
  });
});
