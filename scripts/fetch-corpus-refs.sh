#!/usr/bin/env bash
# HUP-S3.1: fetch the optional reference sources of the Hermes knowledge corpus
# (corpus/hermes-knowledge.toml) at pinned commits, so a release build host has
# every cleared source and the build records the same commits every time.
#
#   scripts/fetch-corpus-refs.sh <sources-base>
#
# Clones into <sources-base>/<name>. An existing checkout is accepted only when
# its HEAD is the pinned commit and its tree is clean; anything else is an error
# (the script never moves or deletes an existing checkout). Medusa and Slither
# docs are not fetched: their licence is not cleared (pending owner sign-off).
# Changing a pin is a reviewed change to this file.
set -euo pipefail

base="${1:?usage: fetch-corpus-refs.sh <sources-base>}"
[ -d "$base" ] || { echo "error: sources base $base is not a directory" >&2; exit 2; }

# name | upstream | pinned commit | licence (as recorded in the corpus spec)
REFS=(
  "solady|https://github.com/Vectorized/solady|2afba69bf67b78dd4abeadcc696052b3a6f71499|MIT"
  "foundry-book|https://github.com/foundry-rs/book|fa7c378defe66191744aa0394146a0bc763ada24|MIT OR Apache-2.0"
)

for entry in "${REFS[@]}"; do
  IFS='|' read -r name url pin licence <<<"$entry"
  dest="$base/$name"
  if [ -e "$dest" ]; then
    head="$(git -C "$dest" rev-parse HEAD 2>/dev/null || true)"
    if [ "$head" != "$pin" ]; then
      echo "error: $dest exists at ${head:-<not a git checkout>}, expected $pin; move it aside first" >&2
      exit 1
    fi
    if [ -n "$(git -C "$dest" status --porcelain)" ]; then
      echo "error: $dest has local changes; the corpus must be built from the pinned tree" >&2
      exit 1
    fi
    echo "ok      $name @ $pin ($licence, already present)"
    continue
  fi
  git init -q "$dest"
  git -C "$dest" remote add origin "$url"
  git -C "$dest" fetch -q --depth 1 origin "$pin"
  git -C "$dest" -c advice.detachedHead=false checkout -q FETCH_HEAD
  got="$(git -C "$dest" rev-parse HEAD)"
  [ "$got" = "$pin" ] || { echo "error: $name fetched $got, expected $pin" >&2; exit 1; }
  echo "fetched $name @ $pin ($licence)"
done
