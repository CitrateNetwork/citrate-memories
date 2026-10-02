---
created: 2026-10-01
updated: 2026-10-01
branch: hup/n5-corpus-rest
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
  and NOTICE and never read.
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

## Open items

- Medusa and Slither are AGPL-3.0 projects. Shipping their docs inside the app is
  not cleared; the spec keeps them off (pending owner sign-off).
- Solady and the Foundry book: fetched at pinned commits by `fetch-corpus-refs.sh`; whether
  they ship (and whether Solady ships its `.sol` sources or only its docs) is a size call for
  the owner.
- The release upload: the corpus asset and a `mem-mcp` built with `import-corpus` go to the
  citrate-core `runtime-deps` prerelease together (citrate-core `docs/RELEASE.md` section 3).
- A newer corpus adds nodes; it does not retire nodes from an older corpus.
