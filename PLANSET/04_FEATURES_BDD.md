---
created: 2026-06-05T22:20:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
---

# citrate-memories — Features (BDD / Gherkin)

> Executable-intent specs for every v1 capability. Each `Feature` maps to one or
> more work packages in `05_SPRINTS_AND_WPS.md`. These become the acceptance
> tests (cucumber-rs); a feature is "done" when its scenarios pass against real
> code (Rule 1 — no mocks in the path under test).

## Feature: Deterministic derived-plane ingestion (WP-D1)
```gherkin
Scenario: Re-ingesting the same git state produces an identical graph
  Given a repo at commit "abc123" with 40 commits and 12 .agentile docs
  When I run "mem-cli ingest" twice from a clean store
  Then both runs produce byte-identical node ids and edge sets
  And no node id contains an embedding or a timestamp in its preimage

Scenario: Crash mid-ingest never duplicates or loses nodes
  Given ingestion has consumed up to cursor 25
  When the process is killed and restarted
  Then the cursor high-water mark does not regress
  And every artifact 1..25 has exactly one node
  And tailing resumes at 26
```

## Feature: Load-bearing edges from structured trailers (WP-D1)
```gherkin
Scenario: A commit trailer creates a deterministic load-bearing edge
  Given a commit with trailer "Agentile-Implements: SELL-S2#step-3"
  When the commit is ingested
  Then an "Implements" edge exists from the commit node to "SELL-S2#step-3"
  And the edge trust_tier is "derived-deterministic"
  And the edge is not quarantined

Scenario: A missing required trailer warns but does not block
  Given an engineer commit touching a chain crate without "Agentile-Data-Source"
  When CI runs the agentile-nudge check
  Then CI emits a warning naming the missing trailer
  And CI does not fail the build
```

## Feature: Freshness watermark prevents stale-authority (WP-D1/D2)
```gherkin
Scenario: Every recall response is stamped with its freshness
  Given the index is 3 commits behind HEAD
  When an agent calls "recall" for the repo
  Then the response includes watermark "derived@<sha>, behind HEAD by 3 commits"
  And the agent can read how many seconds old the index is
```

## Feature: Budget-shaped recall (WP-D3.1)
```gherkin
Scenario: Storyline fits the requested token budget
  Given a repo with 5,000 memory nodes
  When an agent calls recall with token_budget=2000
  Then the returned context is <= 2000 tokens
  And it is a ranked hierarchical summary with drill-down handles
  And every returned item carries provenance and a trust tier

Scenario: Trust floor filters low-trust knowledge
  Given the repo has quarantined inferred edges
  When an agent calls recall with trust_floor="human-confirmed"
  Then no inferred or quarantined edges appear in the result
```

## Feature: Code-anchored / blast-radius recall (WP-D3.2)
```gherkin
Scenario: Editing a function surfaces its decisions and incidents
  Given memory nodes anchored to "core/learning/adapters.rs#compose_lora"
  When an agent calls blast_radius for that symbol
  Then it returns the ADRs, prior incidents, and active blockers anchored there
  And results are ordered by relevance to the symbol
```

## Feature: Memory-diff handoff (WP-D3.3)
```gherkin
Scenario: An agent hands off a session as a memory-diff
  Given an agent created 7 asserted nodes and 9 edges this session
  When the agent calls "diff" to emit the session subgraph
  Then a signed memory-diff is produced containing exactly those 7 nodes and 9 edges
  When a fresh agent calls "merge_diff" on it
  Then those nodes/edges are merged with provenance preserved
  And "blame" attributes each to the originating agent and grant
```

## Feature: As-of / decision replay (WP-D3.4)
```gherkin
Scenario: Reconstruct what the graph knew at a past decision
  Given a decision node observed_at "2026-05-20T10:00:00Z"
  When an agent calls as_of "2026-05-20T10:00:00Z" for the repo
  Then the result excludes all nodes observed after that instant
  And superseding edges added later are not applied
```

## Feature: Verifiable recall (WP-D2.4)
```gherkin
Scenario: An agent independently verifies a subgraph before trusting it
  When an agent calls "verify" on a returned subgraph
  Then each asserted node's signature is checked
  And each node's source_ref resolves to a real artifact at the stated git_sha
  And each node's content hash matches its id
  And any failure is reported per-node, not as a blanket pass
```

## Feature: Authorization + analogy non-leak (WP-D2.2 / D4.3)
```gherkin
Scenario: An agent without a grant cannot read a tenant
  Given agent A has no grant for "repo:citrate-chain/memory"
  When A calls recall for citrate-chain
  Then access is denied with a grant-missing error
  And the denial is recorded in the audit hot-log

Scenario: Cross-tenant analogy requires grant intersection
  Given agent A can read repo X but not repo Y
  When A calls analogize across X and Y
  Then no raw content from Y is returned
  And at most a redacted structural shape is returned
  And the analogy edge is created only within A's authorized scope

Scenario: Delegated worker agent inherits a scoped grant
  Given a human issued a grant to supervisor S with delegation allowed
  And S delegated read on "repo:X/memory" to worker W
  When W calls recall for X
  Then access is allowed
  When the human revokes S's grant
  Then W's delegated access is revoked by cascade
```

## Feature: Crypto-shred redaction (WP-D1.6)
```gherkin
Scenario: Forgetting a node destroys its content but keeps the structure
  Given an encrypted node N belonging to tenant T
  When an admin runs "mem-cli shred --node N"
  Then N's content and embedding are unrecoverable
  And N's id and its edges remain as a tombstone for traversal integrity
  And federation peers receive only ciphertext (now undecryptable)
```

## Feature: Conflict-free federation merge (WP-D5.1)
```gherkin
Scenario: Two replicas with different evidence converge
  Given replica R1 asserts dimension d is True and R2 asserts d is False
  When R1 and R2 sync in either order
  Then both converge to Belnap "Both" for dimension d (a flagged contradiction)
  And the node/edge sets are the set-union of both replicas
```

## Feature: Poison-resistant LoRA training (WP-D5.3)
```gherkin
Scenario: Training excludes low-trust nodes
  Given the graph contains quarantined and inferred-advisory nodes
  When a LoRA training run is started
  Then only derived-deterministic and human-confirmed nodes enter the training set
  And the resulting adapter's provenance records the DAG snapshot hash
  And registration is blocked if holdout eval regresses
```

## Feature: Self-critic completeness pass (WP-D3.5)
```gherkin
Scenario: The critic files gaps as work
  When the self-critic agent runs over a repo
  Then it lists decisions lacking a rationale edge
  And claims that were never verified
  And blockers with no resolution
  And exported public symbols with zero anchored memory
```
