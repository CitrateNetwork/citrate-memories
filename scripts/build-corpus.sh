#!/usr/bin/env bash
# HUP-S3.1: build the release-time Hermes knowledge corpus.
#
#   scripts/build-corpus.sh <sources-base> <out-dir> [spec]
#
# <sources-base>  the citrate-labs federation root (each source is a checkout;
#                 scripts/fetch-corpus-refs.sh adds the pinned optional references)
# <out-dir>       must not exist or be empty; receives manifest.json, NOTICE.md,
#                 skills.lock and tenants/<tenant>.corpus.json
# [spec]          defaults to corpus/hermes-knowledge.toml
#
# SOURCE_DATE_MS (epoch ms) pins the corpus's logical timestamp. It defaults to
# this repo's HEAD commit time, so two builds of the same commits are
# byte-identical. The output is verified before the script exits.
#
# EMBED_BGE_DIR=<dir holding config.json, tokenizer.json, model.safetensors>
# also embeds every node with that BGE model (the same pinned files the app
# bundles) and ships tenants/<tenant>.vectors.f16, so a member's first-run import
# reuses the vectors instead of embedding about 10k nodes on their CPU. This
# needs the transformer build and takes about as long as one CPU import (about
# 1.8 nodes per second on an Apple M2 Max).
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
base="${1:?usage: build-corpus.sh <sources-base> <out-dir> [spec]}"
out="${2:?usage: build-corpus.sh <sources-base> <out-dir> [spec]}"
spec="${3:-$here/corpus/hermes-knowledge.toml}"
date_ms="${SOURCE_DATE_MS:-$(( $(git -C "$here" log -1 --format=%ct) * 1000 ))}"

features=()
embed=()
if [ -n "${EMBED_BGE_DIR:-}" ]; then
  features=(--features transformer)
  embed=(--embed-bge "$EMBED_BGE_DIR")
fi

cargo run --quiet --release --manifest-path "$here/Cargo.toml" -p mem-corpus ${features[@]+"${features[@]}"} --bin mem-corpus -- \
  build --spec "$spec" --sources-base "$base" --out "$out" --source-date-ms "$date_ms" ${embed[@]+"${embed[@]}"}
cargo run --quiet --release --manifest-path "$here/Cargo.toml" -p mem-corpus ${features[@]+"${features[@]}"} --bin mem-corpus -- \
  verify "$out"
