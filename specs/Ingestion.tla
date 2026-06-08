---------------------------- MODULE Ingestion ----------------------------
\* Cursor-resumable, idempotent, deterministic ingestion (WP-0.5).
\* Models the Derived-plane ingest loop: a monotone cursor over an ordered set
\* of source artifacts, where re-ingesting an already-consumed range adds nothing
\* (crash/restart safety) and node identity is a pure function of the artifact.
EXTENDS Naturals, FiniteSets
CONSTANT N                      \* number of source artifacts
Source == 1..N
NodeOf(i) == i                  \* deterministic: artifact i always yields node i
VARIABLES cursor, derived
vars == <<cursor, derived>>

TypeOK == cursor \in 0..N /\ derived \subseteq Source
Init == cursor = 0 /\ derived = {}

\* Consume the next artifact (continuous tailing).
Step ==
    /\ cursor < N
    /\ cursor' = cursor + 1
    /\ derived' = derived \cup { NodeOf(cursor + 1) }

\* Crash/restart: re-ingest from any earlier point up to the high-water mark.
\* The cursor does not regress and nothing new is produced.
Replay ==
    \E c \in 0..cursor :
        /\ cursor' = cursor
        /\ derived' = derived \cup { NodeOf(i) : i \in (c + 1)..cursor }

Next == Step \/ Replay
Spec == Init /\ [][Next]_vars

\* ---- Invariant ----
\* Every consumed artifact is present exactly once (set semantics => no dup).
NoDup == \A i \in 1..cursor : NodeOf(i) \in derived

\* ---- Action properties ----
Monotone == [][cursor' >= cursor]_vars
\* Re-running a consumed range (cursor unchanged) changes nothing: idempotent.
Idempotent == [][ (cursor' = cursor) => (derived' = derived) ]_vars
==========================================================================
