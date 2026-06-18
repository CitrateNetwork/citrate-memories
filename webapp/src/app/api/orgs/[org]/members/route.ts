import { withCaller } from "@/lib/gateway/bff";
import { gateway, type ScopeInput } from "@/lib/gateway/client";

/** The Org roster + delegation tree (gap G-4). */
export function GET(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    return gateway.listMembers(c, org);
  });
}

/** Onboard / update a member under the caller (attenuation-gated at the gateway). */
export function POST(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    const body = (await req.json()) as { sub: string; role: string; scopes?: ScopeInput[] };
    return gateway.addMember(c, org, body);
  });
}
