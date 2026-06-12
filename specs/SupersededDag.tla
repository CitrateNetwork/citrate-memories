-------------------------- MODULE SupersededDag --------------------------
\* Safety of the memory DAG's append-only supersession (WP-0.5).
\* Mirrors mem-store: nodes are grow-only; `a supersedes b` is guarded by the
\* same acyclicity check as `MemoryDagStore::would_cycle_supersedes`.
EXTENDS Naturals, FiniteSets
CONSTANT Nodes
VARIABLES present, supersedes, status
vars == <<present, supersedes, status>>

TypeOK ==
    /\ present \subseteq Nodes
    /\ supersedes \subseteq (present \X present)
    /\ status \in [Nodes -> {"none", "active", "superseded", "archived"}]

Init ==
    /\ present = {}
    /\ supersedes = {}
    /\ status = [n \in Nodes |-> "none"]

\* Cycle-safe transitive closure: terminates on ANY graph because `visited`
\* grows monotonically inside the finite node set. So even a (hypothetical)
\* cycle is reported as an invariant violation instead of hanging the checker.
\*
\* `visited` is SEEDED with the initial frontier (the direct successors), not
\* `{}`. The recursion only ever folds `nxt` into `visited`, so seeding with the
\* seed set is what puts the one-step successors into the result; seeding with
\* `{}` would drop them and under-report reachability (a 1-step edge x->y would
\* leave y out of Reach(x)). This mirrors the Rust BFS in
\* `MemoryDagStore::reachable_via`, which pushes each first-seen successor into
\* its result set. (WP-0.5: TLC found that the `{}` seed let the Acyclic guard
\* admit a 2-cycle the deployed code already rejects — the code was right; the
\* spec's reachability helper was the bug. Fixed 2026-06-11, Lane D.)
RECURSIVE ReachFrom(_, _, _)
ReachFrom(S, frontier, visited) ==
    IF frontier = {} THEN visited
    ELSE LET nxt == { y \in Nodes : \E x \in frontier : <<x, y>> \in S } \ visited
         IN ReachFrom(S, nxt, visited \cup nxt)

Reach(S, x) ==
    LET seed == { y \in Nodes : <<x, y>> \in S }
    IN ReachFrom(S, seed, seed)

AddNode(n) ==
    /\ status[n] = "none"
    /\ present' = present \cup {n}
    /\ status' = [status EXCEPT ![n] = "active"]
    /\ UNCHANGED supersedes

\* `a` supersedes `b`. Guard: `a` must not already be reachable from `b` via
\* supersedes, so the new edge cannot close a cycle.
Supersede(a, b) ==
    /\ a \in present /\ b \in present /\ a # b
    /\ status[b] = "active"
    /\ a \notin Reach(supersedes, b)
    /\ supersedes' = supersedes \cup {<<a, b>>}
    /\ status' = [status EXCEPT ![b] = "superseded"]
    /\ UNCHANGED present

Next ==
    \/ \E n \in Nodes : AddNode(n)
    \/ \E a, b \in present : Supersede(a, b)

Spec == Init /\ [][Next]_vars

\* ---- Invariants ----
\* No node supersedes itself transitively.
Acyclic == \A x \in present : x \notin Reach(supersedes, x)

\* "Latest understanding" is well defined: an active node is superseded by nothing.
LatestWellDefined ==
    \A n \in present :
        (status[n] = "active") => (\A m \in present : <<m, n>> \notin supersedes)

\* ---- Action property ----
\* Nodes are never removed (the store is grow-only — the basis of the CRDT merge).
GrowOnly == [][present \subseteq present']_vars
==========================================================================
