import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Promote a quarantined proposal to load-bearing (HIC confirm). */
export function POST(
  req: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    const body = (await req.json()) as { from: string; to: string; kind: string };
    return gateway.confirmEdge(c, org, body);
  });
}
