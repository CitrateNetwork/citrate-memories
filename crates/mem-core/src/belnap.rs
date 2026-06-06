//! Belnap four-valued logic for per-dimension confidence.
//!
//! v1 local copy of the lattice. v2 will import the proptest-verified version
//! from `citrate-chain/core/learning/belnap.rs` directly (it is drop-in — see
//! `00_OVERVIEW.md` reuse map). We keep a local copy here so the first-pass
//! workspace compiles standalone without pinning the whole chain workspace.
//!
//! Knowledge ordering: `Neither` ≤ {`True`,`False`} ≤ `Both`.
//! `join` is the least-upper-bound (used as the CRDT merge — combining evidence);
//! `meet` is the greatest-lower-bound. Both are commutative, associative, and
//! idempotent, which is exactly what a state-based CRDT requires.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BelnapValue {
    True,
    False,
    Both,
    /// Unknown / no evidence. The bottom of the knowledge lattice and the
    /// identity element for `join`.
    #[default]
    Neither,
}

impl BelnapValue {
    /// Least upper bound under the knowledge ordering — *combine evidence*.
    /// `True ⊔ False = Both` surfaces a genuine contradiction instead of
    /// silently picking a winner. This is the federation merge function.
    pub fn join(self, other: Self) -> Self {
        use BelnapValue::*;
        match (self, other) {
            (Neither, x) | (x, Neither) => x,
            (Both, _) | (_, Both) => Both,
            (True, True) => True,
            (False, False) => False,
            (True, False) | (False, True) => Both,
        }
    }

    /// Greatest lower bound under the knowledge ordering.
    pub fn meet(self, other: Self) -> Self {
        use BelnapValue::*;
        match (self, other) {
            (Both, x) | (x, Both) => x,
            (Neither, _) | (_, Neither) => Neither,
            (True, True) => True,
            (False, False) => False,
            (True, False) | (False, True) => Neither,
        }
    }

    /// A contradiction the consuming agent must stop and resolve, not auto-pick.
    pub fn is_contradiction(&self) -> bool {
        matches!(self, BelnapValue::Both)
    }

    /// No evidence yet — treat as stale/unknown.
    pub fn is_unknown(&self) -> bool {
        matches!(self, BelnapValue::Neither)
    }
}

/// Merge two confidence vectors dimension-wise via `join`. Shorter vectors are
/// treated as `Neither` (no evidence) in the missing positions, so the merge is
/// total and order-independent.
pub fn join_confidence(a: &[BelnapValue], b: &[BelnapValue]) -> Vec<BelnapValue> {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| {
            let x = a.get(i).copied().unwrap_or_default();
            let y = b.get(i).copied().unwrap_or_default();
            x.join(y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::BelnapValue::*;
    use super::*;

    const ALL: [BelnapValue; 4] = [True, False, Both, Neither];

    #[test]
    fn join_commutative_associative_idempotent() {
        for a in ALL {
            assert_eq!(a.join(a), a, "idempotent");
            for b in ALL {
                assert_eq!(a.join(b), b.join(a), "commutative");
                for c in ALL {
                    assert_eq!(a.join(b).join(c), a.join(b.join(c)), "associative");
                }
            }
        }
    }

    #[test]
    fn neither_is_join_identity() {
        for a in ALL {
            assert_eq!(a.join(Neither), a);
        }
    }

    #[test]
    fn contradiction_surfaces() {
        assert_eq!(True.join(False), Both);
        assert!(True.join(False).is_contradiction());
    }

    #[test]
    fn confidence_merge_is_order_independent() {
        let a = vec![True, Neither, False];
        let b = vec![False, True];
        // join(a,b) == join(b,a), and shorter vec padded with Neither
        assert_eq!(join_confidence(&a, &b), join_confidence(&b, &a));
        assert_eq!(join_confidence(&a, &b), vec![Both, True, False]);
    }
}
