import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Append a signed assertion to a repo-tenant (write-scoped + audited at the gateway). */
export function POST(
  req: Request,
  { params }: { params: Promise<{ org: string; tenant: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org, tenant } = await params;
    const body = (await req.json()) as { content: string; kind?: string };
    return gateway.assert(c, org, tenant, body);
  });
}
