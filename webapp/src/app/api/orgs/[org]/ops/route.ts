import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Durability snapshot for an Org (admin-gated at the gateway): counts, checkpoints, anchors. */
export function GET(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    return gateway.ops(c, org);
  });
}
