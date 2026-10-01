#!/usr/bin/env bash
# HUP-S3.1: build the release-time Hermes knowledge corpus.
#
#   scripts/build-corpus.sh <sources-base> <out-dir> [spec]
#
# <sources-base>  the citrate-labs federation root (each source is a checkout)
# <out-dir>       must not exist or be empty; receives manifest.json, NOTICE.md,
#                 skills.lock and tenants/<tenant>.syncbundle.json
# [spec]          defaults to corpus/hermes-knowledge.toml
#
# SOURCE_DATE_MS (epoch ms) pins the corpus's logical timestamp. It defaults to
# this repo's HEAD commit time, so two builds of the same commits are
# byte-identical. The output is verified before the script exits.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
base="${1:?usage: build-corpus.sh <sources-base> <out-dir> [spec]}"
out="${2:?usage: build-corpus.sh <sources-base> <out-dir> [spec]}"
spec="${3:-$here/corpus/hermes-knowledge.toml}"
date_ms="${SOURCE_DATE_MS:-$(( $(git -C "$here" log -1 --format=%ct) * 1000 ))}"

cargo run --quiet --release --manifest-path "$here/Cargo.toml" -p mem-corpus --bin mem-corpus -- \
  build --spec "$spec" --sources-base "$base" --out "$out" --source-date-ms "$date_ms"
cargo run --quiet --release --manifest-path "$here/Cargo.toml" -p mem-corpus --bin mem-corpus -- \
  verify "$out"
