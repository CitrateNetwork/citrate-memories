import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Repo-tenants in an Org the caller may read. */
export function GET(
  req: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    return gateway.listTenants(c, org);
  });
}
