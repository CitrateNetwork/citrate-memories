---
title: GhostDAG consensus
tier: public
created: 2026-06-17T00:00:00Z
author: Citrate team
---

# GhostDAG consensus

Citrate orders blocks with GhostDAG. A block references several parents, so the
ledger is a DAG rather than a single chain.

## Blue set

The blue set is the largest well-connected subset of the DAG. Blocks outside it
are red and are ordered after the blue blocks.

## Finality

Finality follows from the blue score, which only grows.
