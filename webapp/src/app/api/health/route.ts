import { gateway, GatewayError } from "@/lib/gateway/client";

/**
 * Gateway liveness — no auth (no Org data crosses it), fail closed on misconfig.
 * PBA-L3c-038: unauthenticated, so it reports only up/down + status, never the
 * underlying error text (which named config such as MEM_GATEWAY_ORIGIN).
 */
export async function GET(): Promise<Response> {
  try {
    const h = await gateway.health();
    return Response.json({ ok: true, gateway: h });
  } catch (e) {
    const status = e instanceof GatewayError ? e.status : 502;
    return Response.json({ ok: false, error: "gateway unavailable" }, { status });
  }
}
