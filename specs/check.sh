#!/usr/bin/env bash
# Model-check every citrate-memories TLA+ spec with TLC.
#
# Requires a JRE and tla2tools.jar. Point at the jar with $TLA_TOOLS, or drop it
# in this directory. In CI we install both and run this as the WP-0.5 gate; a
# failed invariant fails the build (the one place we hard-block, complementing
# the soft trailer nudge).
set -euo pipefail
cd "$(dirname "$0")"

SPECS=(SupersededDag Ingestion Authz)

# Locate tla2tools.jar.
JAR="${TLA_TOOLS:-}"
if [[ -z "${JAR}" ]]; then
  for cand in ./tla2tools.jar "$HOME/.local/share/tla/tla2tools.jar"; do
    [[ -f "$cand" ]] && JAR="$cand" && break
  done
fi

if ! command -v java >/dev/null 2>&1; then
  echo "ERROR: no Java runtime found. Install a JRE (e.g. 'brew install temurin')." >&2
  echo "       Then re-run: ./specs/check.sh" >&2
  exit 2
fi
if [[ -z "${JAR}" || ! -f "${JAR}" ]]; then
  echo "ERROR: tla2tools.jar not found. Set TLA_TOOLS=/path/to/tla2tools.jar" >&2
  echo "       Download: https://github.com/tlaplus/tlaplus/releases" >&2
  exit 2
fi

rc=0
for spec in "${SPECS[@]}"; do
  echo "=== TLC: ${spec} ==="
  if java -XX:+UseParallelGC -cp "${JAR}" tlc2.TLC -config "${spec}.cfg" "${spec}.tla"; then
    echo "--- ${spec}: OK"
  else
    echo "--- ${spec}: FAILED" >&2
    rc=1
  fi
done
exit "${rc}"
