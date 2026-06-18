import { withCaller } from "@/lib/gateway/bff";
import { gateway } from "@/lib/gateway/client";

/** The Org's tamper-evident audit trail + integrity verdict (Org-scoped). */
export function GET(req: Request, { params }: { params: Promise<{ org: string }> }): Promise<Response> {
  return withCaller(req, async (c) => {
    const { org } = await params;
    const limit = Number(new URL(req.url).searchParams.get("limit")) || undefined;
    return gateway.audit(c, org, limit);
  });
}
