import "server-only";
import type { LanguageModel } from "ai";
import { createOpenAICompatible } from "@ai-sdk/openai-compatible";

/**
 * The Ask synthesis model — an env-driven OpenAI-compatible provider, the same
 * shape citrate-explorer uses. Today it points at a single self-hosted model: an
 * NVIDIA DGX `llama-server` fronted by `citrate-inference-gateway` (local-proxy
 * mode) behind a Digital Ocean Caddy front. When the network scales, model
 * selection will come from CID-pinned artifacts in the registry — at which point
 * only this file changes.
 *
 *   MEMRIZZ_INFERENCE_URL      e.g. https://infer.citrate.ai/v1  (or the DGX /v1)
 *   MEMRIZZ_INFERENCE_API_KEY  cgk_… (or omitted — llama-server ignores it)
 *   MEMRIZZ_MODEL              the model id, e.g. gemma-3-...-it-Q4_K_M
 *
 * Returns `null` when unconfigured, so `/api/chat` falls back to a retrieval-only
 * grounded answer rather than failing — "build for the finished state; the
 * inference endpoint may be warming up."
 */
export function inferenceModel(): LanguageModel | null {
  const baseURL = process.env.MEMRIZZ_INFERENCE_URL;
  const modelId = process.env.MEMRIZZ_MODEL;
  if (!baseURL || !modelId) return null;
  const client = createOpenAICompatible({
    name: "memrizz",
    baseURL,
    // openai-compatible requires *some* key string even when the upstream ignores it.
    apiKey: process.env.MEMRIZZ_INFERENCE_API_KEY ?? "not-needed",
  });
  return client(modelId);
}

/** Output budget — small by default since the DGX model has a modest context window. */
export const maxOutputTokens = Number(process.env.MEMRIZZ_MAX_OUTPUT_TOKENS) || 512;
