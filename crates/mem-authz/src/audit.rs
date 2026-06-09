//! Tamper-evident audit log for memory access (WP-2.5).
//!
//! A blake3 hash-chain mirroring `citrate-agent-runtime`'s `AuditChain`: each
//! record commits to the previous record's hash, so any edit to history is
//! detectable by `verify_integrity`. This is the "hot log" (every read/write/deny);
//! the cold memory graph is separate. v1 keeps it in memory; persistence + on-chain
//! anchoring (the `chain_anchor` field already exists upstream) is a later WP.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryEvent {
    Read,
    Write,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub prev_hash: [u8; 32],
    pub event: MemoryEvent,
    pub actor: String,
    pub resource_id: String,
    pub detail: String,
}

fn feed(h: &mut blake3::Hasher, tag: u8, bytes: &[u8]) {
    h.update(&[tag]);
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// blake3 over the record's fields including `sequence` and `prev_hash`, so the
/// chain linkage is part of each hash.
pub fn record_hash(r: &AuditRecord) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    feed(&mut h, 0, b"mem-authz:audit:v1");
    feed(&mut h, 1, &r.sequence.to_le_bytes());
    feed(&mut h, 2, &r.timestamp_ms.to_le_bytes());
    feed(&mut h, 3, &r.prev_hash);
    feed(&mut h, 4, format!("{:?}", r.event).as_bytes());
    feed(&mut h, 5, r.actor.as_bytes());
    feed(&mut h, 6, r.resource_id.as_bytes());
    feed(&mut h, 7, r.detail.as_bytes());
    *h.finalize().as_bytes()
}

#[derive(Default)]
pub struct AuditChain {
    records: Vec<AuditRecord>,
    last_hash: [u8; 32],
    next_seq: u64,
}

impl AuditChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(
        &mut self,
        event: MemoryEvent,
        actor: impl Into<String>,
        resource_id: impl Into<String>,
        detail: impl Into<String>,
        now_ms: u64,
    ) -> &AuditRecord {
        let record = AuditRecord {
            sequence: self.next_seq,
            timestamp_ms: now_ms,
            prev_hash: self.last_hash,
            event,
            actor: actor.into(),
            resource_id: resource_id.into(),
            detail: detail.into(),
        };
        self.last_hash = record_hash(&record);
        self.next_seq += 1;
        self.records.push(record);
        self.records.last().expect("just pushed")
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &[AuditRecord] {
        &self.records
    }

    /// Re-walk the chain; returns the count or the sequence where linkage breaks.
    pub fn verify_integrity(&self) -> Result<u64, String> {
        let mut expected_prev = [0u8; 32];
        for (i, r) in self.records.iter().enumerate() {
            if r.sequence != i as u64 {
                return Err(format!("sequence break at {}", r.sequence));
            }
            if r.prev_hash != expected_prev {
                return Err(format!("prev_hash break at sequence {}", r.sequence));
            }
            expected_prev = record_hash(r);
        }
        Ok(self.records.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_verifies() {
        let mut c = AuditChain::new();
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1);
        c.append(MemoryEvent::Denied, "agent", "repo:y/memory", "recall: denied", 2);
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 3);
        assert_eq!(c.len(), 3);
        assert_eq!(c.verify_integrity(), Ok(3));
        assert_eq!(c.records()[0].sequence, 0);
        assert_eq!(c.records()[1].event, MemoryEvent::Denied);
    }

    #[test]
    fn tampering_is_detected() {
        let mut c = AuditChain::new();
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1);
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 2);
        // Forge a record's detail after the fact.
        c.records[0].detail = "forged".into();
        assert!(c.verify_integrity().is_err());
    }
}
