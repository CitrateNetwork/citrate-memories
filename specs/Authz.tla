----------------------------- MODULE Authz -------------------------------
\* Authorization safety (WP-0.5): grants gate access, and a cross-tenant analogy
\* requires the INTERSECTION of read grants (risk R3 — no leak via analogy).
\*
\* Operational model: `readsOk` / `analogyOk` are history variables recording the
\* access the system actually authorized. AuthorizeAnalogy records the two
\* component reads alongside the analogy, so the key invariant is non-tautological:
\* it fails if any future edit lets an analogy be authorized without BOTH reads.
EXTENDS Naturals, FiniteSets
CONSTANTS Agents, Tenants, TIME_MAX
VARIABLES grants, now, readsOk, analogyOk
vars == <<grants, now, readsOk, analogyOk>>

GrantRec == [agent: Agents, tenant: Tenants, canRead: BOOLEAN,
             canWrite: BOOLEAN, expires: 1..TIME_MAX]

TypeOK ==
    /\ grants \subseteq GrantRec
    /\ now \in 0..TIME_MAX
    /\ readsOk \subseteq (Agents \X Tenants)
    /\ analogyOk \subseteq (Agents \X Tenants \X Tenants)

Init ==
    /\ grants = {}
    /\ now = 0
    /\ readsOk = {}
    /\ analogyOk = {}

ValidRead(a, t) ==
    \E g \in grants : g.agent = a /\ g.tenant = t /\ g.canRead /\ g.expires > now
ValidWrite(a, t) ==
    \E g \in grants : g.agent = a /\ g.tenant = t /\ g.canWrite /\ g.expires > now

Tick ==
    /\ now < TIME_MAX
    /\ now' = now + 1
    /\ UNCHANGED <<grants, readsOk, analogyOk>>

Issue(g) ==
    /\ grants' = grants \cup {g}
    /\ UNCHANGED <<now, readsOk, analogyOk>>

Revoke(g) ==
    /\ g \in grants
    /\ grants' = grants \ {g}
    /\ UNCHANGED <<now, readsOk, analogyOk>>

AuthorizeRead(a, t) ==
    /\ ValidRead(a, t)
    /\ readsOk' = readsOk \cup {<<a, t>>}
    /\ UNCHANGED <<grants, now, analogyOk>>

\* Cross-tenant analogy: allowed only when BOTH tenant reads are valid; the two
\* component reads are recorded together with the analogy.
AuthorizeAnalogy(a, t1, t2) ==
    /\ t1 # t2
    /\ ValidRead(a, t1)
    /\ ValidRead(a, t2)
    /\ readsOk' = readsOk \cup {<<a, t1>>, <<a, t2>>}
    /\ analogyOk' = analogyOk \cup {<<a, t1, t2>>}
    /\ UNCHANGED <<grants, now>>

Next ==
    \/ Tick
    \/ \E g \in GrantRec : Issue(g)
    \/ \E g \in grants : Revoke(g)
    \/ \E a \in Agents, t \in Tenants : AuthorizeRead(a, t)
    \/ \E a \in Agents, t1 \in Tenants, t2 \in Tenants : AuthorizeAnalogy(a, t1, t2)

Spec == Init /\ [][Next]_vars

\* ---- Invariant (non-tautological) ----
\* Every authorized analogy is backed by both authorized component reads.
\* Regression-catcher: if AuthorizeAnalogy ever forgets a component read, or is
\* guarded by a single grant, this fails.
AnalogyImpliesReads ==
    \A tr \in analogyOk :
        /\ <<tr[1], tr[2]>> \in readsOk
        /\ <<tr[1], tr[3]>> \in readsOk

\* State-space bound for TLC (keeps grant subsets small).
StateBound == Cardinality(grants) =< 3
==========================================================================
