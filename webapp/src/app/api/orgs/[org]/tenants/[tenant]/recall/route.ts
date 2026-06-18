import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** A tenant's storyline (newest-first), budget-shaped. Feeds Ask + storyline. */
export function GET(
  req: Request,
  { params }: { params: Promise<{ org: string; tenant: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org, tenant } = await params;
    const budget = Number(new URL(req.url).searchParams.get("budget")) || undefined;
    return gateway.recall(c, org, tenant, budget);
  });
}
