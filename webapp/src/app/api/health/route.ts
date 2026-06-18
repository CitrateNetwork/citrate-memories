import { gateway, GatewayError } from "@/lib/gateway/client";

/** Gateway liveness — no auth (no Org data crosses it), fail closed on misconfig. */
export async function GET(): Promise<Response> {
  try {
    const h = await gateway.health();
    return Response.json({ ok: true, gateway: h });
  } catch (e) {
    const status = e instanceof GatewayError ? e.status : 502;
    return Response.json({ ok: false, error: String((e as Error).message) }, { status });
  }
}
