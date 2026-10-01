---
created: 2026-10-01
branch: hup/n4-corpus
author: Larry Klosowski + Claude Opus 5.5
status: implemented (build + verify + import); release staging pending
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
| Release build script | `scripts/build-corpus.sh <citrate-labs-root> <out-dir>` |
| First-run import (shipped binary) | `mem-mcp import-corpus <store> <corpus-dir>` |
| Fixture corpus (tests, golden output) | `crates/mem-corpus/tests/fixtures/` |

No corpus content is committed here. Only the spec, the code, and a small fixture.

## Output layout

```text
<out-dir>/
  manifest.json                      sources, commits, licences, per-file sha256,
                                     per-tenant bundle sha256, bundle_digest
  NOTICE.md                          attribution + changes, one section per source
  skills.lock                        exact copy of the HUP-S3.6 lock the build checked
  tenants/citrate-docs.syncbundle.json
  tenants/skills.syncbundle.json
  tenants/refs.syncbundle.json
  tenants/methodology.syncbundle.json
```

Each tenant file is a `mem_sync::SyncBundle` (the existing federation wire
format). Nodes are Derived-plane `Doc` nodes: one per file (its title) and one per
heading-aware chunk (breadcrumb plus text, with the byte span in `source_ref`).
`DerivedFrom` edges tie chunks to their file; `References` edges tie a skill to
its pinned reference files.

## Determinism

Same spec, same source bytes, same `--source-date-ms`: byte-identical output.
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

## Trial build on the owner's machine (2026-10-01)

Built from the citrate-labs root with `corpus/hermes-knowledge.toml`: 10,630 nodes
and 10,350 edges across the four tenants (about 35 MB of JSON), byte-identical
across two builds, verified, and imported into a fresh store with the hashing
embedder in about 90 s on a debug build. Excluded on that host: Solady and the
Foundry book (no checkout), Medusa and Slither docs (licence not cleared).

## Open items

- Medusa and Slither are AGPL-3.0 projects. Shipping their docs inside the app is
  not cleared; the spec keeps them off (pending owner sign-off).
- Solady and the Foundry book need checkouts on the release build host.
- The release workflow does not stage the corpus yet, and the shipped `mem-mcp`
  binary in the `runtime-deps` prerelease predates `import-corpus`.
- A newer corpus adds nodes; it does not retire nodes from an older corpus.
