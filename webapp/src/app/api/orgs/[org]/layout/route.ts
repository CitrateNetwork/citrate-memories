import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** The 3D constellation scene for an Org (feeds the engine, WP-7.4). */
export function GET(
  req: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    return gateway.layout(c, org);
  });
}
