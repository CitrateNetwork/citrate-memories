import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Create a recovery checkpoint now (admin-gated + audited at the gateway). */
export function POST(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    const keep = Number(new URL(req.url).searchParams.get("keep")) || undefined;
    return gateway.createCheckpoint(c, org, keep);
  });
}
