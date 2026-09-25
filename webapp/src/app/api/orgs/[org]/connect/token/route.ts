import { SignJWT } from "jose";
import { withCaller } from "@/lib/gateway/bff";
import { gateway, GatewayError } from "@/lib/gateway/client";

/**
 * BYOM connect (gap G-7 client side): mint a SHORT-LIVED, SCOPED connection token
 * for an MCP client. The token is a real signed JWT (HS256, `MEM_CONNECT_SECRET`)
 * bound to the caller's OIDC `sub` and carrying the tenants they may read — so it
 * reflects actual capability and expires in 15 min. The consuming side — the
 * gateway's MCP-over-HTTP endpoint that verifies this token and scopes every tool
 * call — is the next backend milestone (G-7); until it lands, this mints the
 * credential a client will use. Fail-closed: no secret → 503; no session → 401.
 */
const TTL_MINUTES = 15;

/**
 * PBA-L3c-032: the token is READ-ONLY by default (the connect UI only offers the
 * read tools). The gateway treats `propose` as write-capable, so a write token is
 * minted only when the caller explicitly asks for one (`{ "write": true }`); the
 * gateway still intersects it with the caller's membership. The `org` claim binds
 * the token to this org — the gateway refuses it anywhere else.
 */
async function wantsWrite(req: Request): Promise<boolean> {
  if (!(req.headers.get("content-type") ?? "").toLowerCase().startsWith("application/json")) return false;
  try {
    const body = (await req.json()) as { write?: unknown };
    return body?.write === true;
  } catch {
    return false;
  }
}

export function POST(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const secret = process.env.MEM_CONNECT_SECRET;
    if (!secret) {
      throw new GatewayError(503, "BYOM connect is not configured (MEM_CONNECT_SECRET unset).");
    }
    const { org } = await params;
    // Reflect real capability: the tenants this caller may read become the token's scope.
    let tenants: string[] = [];
    try {
      tenants = (await gateway.listTenants(c, org)).tenants;
    } catch {
      /* token still mints; scope is empty until the gateway is reachable */
    }
    const scope = (await wantsWrite(req)) ? "read,propose" : "read";
    const token = await new SignJWT({ sub: c.sub, org, tenants, scope })
      .setProtectedHeader({ alg: "HS256" })
      .setIssuedAt()
      .setExpirationTime(`${TTL_MINUTES}m`)
      .setSubject(c.sub)
      .sign(new TextEncoder().encode(secret));

    const base = process.env.NEXT_PUBLIC_MEM_MCP_ENDPOINT || "https://mem-gateway.citrate.ai/mcp";
    return {
      endpoint: `${base}/u/${encodeURIComponent(c.sub)}`,
      token,
      ttlMinutes: TTL_MINUTES,
      tenants,
      scope,
    };
  });
}
