#!/usr/bin/env bash
# Build a curated PUBLIC release tree from oss/public-allowlist.txt.
#
# Fail-closed: iterates only git-TRACKED files, and copies one only if it matches
# an allowlist entry. Untracked junk (target/, data/, snapshots) and everything
# not on the allowlist (webapp/, PLANSET/, handoffs/, audits/, mutants.out*,
# internal docs) can never reach the output. Produces a FRESH-history git repo —
# your private citrate-memories, its history, and its DAG stay untouched.
#
# Usage: scripts/build-public-release.sh [OUT_DIR]
#   OUT_DIR defaults to ../citrate-memories-public
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOW="$REPO/oss/public-allowlist.txt"
PUBLIC_README="$REPO/oss/README.public.md"
OUT="${1:-$REPO/../citrate-memories-public}"

[ -f "$ALLOW" ] || { echo "missing allowlist: $ALLOW" >&2; exit 1; }
[ -f "$PUBLIC_README" ] || { echo "missing public README: $PUBLIC_README" >&2; exit 1; }

# Load allowlist into two arrays: exact files and dir prefixes (entries ending /).
files=(); dirs=()
while IFS= read -r line; do
  line="${line%%#*}"; line="${line//[$'\t\r']/}"; line="$(echo "$line" | xargs || true)"
  [ -z "$line" ] && continue
  if [[ "$line" == */ ]]; then dirs+=("$line"); else files+=("$line"); fi
done < "$ALLOW"

allowed() {
  local f="$1"
  local d p
  for p in "${files[@]}"; do [ "$f" == "$p" ] && return 0; done
  for d in "${dirs[@]}"; do [[ "$f" == "$d"* ]] && return 0; done
  return 1
}

echo "building public tree at: $OUT"
rm -rf "$OUT"; mkdir -p "$OUT"

count=0
# git ls-files = tracked only. README.md is deliberately NOT allowlisted; we
# substitute the standalone public README below.
while IFS= read -r f; do
  allowed "$f" || continue
  mkdir -p "$OUT/$(dirname "$f")"
  cp "$REPO/$f" "$OUT/$f"
  count=$((count + 1))
done < <(git -C "$REPO" ls-files)

cp "$PUBLIC_README" "$OUT/README.md"
echo "copied $count allowlisted files (+ substituted public README)"

# Fresh history — no private commits carry over.
git -C "$OUT" init -q
git -C "$OUT" add -A
git -C "$OUT" -c commit.gpgsign=false commit -q -m "citrate-memories — initial public release"

# Leak backstop.
echo "--- leak scan ---"
resid=0
for v in "100.68.173.64" "142.93.58.145" "spark-2e01" "/home/saul" "citrate-rpc-1"; do
  n=$(grep -rIl "$v" "$OUT" 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" != "0" ] && { echo "LEAK: '$v' in $n file(s)"; resid=1; }
done
if command -v gitleaks >/dev/null 2>&1; then
  (cd "$OUT" && gitleaks detect --no-banner --config .gitleaks.toml) || resid=1
else
  echo "(gitleaks not installed — run it on $OUT before publishing)"
fi
[ "$resid" == "0" ] && echo "leak scan: clean" || { echo "LEAK SCAN FAILED — do not publish" >&2; exit 1; }

echo
echo "Public tree ready at: $OUT"
echo "Review it, then publish to a NEW public repo:"
echo "  cd $OUT && git remote add origin <PUBLIC_REPO_URL> && git push -u origin main"
