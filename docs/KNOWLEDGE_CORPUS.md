---
created: 2026-10-01
updated: 2026-10-01
branch: hup/m2-knowledge
author: Larry Klosowski + Claude Opus 5.5
status: implemented (format 2: build + verify + import + precomputed vectors); release upload pending
wp: HUP-S3.1
planset: citrate-core .agentile/planset/2026-09-30-hermes-upskill
---

# Knowledge corpus (HUP-S3.1)

Hermes ships with a knowledge graph so it can answer chain, SDK, precompile and
paraconsensus questions offline on first launch. This repo builds that graph at
release time; citrate-core imports it into the member's local memory store on
first run.

## Pieces

| Piece | Where |
|---|---|
| Corpus spec (what goes in, licences) | `corpus/hermes-knowledge.toml` |
| Builder + verifier + importer | `crates/mem-corpus` |
| Release build script | `scripts/build-corpus.sh <citrate-labs-root> <out-dir>` (`EMBED_BGE_DIR=` adds vectors) |
| Pinned optional references | `scripts/fetch-corpus-refs.sh <citrate-labs-root>` (Solady, Foundry book) |
| First-run import (shipped binary) | `mem-mcp import-corpus <store> <corpus-dir>` |
| Passages for answering | `memory.search` with `passages: true` (mem-mcp) |
| Fixture corpus (tests, golden output) | `crates/mem-corpus/tests/fixtures/` |

No corpus content is committed here. Only the spec, the code, and a small fixture.

## Output layout (format `citrate-corpus/2`)

```text
<out-dir>/
  manifest.json                      sources (with the repo each cites as), commits, licences,
                                     per-file sha256, per-tenant hashes, bundle_digest
  NOTICE.md                          attribution + changes, one section per source
  skills.lock                        exact copy of the HUP-S3.6 lock the build checked
  tenants/citrate-docs.corpus.json
  tenants/methodology.corpus.json
  tenants/refs.corpus.json
  tenants/skills.corpus.json
  tenants/<tenant>.vectors.f16       only when the build embedded the corpus (EMBED_BGE_DIR)
```

Tenants are written (and imported) in that order: Citrate knowledge first, the large
reviewed-skills tenant last.

Each tenant file carries a `mem_sync::SyncBundle`'s data in a compact encoding
(`crates/mem-corpus/src/bundle.rs`): node `content` as a JSON string when it is UTF-8 (every
chunk is) and edge endpoints as 64-character hex, instead of the wire format's arrays of
numbers. That halves the corpus (35.4 MB to 17.3 MB for the 2026-10-01 build). Node identity
is computed from the decoded node, so it does not depend on the encoding. Nodes are
Derived-plane `Doc` nodes: one per file (its title) and one per heading-aware chunk
(breadcrumb plus text, with the byte span in `source_ref`). `DerivedFrom` edges tie chunks to
their file; `References` edges tie a skill to its pinned reference files.

### Citations

Every source names the repository it cites as (`repo`, default its `id`) and where its root
sits in that repository (`repo_path`). A node's `source_ref` is `Artifact { repo, path }` with
`path` repository-relative, so a passage cites as `<repo>:<path>#<anchor>`
(`crates/mem-corpus/src/cite.rs`; the anchor is the GitHub slug of the chunk's section
heading). That is the citation shape the citrate-core Citrate QA eval checks. `mem-mcp`
`memory.search` with `passages: true` prints, under each hit, `cite: <citation>` and the
hit's text (capped at 2,000 characters); without the flag the rendering is unchanged.

### Precomputed vectors

Embedding the corpus with BGE on a member's CPU is slow: 1,152 nodes took 623 s (1.85 nodes
per second) on an Apple M2 Max with nothing else running, so the 10,630-node corpus would
take about 96 minutes, and the memory daemon cannot run while the importer holds the store.
`EMBED_BGE_DIR=<dir>` (a `--features transformer` build) embeds every node once at release
time with the same pinned BGE files the app bundles and writes
`tenants/<tenant>.vectors.f16`: little-endian IEEE half floats, `dim` per node, in the
tenant file's node order (`crates/mem-corpus/src/vectors.rs`). The manifest records the file,
its sha256, the model id, the dimension and the sha256 of `model.safetensors`.

`mem-mcp import-corpus` hashes the `model.safetensors` it loaded from
`CITRATE_BGE_MODEL_DIR` and reuses a tenant's vectors only when the model id, the dimension
and that weights hash all match; otherwise it embeds every node as before. Reused vectors are
re-normalised after the f16 round trip (cosine to the f32 vector above 0.999). The `done`
progress line reports `nodes_embedded` and `vectors_reused`. Vectors are never part of node
identity.

## Determinism

Same spec, same source bytes, same `--source-date-ms`: byte-identical output (vectors aside:
they come from floating-point inference, reproducible on the same host and model files).
Directory walks are sorted, no field reads the clock, and node identity is a
function of the source bytes only (`source_ref.git_sha` carries the file's
sha256). The release passes `SOURCE_DATE_MS` (the script defaults it to this
repo's HEAD commit time). `golden_fixture_bundle_matches_a_fresh_build` pins the
fixture output byte for byte.

## Build rules

- **Public tier only.** `citrate-docs` files ship only with `tier: public`
  frontmatter. Everything else is listed under `skipped` with the reason.
- **Licences.** A source with `licence_cleared = false` is listed in the manifest
  and NOTICE and never read. Every source in the spec is cleared today. The Medusa and
  Slither docs are AGPL-3.0; the owner decided on 2026-10-01 to ship them. They are
  redistributed with attribution, the upstream link and the pinned commit in
  `manifest.json` and `NOTICE.md`, and the only change is chunking for search.
- **Skills come from the lock.** A skills source ships only what citrate-core's
  `skills.lock` admits (`include-as-is`, `include-with-scripts-stripped`,
  `convert-script-to-capsule`), and only `SKILL.md` plus the refs the lock pins.
  Every file is checked against its sha256; any drift fails the build. Scripts
  never ship. An unknown verdict fails the build.
- **No symlinks.** The walker never follows a symlink, at any depth. Files over
  1 MiB are skipped and listed.
- **Missing sources.** A required source that is absent or empty fails the build.
  An optional one is recorded as excluded with the reason.

## Import rules

`verify_corpus` runs before anything is written: the manifest must re-hash to its
`bundle_digest`, every tenant file to its manifest hash, and the skills.lock copy
to its hash. Each bundle must then be plain corpus content: knowledge tenants only
(`citrate-docs`, `skills`, `refs`, `methodology`; never `personal` or
`chain-state`), Derived plane, unsigned, active, and only `DerivedFrom` /
`References` edges between nodes of the same bundle. A corpus can add knowledge;
it cannot retire, contradict or re-point anything already in the store.

`import_corpus` embeds each node with the store's own embedder (the same choice
the daemon makes for writes) and merges in batches of 128, reporting progress. A
tenant's bundle hash is recorded in the store only after the whole tenant landed,
so a second import is a no-op and an interrupted one resumes.

`mem-mcp import-corpus` prints the progress contract in
`crates/mem-corpus/src/progress.rs` as JSON lines. It opens the store directly, so
it must run while the daemon is stopped; with the daemon up it fails with a
`stage: "open"` error line.

## Build of 2026-10-01 (owner's machine, format 2)

Built from the citrate-labs root with `corpus/hermes-knowledge.toml`,
`--source-date-ms 1790900314000`, twice: byte-identical, `verify` passes.

| tenant | nodes | edges | file bytes |
|---|---:|---:|---:|
| citrate-docs | 1,688 | 1,577 | 2,656,669 |
| methodology | 37 | 34 | 60,706 |
| refs | 1,117 | 1,062 | 2,116,598 |
| skills | 7,788 | 7,677 | 12,171,234 |

Total 17,348,645 bytes with `manifest.json` (186,691), `skills.lock` (153,041) and
`NOTICE.md` (3,798); bundle digest
`6c50d9625cd86fdd49173548523c8d41a0fb368bb953fc97d29fee8ff756a42c`. The same corpus in
format 1 was 35.7 MB.

Sources: citrate-docs 99 public pages at `73ea7c56` (the commit the QA set pins), Gradient
Papers 12, `AGENTILE.md`, agentile-skills docs 2, Trail of Bits 264 files, frontend-skills
215, agentile-skills 20, OpenZeppelin docs 21 at `cab19933`, forge-std 34. The two
hermes-agent skill sources ship no files: every skill there is excluded by its skills.lock
verdict. Excluded: Solady and the Foundry book (no checkout on this host), Medusa and Slither
docs (licence not cleared).

With `scripts/fetch-corpus-refs.sh` checkouts (Solady `2afba69b`, Foundry book `fa7c378d`) the
refs tenant grows from 1,117 to 11,466 nodes (Solady 178 files and 4,275 chunks, 100 of them
`.sol` sources; the Foundry book 722 pages and 5,174 chunks) and the corpus to 31.7 MB.
Precomputed vectors add 1,536 bytes per node: 16.3 MB for the corpus above, 32.2 MB with
the two references.

### Vectors, measured

The `citrate-docs` and `methodology` tenants (1,725 nodes) were built with
`--embed-bge` and the app's bundled BGE files (weights sha256 `c7c1988a…67d7`): 1,335 s on
the M2 Max while other builds kept the load average between 60 and 98, writing 2,649,600
bytes of vectors. A real `mem-mcp import-corpus` into a fresh store then took 8.4 s:
`nodes_embedded` 0, `vectors_reused` 1,725, edges 1,611. Embedding the same 1,725 nodes at
import time would take about 15.5 minutes at the uncontended 1.85 nodes per second.

## Full build of 2026-10-02 (M2: every reference, 240-skill lock, vectors)

Inputs: the citrate-labs root with `fetch-corpus-refs.sh` checkouts (Solady `2afba69b`,
Foundry book `fa7c378d`, Medusa `87f65e2e`, Slither `eef5df94`), the citrate-core
`skills.lock` of `hup/m2-knowledge` (HUP-S3.2: 240 skills ship, 129 of them hermes-fork
skills admitted after the intake rewrite), `--source-date-ms 1790912777000` (the merge commit HEAD
time) and `--embed-bge` with the app's bundled BGE files (weights `c7c1988a...67d7`).

| tenant | nodes | edges | corpus file bytes | vectors bytes |
|---|---:|---:|---:|---:|
| citrate-docs | 1,688 | 1,577 | 2,656,669 | 2,592,768 |
| methodology | 37 | 34 | 60,706 | 56,832 |
| refs | 12,730 | 11,684 | 18,526,451 | 19,553,280 |
| skills | 18,247 | 18,007 | 27,468,010 | 28,027,392 |

Total 99,669,598 bytes with `manifest.json` (504,529), `skills.lock` (217,967) and
`NOTICE.md` (4,994); 58,498,298 bytes as `knowledge-corpus.tar.gz`; bundle digest
`970966831d9cd851c36a1564bf9494c8957405f1870024cf6cb7a7e2f6426638`. Embedding took 10,010 s
(niced, beside an LLM eval run; 2.7 nodes per second uncontended on the 1,725-node subset).
`verify` passes. A real `mem-mcp import-corpus` into a fresh store took 224 s with
`vectors_reused` 32,702 and `nodes_embedded` 0; a second import was a no-op in 4.9 s.

The skills tenant more than doubled (7,788 to 18,247 nodes) because the hermes-fork skills
now ship, and Medusa and Slither add 1,264 refs nodes. The corpus is the largest single
resource after the models: whether the skills tenant should also carry every shipped skill's
text (the skills now load through the SKILL.md loader as well) is a size call for the owner.

## Open items

- Medusa and Slither docs (AGPL-3.0): included by owner decision (2026-10-01), fetched at
  pinned commits by `fetch-corpus-refs.sh` (Medusa `87f65e2e`, Slither `eef5df94`). The
  licence review of the bundled AGPL/GPL tools themselves (gate g3-licence) is separate.
- Size: the full corpus is 99.7 MB on disk with vectors. Solady (including its `.sol`
  sources), the Foundry book and the 18,247-node skills tenant are the large parts; trimming
  any of them is a size call for the owner (citrate-core gate g5-size).
- The release upload: the corpus asset and a `mem-mcp` built with `import-corpus` go to the
  citrate-core `runtime-deps` prerelease together (citrate-core `docs/RELEASE.md` section 3).
- A newer corpus adds nodes; it does not retire nodes from an older corpus.
