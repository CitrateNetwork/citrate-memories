import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Propose a quarantined edge between two readable nodes (advisory until confirmed). */
export function POST(
  req: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    const body = (await req.json()) as { from: string; to: string; kind?: string; evidence?: string };
    return gateway.proposeEdge(c, org, body);
  });
}
