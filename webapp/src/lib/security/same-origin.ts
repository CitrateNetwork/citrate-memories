/**
 * Same-origin guard for cookie-setting / cookie-authenticated POSTs
 * (PBA-L3c-031, login-CSRF).
 *
 * A cross-site page can POST a `text/plain` body (no CORS preflight) that
 * `req.json()` happily parses, so `/api/auth/session` would set the ATTACKER's
 * session cookie in the victim's browser and the victim's conversations would
 * then persist under the attacker's `sub`. Browsers always attach `Origin` (and
 * modern ones `Sec-Fetch-Site`) to cross-origin POSTs, so:
 *   - `Sec-Fetch-Site`, when present, must be `same-origin` (or `none`, a
 *     user-initiated navigation);
 *   - `Origin`, when present, must equal this request's own origin (or one listed
 *     in `MEMRIZZ_APP_ORIGINS`, comma-separated, for a proxied deploy);
 *   - the body must be declared `application/json` (forces a CORS preflight for
 *     any cross-origin sender).
 * Non-browser callers that send neither header are unaffected: they cannot carry
 * a victim's cookies.
 */
export type OriginVerdict = { ok: true } | { ok: false; status: 403 | 415; error: string };

function allowedOrigins(req: Request): Set<string> {
  const set = new Set<string>();
  try {
    set.add(new URL(req.url).origin);
  } catch {
    /* no parseable own origin — only the configured list applies */
  }
  for (const o of (process.env.MEMRIZZ_APP_ORIGINS ?? "").split(",")) {
    const t = o.trim().replace(/\/$/, "");
    if (t) set.add(t);
  }
  return set;
}

export function checkSameOriginJson(req: Request): OriginVerdict {
  const site = req.headers.get("sec-fetch-site");
  if (site && site !== "same-origin" && site !== "none") {
    return { ok: false, status: 403, error: "cross-site request refused" };
  }
  const origin = req.headers.get("origin");
  if (origin && !allowedOrigins(req).has(origin)) {
    return { ok: false, status: 403, error: "cross-origin request refused" };
  }
  const ct = (req.headers.get("content-type") ?? "").split(";")[0].trim().toLowerCase();
  if (ct !== "application/json") {
    return { ok: false, status: 415, error: "content-type must be application/json" };
  }
  return { ok: true };
}
