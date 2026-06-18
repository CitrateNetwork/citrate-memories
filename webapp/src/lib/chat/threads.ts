"use client";

/**
 * Conversation persistence — SERVER-backed (Neon Postgres via `/api/conversations`).
 * No localStorage: conversations live in the secure DB, owned by the OIDC `sub` and
 * scoped to the Org, so they're durable, owner-isolated, and consistent across the
 * app. This module is the thin client over those owner-scoped routes.
 */
import type { UIMessage } from "ai";

export interface ConversationSummary {
  id: string;
  title: string;
  updatedAt: number;
}
export interface Conversation extends ConversationSummary {
  messages: UIMessage[];
}

export function newThreadId(): string {
  if (typeof window !== "undefined" && window.crypto?.randomUUID) return window.crypto.randomUUID();
  return "t-" + Math.random().toString(36).slice(2) + Date.now().toString(36);
}

export async function loadThreads(org: string): Promise<ConversationSummary[]> {
  try {
    const r = await fetch(`/api/conversations?org=${encodeURIComponent(org)}`, { cache: "no-store" });
    return r.ok ? ((await r.json()) as ConversationSummary[]) : [];
  } catch {
    return [];
  }
}

export async function loadThread(id: string): Promise<Conversation | null> {
  try {
    const r = await fetch(`/api/conversations/${encodeURIComponent(id)}`, { cache: "no-store" });
    if (!r.ok) return null;
    return (await r.json()) as Conversation | null;
  } catch {
    return null;
  }
}

export async function saveThread(org: string, id: string, messages: UIMessage[]): Promise<void> {
  try {
    await fetch("/api/conversations", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id, org, messages }),
    });
  } catch {
    /* best-effort persistence */
  }
}

export async function deleteThread(id: string): Promise<void> {
  try {
    await fetch(`/api/conversations/${encodeURIComponent(id)}`, { method: "DELETE" });
  } catch {
    /* best-effort */
  }
}
