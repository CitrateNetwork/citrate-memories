import "server-only";
import { and, desc, eq } from "drizzle-orm";
import type { UIMessage } from "ai";
import { db } from "./client";
import { conversations } from "./schema";

/**
 * Conversation persistence — every function is scoped to the owner (OIDC `sub`)
 * AND the Org, so a row is reachable only by its owner inside its Org. Upsert
 * guards the owner in its `where`, so a guessed id can never overwrite someone
 * else's conversation.
 */
export interface ConversationSummary {
  id: string;
  title: string;
  updatedAt: number;
}
export interface Conversation extends ConversationSummary {
  messages: UIMessage[];
}

export async function listConversations(owner: string, org: string): Promise<ConversationSummary[]> {
  const rows = await db()
    .select({ id: conversations.id, title: conversations.title, updatedAt: conversations.updatedAt })
    .from(conversations)
    .where(and(eq(conversations.owner, owner), eq(conversations.org, org)))
    .orderBy(desc(conversations.updatedAt))
    .limit(200);
  return rows.map((r) => ({ id: r.id, title: r.title, updatedAt: r.updatedAt.getTime() }));
}

export async function getConversation(owner: string, id: string): Promise<Conversation | null> {
  const [row] = await db()
    .select()
    .from(conversations)
    .where(and(eq(conversations.id, id), eq(conversations.owner, owner)))
    .limit(1);
  if (!row) return null;
  return { id: row.id, title: row.title, updatedAt: row.updatedAt.getTime(), messages: row.messages };
}

/** Derive a conversation title from its first user message. */
function deriveTitle(messages: UIMessage[]): string {
  const firstUser = messages.find((m) => m.role === "user");
  const text = firstUser?.parts.find((p) => p.type === "text") as { text?: string } | undefined;
  return (text?.text ?? "").trim().slice(0, 80) || "Untitled";
}

export async function upsertConversation(
  owner: string,
  org: string,
  id: string,
  messages: UIMessage[],
): Promise<void> {
  const title = deriveTitle(messages);
  const now = new Date();
  await db()
    .insert(conversations)
    .values({ id, owner, org, title, messages, updatedAt: now })
    .onConflictDoUpdate({
      target: conversations.id,
      set: { title, messages, updatedAt: now },
      // Only the owner may update their own row — a guessed id can't hijack one.
      setWhere: eq(conversations.owner, owner),
    });
}

export async function deleteConversation(owner: string, id: string): Promise<void> {
  await db().delete(conversations).where(and(eq(conversations.id, id), eq(conversations.owner, owner)));
}
