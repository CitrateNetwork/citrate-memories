import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** The Orgs the caller belongs to (Org resolution — planset §3.B). */
export function GET(req: Request): Promise<Response> {
  return withCaller(req, (c) => gateway.listOrgs(c));
}

/** Create a new Org (self-serve): inits its isolated store + binds the caller as Owner. */
export function POST(req: Request): Promise<Response> {
  return withCaller(req, async (c) => {
    const body = (await req.json()) as { name: string; id?: string };
    return gateway.createOrg(c, body);
  });
}
