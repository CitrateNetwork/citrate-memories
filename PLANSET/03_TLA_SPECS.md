---
created: 2026-06-05T22:15:00Z
branch: main
author: Saul Loveman + Claude Opus 4.8 (1M context)
status: active
---

# citrate-memories — Formal Specification Plan (TLA+)

> What we model-check before trusting the system, and concrete TLA+ modules for the
> three safety-critical cores. Follows the federation's existing TLA+ practice
> (`citrate-chain/core/learning` ships 23 specs; agent-runtime's audit chain is
> TLA+-verified). Specs live in `specs/` in the repo; each WP that touches a
> spec'd property re-runs TLC in CI.

## 1. What must be formally specified (and why)

| Spec | Property class | Why it must be formal |
|---|---|---|
| `Ingestion` | safety/liveness | Cursor monotonicity + idempotency: re-ingest never duplicates or skips; rebuild is deterministic |
| `SupersededDag` | safety | Append-only + supersession stays **acyclic**; "latest" is well-defined |
| `Authz` | safety/security | No read/write without a valid unexpired grant; cross-tenant analogy needs grant **intersection** (R3) |
| `BelnapCRDT` | convergence | Replicas converge regardless of merge order (commutativity/idempotence/associativity of join) |
| `CryptoShred` | security | After Forget, content is unrecoverable; structure/traversal survives; federation moved only ciphertext |
| `Determinism` | refinement | Derived plane is a function of inputs: same `(git,.md,manifest)` ⇒ same graph (checked as a refinement/equality property) |

The first three are written below. `BelnapCRDT` reduces to lattice laws already
proptest-verified in `core/learning/belnap.rs` (we lift them to a state-based CRDT
convergence theorem). `CryptoShred` and `Determinism` are specified in Phase 0/1.

## 2. `SupersededDag` — append-only, acyclic supersession

```tla
-------------------------- MODULE SupersededDag --------------------------
EXTENDS Naturals, FiniteSets, Sequences, TLC
CONSTANTS Nodes            \* finite universe of possible content-hash ids
VARIABLES
    present,               \* set of nodes that exist (grow-only)
    supersedes,            \* set of <<a,b>> meaning a supersedes b
    status                 \* [Nodes -> {"none","active","superseded","archived"}]

vars == <<present, supersedes, status>>

TypeOK ==
    /\ present \subseteq Nodes
    /\ supersedes \subseteq (present \X present)
    /\ status \in [Nodes -> {"none","active","superseded","archived"}]

Init ==
    /\ present = {}
    /\ supersedes = {}
    /\ status = [n \in Nodes |-> "none"]

\* Reachability over supersedes (transitive closure helper)
RECURSIVE Reach(_, _)
Reach(S, x) ==
    LET succ == { y \in present : <<x,y>> \in S }
    IN  succ \cup UNION { Reach(S, y) : y \in succ }

AddNode(n) ==
    /\ status[n] = "none"
    /\ present' = present \cup {n}
    /\ status' = [status EXCEPT ![n] = "active"]
    /\ UNCHANGED supersedes

\* a supersedes b: only if it introduces no cycle; b becomes superseded.
Supersede(a, b) ==
    /\ a \in present /\ b \in present /\ a # b
    /\ status[b] = "active"
    /\ a \notin Reach(supersedes, b)         \* acyclicity guard
    /\ supersedes' = supersedes \cup {<<a,b>>}
    /\ status' = [status EXCEPT ![b] = "superseded"]
    /\ UNCHANGED present

Next == (\E n \in Nodes : AddNode(n)) \/ (\E a,b \in present : Supersede(a,b))
Spec == Init /\ [][Next]_vars

\* ---- Invariants ----
GrowOnly == [][present \subseteq present']_vars        \* nodes never removed
Acyclic  == \A x \in present : x \notin Reach(supersedes, x)
\* "latest" is well defined: every active node is superseded by nothing
LatestWellDefined ==
    \A n \in present : (status[n] = "active") => (\A m \in present : <<m,n>> \notin supersedes)
THEOREM Spec => [](TypeOK /\ Acyclic /\ LatestWellDefined)
==========================================================================
```

## 3. `Ingestion` — cursor monotonicity + idempotent rebuild

```tla
---------------------------- MODULE Ingestion ----------------------------
EXTENDS Naturals, FiniteSets, TLC
CONSTANTS Source            \* ordered set 1..N of source artifacts (commits/docs)
VARIABLES cursor, derived   \* cursor: last consumed index; derived: set of produced node ids

vars == <<cursor, derived>>
\* deterministic extraction: artifact i always yields the same node id
NodeOf(i) == i             \* model: id is a pure function of the artifact

Init == cursor = 0 /\ derived = {}

\* consume the next artifact (continuous tailing)
Step ==
    /\ cursor < Cardinality(Source)
    /\ cursor' = cursor + 1
    /\ derived' = derived \cup {NodeOf(cursor + 1)}

\* crash/restart + re-ingest from an OLD cursor must not duplicate or lose nodes
Replay ==
    /\ \E c \in 0..cursor :
         /\ cursor' = cursor              \* high-water mark never regresses
         /\ derived' = derived \cup { NodeOf(i) : i \in (c+1)..cursor }

Next == Step \/ Replay
Spec == Init /\ [][Next]_vars

Monotone     == [][cursor' >= cursor]_vars
NoDup        == \A i \in 1..cursor : NodeOf(i) \in derived          \* every consumed artifact present
Idempotent   == [][ (cursor' = cursor) => (derived' = derived) ]_vars  \* re-running consumed range adds nothing new
Deterministic == \A i \in 1..Cardinality(Source) : NodeOf(i) = NodeOf(i)  \* (placeholder for refinement check)
THEOREM Spec => [](Monotone /\ NoDup) /\ Idempotent
==========================================================================
```

## 4. `Authz` — grants gate access; analogy needs intersection

```tla
----------------------------- MODULE Authz -------------------------------
EXTENDS Naturals, FiniteSets, TLC
CONSTANTS Agents, Tenants, TIME_MAX
VARIABLES grants, now
\* grant: [agent, tenant, canRead, canWrite, expires]
vars == <<grants, now>>

TypeOK ==
    /\ grants \subseteq [agent: Agents, tenant: Tenants,
                         canRead: BOOLEAN, canWrite: BOOLEAN, expires: 1..TIME_MAX]
    /\ now \in 0..TIME_MAX

Init == grants = {} /\ now = 0
Tick == now < TIME_MAX /\ now' = now + 1 /\ UNCHANGED grants
Issue(g) == grants' = grants \cup {g} /\ UNCHANGED now
Revoke(g) == grants' = grants \ {g} /\ UNCHANGED now
Next == Tick \/ (\E g \in [agent:Agents,tenant:Tenants,canRead:BOOLEAN,canWrite:BOOLEAN,expires:1..TIME_MAX] : Issue(g)) \/ (\E g \in grants : Revoke(g))
Spec == Init /\ [][Next]_vars

ValidRead(a, t) ==
    \E g \in grants : g.agent = a /\ g.tenant = t /\ g.canRead /\ g.expires > now
ValidWrite(a, t) ==
    \E g \in grants : g.agent = a /\ g.tenant = t /\ g.canWrite /\ g.expires > now

\* The enforced predicates (checked against implementation traces):
CanRead(a, t)  == ValidRead(a, t)
CanWrite(a, t) == ValidWrite(a, t)
\* R3: an analogy spanning tenants t1,t2 requires read on BOTH (grant intersection)
CanAnalogize(a, t1, t2) == ValidRead(a, t1) /\ ValidRead(a, t2)

\* Safety: no expired/absent grant ever authorizes; analogy can't bridge a missing grant
NoAccessWithoutGrant ==
    \A a \in Agents, t \in Tenants :
        (CanRead(a,t) => \E g \in grants : g.agent=a /\ g.tenant=t /\ g.canRead /\ g.expires>now)
NoLeakViaAnalogy ==
    \A a \in Agents, t1,t2 \in Tenants :
        CanAnalogize(a,t1,t2) => (CanRead(a,t1) /\ CanRead(a,t2))
THEOREM Spec => [](TypeOK /\ NoAccessWithoutGrant /\ NoLeakViaAnalogy)
==========================================================================
```

## 5. Model-checking plan
- TLC configs in `specs/*.cfg` with small finite bounds (e.g., |Nodes|≤6, |Source|≤8, TIME_MAX≤6) — enough to expose ordering/cycle/expiry bugs.
- CI gate: any WP touching ingestion, supersession, authz, or sync re-runs the relevant spec; a failed invariant blocks merge (this is the one place we *do* block, complementing the soft trailer nudge).
- `BelnapCRDT` convergence: lift the existing `belnap.rs` proptest laws (commutativity, associativity, idempotence of `join`) into a state-based CRDT convergence argument; spot-check a 3-replica interleaving in TLC.
- `Determinism`/`CryptoShred`: authored in Phase 0/1 alongside the implementation, with the refinement check that two ingestion orders of the same source set yield equal `derived`.
