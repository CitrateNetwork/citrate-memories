import { withOwner } from "@/lib/gateway/bff";
import { deleteConversation, getConversation } from "@/lib/db/conversations";

/** One conversation (with its messages) — only if the caller owns it. */
export function GET(req: Request, { params }: { params: Promise<{ id: string }> }): Promise<Response> {
  return withOwner(req, async (owner) => {
    const { id } = await params;
    return getConversation(owner, id); // null when absent / not owned
  });
}

/** Delete one of the caller's conversations. */
export function DELETE(req: Request, { params }: { params: Promise<{ id: string }> }): Promise<Response> {
  return withOwner(req, async (owner) => {
    const { id } = await params;
    await deleteConversation(owner, id);
    return { ok: true };
  });
}
