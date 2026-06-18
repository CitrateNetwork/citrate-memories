import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Blast-radius neighbours of a node (Inspector). Cross-tenant hidden by grant. */
export function GET(
  req: Request,
  { params }: { params: Promise<{ org: string; id: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org, id } = await params;
    const budget = Number(new URL(req.url).searchParams.get("budget")) || undefined;
    return gateway.neighbors(c, org, id, budget);
  });
}
