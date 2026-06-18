import {
  convertToModelMessages,
  createUIMessageStream,
  createUIMessageStreamResponse,
  streamText,
  type UIMessage,
} from "ai";
import { requireOwner, tokenFromRequest } from "@/lib/auth/session";
import { checkRateLimit, clientIp } from "@/lib/api/ratelimit";
import { gateway, GatewayError, type GatewayCaller } from "@/lib/gateway/client";
import { inferenceModel, maxOutputTokens } from "@/lib/ai/provider";
import type { RecallItem } from "@/lib/gateway/types";

/**
 * Grounded Ask — token-by-token streaming (WP-7.6 streaming). Every answer is
 * grounded in REAL memories pulled from the gateway and streamed back as an AI-SDK
 * v6 UI-message stream:
 *  - a `data-citations` part (the cited memories — each flies to its node), then
 *  - the answer text, streamed live from the configured model (the DGX via the
 *    inference gateway). When no model is configured (or it's warming up), a single
 *    retrieval-grounded message is streamed instead — never fabricated.
 * Auth + per-account rate limit are enforced inline (the response is a stream, not
 * the JSON the shared `withCaller` returns).
 */
export const maxDuration = 300; // cold DGX/CPU inference can be slow

const READ_RATE_PER_SEC = Number(process.env.MEM_BFF_RATE_PER_SEC) || 12;

interface Citation {
  id: string;
  title: string;
  kind: string;
  trust: string;
  repo: string;
}

const textOf = (m: UIMessage): string =>
  m.parts.filter((p): p is { type: "text"; text: string } => p.type === "text").map((p) => p.text).join(" ");

function citationsFrom(items: RecallItem[]): Citation[] {
  return items.slice(0, 6).map((i) => ({ id: i.id, title: i.title, kind: i.kind, trust: i.trust, repo: i.repo }));
}

async function retrieve(c: GatewayCaller, org: string, tenant: string | undefined, question: string): Promise<{ items: RecallItem[]; tenant?: string }> {
  let theTenant = tenant;
  if (!theTenant) {
    try {
      theTenant = (await gateway.listTenants(c, org)).tenants[0];
    } catch {
      return { items: [] };
    }
  }
  if (!theTenant) return { items: [] };
  try {
    return { items: (await gateway.search(c, org, theTenant, question, 6)).items, tenant: theTenant };
  } catch (e) {
    if (e instanceof GatewayError && e.status === 503) {
      return { items: (await gateway.recall(c, org, theTenant, 6)).items, tenant: theTenant };
    }
    return { items: [], tenant: theTenant };
  }
}

export async function POST(req: Request): Promise<Response> {
  const sub = await requireOwner(req);
  if (!sub) return Response.json({ error: "unauthenticated" }, { status: 401 });
  const rl = await checkRateLimit(`bff:${sub}:${clientIp(req)}`, READ_RATE_PER_SEC);
  if (!rl.ok) return Response.json({ error: "rate_limited" }, { status: 429 });

  const caller: GatewayCaller = { sub, token: tokenFromRequest(req) };
  const { messages = [], org, tenant } = (await req.json()) as { messages?: UIMessage[]; org?: string; tenant?: string };
  const theOrg = org || "citrate-federation";
  const lastUser = [...messages].reverse().find((m) => m.role === "user");
  const question = lastUser ? textOf(lastUser) : "";

  const { items, tenant: theTenant } = await retrieve(caller, theOrg, tenant, question);
  const citations = citationsFrom(items);
  const model = inferenceModel();

  const stream = createUIMessageStream({
    execute: async ({ writer }) => {
      if (citations.length) {
        writer.write({ type: "data-citations", data: citations });
      }
      if (model && citations.length) {
        const context = items
          .slice(0, 6)
          .map((m, i) => `[${i + 1}] (${m.kind} · ${m.repo} · trust:${m.trust}) ${m.title}`)
          .join("\n");
        const result = streamText({
          model,
          maxOutputTokens,
          temperature: 0.3,
          system:
            "You are Memrizz, an organization's memory. Answer ONLY from the numbered memories provided. " +
            "Be concise and plain-spoken. If the memories don't cover the question, say so plainly. " +
            "Never invent facts, repos, or decisions that aren't in the memories. Do not output the bracketed list.\n\n" +
            `Memories from ${theTenant}:\n${context}`,
          messages: await convertToModelMessages(messages),
        });
        writer.merge(result.toUIMessageStream());
      } else {
        const id = "grounded";
        const text = citations.length
          ? `Grounded in ${citations.length} ${citations.length === 1 ? "memory" : "memories"} from ${theTenant}. ` +
            `The citations below fly to their nodes — click to inspect how trustworthy each is. ` +
            `(Connect a model — set MEMRIZZ_INFERENCE_URL — to stream a synthesized answer over these.)`
          : `No memories in ${theTenant ?? "this org"} matched that yet.`;
        writer.write({ type: "text-start", id });
        writer.write({ type: "text-delta", id, delta: text });
        writer.write({ type: "text-end", id });
      }
    },
  });

  return createUIMessageStreamResponse({ stream });
}
