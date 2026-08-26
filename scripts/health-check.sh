#!/usr/bin/env bash
# citrate-memories health + deploy-freshness check.
#
# One command that answers "is it up, and does prod match main?". Prints a status
# table and exits non-zero if any CRITICAL check fails, so it can drive a cron/CI
# monitor. Read-only: only GETs public endpoints and reads local git.
#
# Usage: scripts/health-check.sh            (human table)
#        scripts/health-check.sh --quiet    (only failures + exit code)
set -uo pipefail

GATEWAY="${MEM_GATEWAY_ORIGIN:-https://mem-gateway.citrate.ai}"
ISSUER="${OIDC_ISSUER:-https://auth.citrate.ai}"
WEBAPP="${MEMRIZZ_ORIGIN:-https://memrizz.citrate.ai}"
INFER="${INFER_ORIGIN:-https://infer.citrate.ai}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
QUIET="${1:-}"

fail=0; warn=0
code() { curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$@" 2>/dev/null || echo "000"; }
row() { # severity  name  detail  ok(0/1/2:warn)
  local sev="$1" name="$2" detail="$3" st="$4"
  local mark; case "$st" in 0) mark="OK  ";; 2) mark="WARN"; warn=$((warn+1));; *) mark="FAIL"; [ "$sev" = crit ] && fail=$((fail+1));; esac
  [ "$QUIET" = "--quiet" ] && [ "$st" = 0 ] && return
  printf '  [%s] %-26s %s\n' "$mark" "$name" "$detail"
}

echo "citrate-memories health — $GATEWAY"
echo "── liveness (CRITICAL) ──"
h=$(code "$GATEWAY/api/health");        [ "$h" = 200 ] && row crit "gateway /api/health" "$h" 0 || row crit "gateway /api/health" "$h" 1
g=$(code "$GATEWAY/api/orgs/citrate-federation/layout"); [ "$g" = 401 ] && row crit "gateway fail-closed" "401 (correct)" 0 || row crit "gateway fail-closed" "$g (expected 401)" 1
o=$(code "$ISSUER/.well-known/openid-configuration"); [ "$o" = 200 ] && row crit "OIDC discovery" "$o" 0 || row crit "OIDC discovery" "$o" 1
i=$(code "$INFER/v1/models");           [ "$i" = 401 ] || [ "$i" = 200 ] && row warn "inference gateway" "$i" 0 || row warn "inference gateway" "$i" 2
w=$(code "$WEBAPP");                     [ "$w" = 200 ] && row warn "memrizz webapp" "$w" 0 || row warn "memrizz webapp" "$w" 2

echo "── deploy freshness (does prod match main?) ──"
ct=$(code -X POST "$GATEWAY/connect/token")
if [ "$ct" = 404 ]; then row warn "/connect/token deployed" "404 — gateway NOT redeployed (PR #22 not live)" 2
else row warn "/connect/token deployed" "$ct — live" 0; fi

# SDK publish parity: is the published version >= the repo's declared version?
sdk_parity() { # name  repo_version  published_version
  if [ "$2" = "$3" ]; then row warn "$1 published" "$3 (matches repo)" 0
  else row warn "$1 published" "repo=$2 published=$3 — memory module NOT released" 2; fi
}
JS_REPO=$(grep -m1 '"version"' "$REPO/../citrate-sdk-js/package.json" 2>/dev/null | sed 's/.*"version" *: *"\([^"]*\)".*/\1/')
JS_PUB=$(curl -s --max-time 8 https://registry.npmjs.org/@citratelabs/sdk/latest 2>/dev/null | sed 's/.*"version":"\([^"]*\)".*/\1/')
[ -n "${JS_REPO:-}" ] && sdk_parity "npm @citratelabs/sdk" "$JS_REPO" "${JS_PUB:-none}"
PY_REPO=$(grep -m1 '^version' "$REPO/../citrate-sdk-python/pyproject.toml" 2>/dev/null | sed 's/.*= *"\([^"]*\)".*/\1/')
PY_PUB=$(curl -s --max-time 8 https://pypi.org/pypi/citrate-labs-sdk/json 2>/dev/null | sed 's/.*"version":"\([^"]*\)".*/\1/' | head -c 12)
[ -n "${PY_REPO:-}" ] && sdk_parity "pypi citrate-labs-sdk" "$PY_REPO" "${PY_PUB:-none}"

# Manifest pin drift: does the federation pin match memories main HEAD?
MAIN=$(git -C "$REPO" rev-parse --short=12 main 2>/dev/null || echo unknown)
PIN=$(grep -A6 '\[repos.citrate-memories\]' "$REPO/../citrate-federation/manifest.toml" 2>/dev/null | grep -m1 'rev =' | sed 's/.*"\(.*\)".*/\1/' | head -c 12)
if [ -n "${PIN:-}" ]; then
  [ "$PIN" = "$MAIN" ] && row warn "manifest pin" "matches main ($MAIN)" 0 || row warn "manifest pin" "pin=$PIN main=$MAIN — drift" 2
fi

echo
if [ "$fail" -gt 0 ]; then echo "RESULT: $fail CRITICAL failure(s), $warn warning(s)"; exit 1
elif [ "$warn" -gt 0 ]; then echo "RESULT: healthy; $warn warning(s) — see above"; exit 0
else echo "RESULT: all green"; exit 0; fi
