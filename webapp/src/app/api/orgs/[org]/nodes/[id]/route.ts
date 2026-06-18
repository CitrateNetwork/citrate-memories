import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** One node by id-prefix (Inspector). 404s if the caller can't read its tenant. */
export function GET(
  req: Request,
  { params }: { params: Promise<{ org: string; id: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org, id } = await params;
    return gateway.node(c, org, id);
  });
}
