# citrate-memories — maintenance & monitoring regimen

The one command is `scripts/health-check.sh` (table + non-zero exit on any
CRITICAL failure). It checks liveness AND deploy-freshness (does prod match main),
so it catches both outages and the "merged ≠ deployed" drift.

## What to watch

| Check | Severity | Healthy | Meaning when it trips |
|---|---|---|---|
| `GET /api/health` | CRITICAL | 200 | gateway down |
| gated route without bearer | CRITICAL | 401 | **auth regression / fail-OPEN** — page immediately |
| OIDC discovery (auth.citrate.ai) | CRITICAL | 200 | logins broken |
| inference gateway `/v1/models` | warn | 401/200 | LLM-backed features degraded |
| memrizz webapp | warn | 200 | UI down |
| `POST /connect/token` | warn | not 404 | gateway not redeployed (mint unavailable) |
| SDK publish parity | warn | published == repo | memory module unreleased / version not bumped |
| manifest pin vs main | warn | equal | drift-check will flag; bump at next integration |

The fail-closed 401 is the highest-value signal: a gated route returning 200
without a bearer means the auth chokepoint regressed — treat as sev-1.

## Cadence

- **Liveness:** every 5–15 min (cron or a scheduled cloud agent running
  `health-check.sh --quiet`; alert only on exit≠0). Run it from the Mac Studio or
  a box with the sibling repos checked out (the pin/SDK checks read local repos).
- **Deploy-freshness + parity:** the same run covers it; review the warnings
  weekly even when liveness is green — that's where "shipped but not deployed"
  hides.
- **Audit chain + store integrity:** monthly — verify the gateway's audit chain
  loads clean on restart, and that ingestion watermarks are advancing (the
  reconciler sweeps every `MEM_INGEST_RECONCILE_SECS`; a stalled watermark means a
  wedged worker).

## Failure runbook (first move per row)

- **gateway /api/health ≠ 200** → check the `mem-gateway` systemd unit on the DGX
  (via Mac Studio); `journalctl -u mem-gateway`. Store lock or OOM are the usual
  causes.
- **fail-closed ≠ 401** → sev-1. Confirm `OIDC_ISSUER/AUDIENCE/JWKS` are set on the
  unit; a missing verifier must fail closed, not open. Roll back if a recent deploy
  introduced it.
- **OIDC discovery down** → auth.citrate.ai issue; memory logins block but reads via
  connect-token still work.
- **/connect/token 404** → expected until the gateway is redeployed with the mint
  endpoint. Rebuild + restart the unit from the Mac Studio.
- **SDK parity drift** → bump the package version, publish npm/PyPI, re-run.
- **manifest pin drift** → `citrate-federation/scripts/` pin-bump to main at the
  next integration point (pins need not chase every commit).

## Maintenance tasks (periodic)

- **Weekly:** run the health check; review deploy-freshness warnings; skim
  `cargo test --workspace` on main (CI should already gate this).
- **Monthly:** `cargo update` + `cargo audit` (dependency CVEs); verify the audit
  chain; confirm store backfill freshness (ingest watermark vs repo HEADs).
- **On engine change:** bump the affected crate/SDK versions; re-run the OSS
  export dry-run so the public tree stays buildable; bump the federation pin.
- **Key/secret hygiene:** `MEM_CONNECT_SECRET` rotation invalidates all connect
  tokens (users re-run `citrate-agent connect`); `MEM_GATEWAY_ASSERTER_SEED` must
  stay stable (rotating it changes write-author identity). JWKS re-fetch on rotation.

## Note

`health-check.sh` is intentionally NOT in the public allowlist — it names internal
hostnames and stays private. Endpoints are overridable via env
(`MEM_GATEWAY_ORIGIN`, `OIDC_ISSUER`, `MEMRIZZ_ORIGIN`, `INFER_ORIGIN`).
