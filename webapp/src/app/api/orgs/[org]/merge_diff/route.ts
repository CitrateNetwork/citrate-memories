import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Merge a signed memory-diff (the "git for agents" session handoff). The body is
 *  the serialized MemoryDiff JSON, forwarded raw to the write-gated gateway route. */
export function POST(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    const diffJson = await req.text();
    return gateway.mergeDiff(c, org, diffJson);
  });
}
