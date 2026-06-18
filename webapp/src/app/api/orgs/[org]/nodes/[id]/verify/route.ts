import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Trust verdict for a node — signature/supersession/contradiction posture. */
export function GET(
  req: Request,
  { params }: { params: Promise<{ org: string; id: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org, id } = await params;
    return gateway.verify(c, org, id);
  });
}
