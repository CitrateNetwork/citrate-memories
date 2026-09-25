/**
 * Bounded, sanitized chat history for the grounded-Ask LLM route (PBA-L3c-015).
 *
 * The client supplies the whole `messages` array, so without bounds one request
 * can push an arbitrarily long history (and forged `system` / tool turns) into
 * the model on every call. We keep only what a grounded Q&A needs:
 *   - roles `user` and `assistant` (client `system` / `tool` turns are dropped —
 *     the server owns the system prompt);
 *   - `text` parts only (no client-forged tool calls/results, files, etc.);
 *   - the most recent {@link MAX_CHAT_MESSAGES} messages;
 * and refuse (413) a history whose kept text exceeds {@link MAX_CHAT_CHARS}.
 */
import type { UIMessage } from "ai";

export const MAX_CHAT_MESSAGES = 20;
export const MAX_CHAT_CHARS = 16_000;
/** Raw request-body ceiling, checked before JSON parsing. */
export const MAX_CHAT_BODY_BYTES = 256 * 1024;

type TextPart = { type: "text"; text: string };
export type SanitizedMessage = { id: string; role: "user" | "assistant"; parts: TextPart[] };
export type SanitizeResult =
  | { ok: true; messages: SanitizedMessage[] }
  | { ok: false; status: 400 | 413; error: string };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

export function sanitizeChatMessages(input: unknown): SanitizeResult {
  if (!Array.isArray(input)) return { ok: false, status: 400, error: "messages must be an array" };
  const kept: SanitizedMessage[] = [];
  for (const m of input) {
    if (!isObj(m) || (m.role !== "user" && m.role !== "assistant") || !Array.isArray(m.parts)) continue;
    const parts: TextPart[] = m.parts
      .filter((p): p is TextPart => isObj(p) && p.type === "text" && typeof p.text === "string")
      .map((p) => ({ type: "text", text: p.text }));
    if (!parts.length) continue;
    kept.push({ id: typeof m.id === "string" ? m.id : String(kept.length), role: m.role, parts });
  }
  const recent = kept.slice(-MAX_CHAT_MESSAGES);
  const chars = recent.reduce((n, m) => n + m.parts.reduce((k, p) => k + p.text.length, 0), 0);
  if (chars > MAX_CHAT_CHARS) {
    return { ok: false, status: 413, error: `chat history too large (${chars} > ${MAX_CHAT_CHARS} chars)` };
  }
  return { ok: true, messages: recent };
}

/** Cast for the AI SDK: a sanitized message is a valid text-only UIMessage. */
export const asUIMessages = (m: SanitizedMessage[]): UIMessage[] => m as unknown as UIMessage[];
