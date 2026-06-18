import { withOwner } from "@/lib/gateway/bff";
import { listConversations, upsertConversation } from "@/lib/db/conversations";
import type { UIMessage } from "ai";

/** The caller's conversations in an Org (owner-scoped; newest first). */
export function GET(req: Request): Promise<Response> {
  return withOwner(req, (owner) => {
    const org = new URL(req.url).searchParams.get("org") || "citrate-federation";
    return listConversations(owner, org);
  });
}

/** Upsert one of the caller's conversations (owner-guarded — can't hijack an id). */
export function POST(req: Request): Promise<Response> {
  return withOwner(req, async (owner) => {
    const { id, org, messages } = (await req.json()) as { id: string; org: string; messages: UIMessage[] };
    await upsertConversation(owner, org || "citrate-federation", id, messages ?? []);
    return { ok: true };
  });
}
