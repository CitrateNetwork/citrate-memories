/**
 * Drizzle schema — the app's secure persistence (Neon Postgres). Every per-user
 * row is owned by the OIDC `subject` (`owner`) and scoped to an `org`; the
 * data-access layer never queries without both, so a row is reachable only by its
 * owner inside its Org. This module is the foundation for all app-state tables.
 */
import { index, jsonb, pgTable, text, timestamp } from "drizzle-orm/pg-core";
import type { UIMessage } from "ai";

export const conversations = pgTable(
  "conversations",
  {
    id: text("id").primaryKey(),
    /** OIDC subject — the canonical owner key (SR-0). Never the wallet. */
    owner: text("owner").notNull(),
    org: text("org").notNull(),
    title: text("title").notNull().default("Untitled"),
    /** The full UI-message thread (AI SDK v6 UIMessage[]). */
    messages: jsonb("messages").$type<UIMessage[]>().notNull().default([]),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [index("conversations_owner_org_idx").on(t.owner, t.org)],
);

export type ConversationRow = typeof conversations.$inferSelect;
