import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** Revoke a member and everyone delegated beneath them (F-7 cascade, gap G-4). */
export function DELETE(
  req: Request,
  { params }: { params: Promise<{ org: string; sub: string }> },
): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org, sub } = await params;
    return gateway.revokeMember(c, org, sub);
  });
}
