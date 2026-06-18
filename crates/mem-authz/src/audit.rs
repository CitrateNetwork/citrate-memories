//! Tamper-evident audit log for memory access (WP-2.5, persistence SECREM-02 7.5).
//!
//! A blake3 hash-chain mirroring `citrate-agent-runtime`'s `AuditChain`: each
//! record commits to the previous record's hash, so any edit to history is
//! detectable by `verify_integrity`. This is the "hot log" (every read/write/deny);
//! the cold memory graph is separate.
//!
//! Persistence: [`AuditChain::open`] binds the chain to an append-only JSONL log
//! plus a `.head` sidecar recording `(len, last_hash)`. Every append is written
//! through (line fsync'd, then the head is atomically replaced), so a restart
//! resumes the chain where it left off. On open the whole chain is re-walked:
//! a record edited in place breaks linkage, and a log truncated behind the head
//! is reported as [`AuditError::Truncated`]. An adversary who rewrites BOTH the
//! log and the head consistently is out of local scope — that is what on-chain
//! anchoring of the head hash (the `chain_anchor` field upstream) is for.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

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

/// Why a persistent audit chain could not be opened or appended to.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("audit log io: {0}")]
    Io(#[from] std::io::Error),
    /// The log or head sidecar exists but cannot be trusted (parse failure,
    /// linkage break, head/log hash mismatch, missing sidecar).
    #[error("audit log corrupt: {0}")]
    Corrupt(String),
    /// The head sidecar commits to more records than the log holds — the log
    /// was truncated (or rolled back) behind the chain head.
    #[error("audit log truncated: head commits to {head} records but log holds {log}")]
    Truncated { head: u64, log: u64 },
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

/// `.head` sidecar payload: the chain length and last hash the log must match.
#[derive(Serialize, Deserialize)]
struct Head {
    len: u64,
    last_hash: String,
}

/// Write-through sink: the append-only JSONL log + the head sidecar paths.
struct FileSink {
    log: File,
    log_path: PathBuf,
    head_path: PathBuf,
}

fn head_path_for(log_path: &Path) -> PathBuf {
    let mut s = log_path.as_os_str().to_os_string();
    s.push(".head");
    PathBuf::from(s)
}

/// Atomically replace the head sidecar (write temp, fsync, rename).
fn write_head(head_path: &Path, len: u64, last_hash: &[u8; 32]) -> Result<(), AuditError> {
    let tmp = {
        let mut s = head_path.as_os_str().to_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    };
    let payload = serde_json::to_vec(&Head { len, last_hash: hex::encode(last_hash) })
        .map_err(|e| AuditError::Corrupt(format!("head serialize: {e}")))?;
    let mut f = File::create(&tmp)?;
    f.write_all(&payload)?;
    f.sync_all()?;
    std::fs::rename(&tmp, head_path)?;
    Ok(())
}

#[derive(Default)]
pub struct AuditChain {
    records: Vec<AuditRecord>,
    last_hash: [u8; 32],
    next_seq: u64,
    /// `None` → in-memory only (tests/ephemeral); `Some` → write-through.
    sink: Option<FileSink>,
}

impl AuditChain {
    /// In-memory chain (no persistence) — tamper-evidence lasts one process.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open (or create) a persistent chain at `log_path`, verify the whole
    /// chain on load, and resume appending where the last run stopped.
    ///
    /// Detection on load:
    /// - any record edited in place → [`AuditError::Corrupt`] (linkage break);
    /// - log truncated behind the head sidecar → [`AuditError::Truncated`];
    /// - head sidecar missing/unparseable while the log has records →
    ///   [`AuditError::Corrupt`].
    ///
    /// Crash tolerance: an append fsyncs the log line before replacing the
    /// head, so a crash in between leaves the log exactly one record ahead of
    /// the head. That state is accepted and the head is repaired; anything
    /// further ahead is rejected.
    pub fn open(log_path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let log_path = log_path.as_ref().to_path_buf();
        let head_path = head_path_for(&log_path);

        // Fresh chain: create both files up front so the head sidecar always
        // exists once the chain does (a missing head is then evidence, not noise).
        if !log_path.exists() {
            if let Some(parent) = log_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            let log = OpenOptions::new().create_new(true).append(true).open(&log_path)?;
            write_head(&head_path, 0, &[0u8; 32])?;
            return Ok(Self {
                records: Vec::new(),
                last_hash: [0u8; 32],
                next_seq: 0,
                sink: Some(FileSink { log, log_path, head_path }),
            });
        }

        // Load + re-walk the chain, keeping the running hash after each record
        // so the head's commitment can be checked at its own length.
        let reader = BufReader::new(File::open(&log_path)?);
        let mut records: Vec<AuditRecord> = Vec::new();
        let mut hash_after: Vec<[u8; 32]> = Vec::new();
        let mut expected_prev = [0u8; 32];
        for (i, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let r: AuditRecord = serde_json::from_str(&line)
                .map_err(|e| AuditError::Corrupt(format!("record {i} unparseable: {e}")))?;
            if r.sequence != records.len() as u64 {
                return Err(AuditError::Corrupt(format!(
                    "sequence break at record {i}: expected {}, found {}",
                    records.len(),
                    r.sequence
                )));
            }
            if r.prev_hash != expected_prev {
                return Err(AuditError::Corrupt(format!("prev_hash break at sequence {}", r.sequence)));
            }
            expected_prev = record_hash(&r);
            hash_after.push(expected_prev);
            records.push(r);
        }
        let log_len = records.len() as u64;
        let last_hash = expected_prev;

        // The head sidecar must exist and commit to a verified prefix.
        let head_bytes = std::fs::read(&head_path).map_err(|e| {
            AuditError::Corrupt(format!("head sidecar missing/unreadable ({e}) — possible tamper"))
        })?;
        let head: Head = serde_json::from_slice(&head_bytes)
            .map_err(|e| AuditError::Corrupt(format!("head sidecar unparseable: {e}")))?;
        let head_hash: [u8; 32] = hex::decode(&head.last_hash)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| AuditError::Corrupt("head last_hash is not 32 hex bytes".into()))?;

        if head.len > log_len {
            return Err(AuditError::Truncated { head: head.len, log: log_len });
        }
        if log_len > head.len + 1 {
            return Err(AuditError::Corrupt(format!(
                "log holds {log_len} records but head commits to {} — more than one in-flight append",
                head.len
            )));
        }
        let prefix_hash = if head.len == 0 { [0u8; 32] } else { hash_after[head.len as usize - 1] };
        if prefix_hash != head_hash {
            return Err(AuditError::Corrupt(format!(
                "head hash does not match the log at length {}",
                head.len
            )));
        }

        // Repair the head after an accepted crash window.
        if head.len != log_len {
            write_head(&head_path, log_len, &last_hash)?;
        }

        let log = OpenOptions::new().append(true).open(&log_path)?;
        Ok(Self {
            records,
            last_hash,
            next_seq: log_len,
            sink: Some(FileSink { log, log_path, head_path }),
        })
    }

    /// Path of the backing log, when persistent.
    pub fn log_path(&self) -> Option<&Path> {
        self.sink.as_ref().map(|s| s.log_path.as_path())
    }

    /// Append a record. For a persistent chain the record is durably written
    /// (log line fsync'd, head replaced) BEFORE it becomes visible in memory;
    /// on error the in-memory chain is unchanged, so callers can fail closed.
    pub fn append(
        &mut self,
        event: MemoryEvent,
        actor: impl Into<String>,
        resource_id: impl Into<String>,
        detail: impl Into<String>,
        now_ms: u64,
    ) -> Result<&AuditRecord, AuditError> {
        let record = AuditRecord {
            sequence: self.next_seq,
            timestamp_ms: now_ms,
            prev_hash: self.last_hash,
            event,
            actor: actor.into(),
            resource_id: resource_id.into(),
            detail: detail.into(),
        };
        let hash = record_hash(&record);
        if let Some(sink) = &mut self.sink {
            let mut line = serde_json::to_vec(&record)
                .map_err(|e| AuditError::Corrupt(format!("record serialize: {e}")))?;
            line.push(b'\n');
            sink.log.write_all(&line)?;
            sink.log.sync_data()?;
            write_head(&sink.head_path, record.sequence + 1, &hash)?;
        }
        self.last_hash = hash;
        self.next_seq += 1;
        self.records.push(record);
        Ok(self.records.last().expect("just pushed"))
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_log(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!("memauthz-audit-{tag}-{}-{nanos}.jsonl", std::process::id()));
        p
    }

    fn cleanup(p: &Path) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(head_path_for(p));
    }

    #[test]
    fn appends_and_verifies() {
        let mut c = AuditChain::new();
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
        c.append(MemoryEvent::Denied, "agent", "repo:y/memory", "recall: denied", 2).expect("append");
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 3).expect("append");
        assert_eq!(c.len(), 3);
        assert_eq!(c.verify_integrity(), Ok(3));
        assert_eq!(c.records()[0].sequence, 0);
        assert_eq!(c.records()[1].event, MemoryEvent::Denied);
    }

    #[test]
    fn tampering_is_detected() {
        let mut c = AuditChain::new();
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 2).expect("append");
        // Forge a record's detail after the fact.
        c.records[0].detail = "forged".into();
        assert!(c.verify_integrity().is_err());
    }

    // --- persistence (SECREM-02 7.5: chain must survive restart) ---

    #[test]
    fn persists_resumes_and_continues_across_reopen() {
        let path = temp_log("resume");
        {
            let mut c = AuditChain::open(&path).expect("open fresh");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
            c.append(MemoryEvent::Write, "agent", "repo:x/memory", "assert", 2).expect("append");
            c.append(MemoryEvent::Denied, "agent", "repo:y/memory", "recall: denied", 3).expect("append");
        } // process "restart"
        {
            let mut c = AuditChain::open(&path).expect("reopen");
            assert_eq!(c.verify_integrity(), Ok(3));
            assert_eq!(c.records()[2].event, MemoryEvent::Denied);
            // The chain CONTINUES: next record links to the pre-restart head.
            let r = c
                .append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 4)
                .expect("append after reopen")
                .clone();
            assert_eq!(r.sequence, 3);
            assert_eq!(r.prev_hash, record_hash(&c.records()[2]));
        }
        let c = AuditChain::open(&path).expect("reopen again");
        assert_eq!(c.verify_integrity(), Ok(4));
        cleanup(&path);
    }

    #[test]
    fn truncated_log_is_detected_on_open() {
        let path = temp_log("truncate");
        {
            let mut c = AuditChain::open(&path).expect("open");
            for i in 0..3 {
                c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", i).expect("append");
            }
        }
        // Drop the last record from the log (head still commits to 3).
        let contents = std::fs::read_to_string(&path).expect("read log");
        let kept: Vec<&str> = contents.lines().take(2).collect();
        std::fs::write(&path, format!("{}\n", kept.join("\n"))).expect("truncate log");
        match AuditChain::open(&path) {
            Err(AuditError::Truncated { head: 3, log: 2 }) => {}
            Err(e) => panic!("expected Truncated {{3, 2}}, got error: {e}"),
            Ok(_) => panic!("expected Truncated {{3, 2}}, got Ok"),
        }
        cleanup(&path);
    }

    #[test]
    fn tampered_log_record_is_detected_on_open() {
        let path = temp_log("tamper");
        {
            let mut c = AuditChain::open(&path).expect("open");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 2).expect("append");
        }
        let contents = std::fs::read_to_string(&path).expect("read log");
        let forged = contents.replacen("recall", "forged", 1);
        assert_ne!(contents, forged, "fixture must actually change the log");
        std::fs::write(&path, forged).expect("forge log");
        assert!(matches!(AuditChain::open(&path), Err(AuditError::Corrupt(_))));
        cleanup(&path);
    }

    #[test]
    fn missing_head_sidecar_is_detected_on_open() {
        let path = temp_log("nohead");
        {
            let mut c = AuditChain::open(&path).expect("open");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
        }
        std::fs::remove_file(head_path_for(&path)).expect("remove head");
        assert!(matches!(AuditChain::open(&path), Err(AuditError::Corrupt(_))));
        cleanup(&path);
    }

    #[test]
    fn crash_window_log_one_ahead_of_head_is_accepted_and_repaired() {
        let path = temp_log("crash");
        let head = head_path_for(&path);
        let head_at_2;
        {
            let mut c = AuditChain::open(&path).expect("open");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 2).expect("append");
            head_at_2 = std::fs::read(&head).expect("snapshot head");
            c.append(MemoryEvent::Write, "agent", "repo:x/memory", "assert", 3).expect("append");
        }
        // Simulate a crash between the log fsync and the head replacement.
        std::fs::write(&head, &head_at_2).expect("roll head back");
        let c = AuditChain::open(&path).expect("crash window accepted");
        assert_eq!(c.verify_integrity(), Ok(3));
        // …and the head was repaired to the full length.
        let repaired: Head = serde_json::from_slice(&std::fs::read(&head).expect("read head")).expect("parse");
        assert_eq!(repaired.len, 3);
        cleanup(&path);
    }

    #[test]
    fn log_more_than_one_ahead_of_head_is_rejected() {
        let path = temp_log("ahead");
        let head = head_path_for(&path);
        let head_at_1;
        {
            let mut c = AuditChain::open(&path).expect("open");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
            head_at_1 = std::fs::read(&head).expect("snapshot head");
            c.append(MemoryEvent::Read, "agent", "repo:x/memory", "search", 2).expect("append");
            c.append(MemoryEvent::Write, "agent", "repo:x/memory", "assert", 3).expect("append");
        }
        std::fs::write(&head, &head_at_1).expect("roll head back two");
        assert!(matches!(AuditChain::open(&path), Err(AuditError::Corrupt(_))));
        cleanup(&path);
    }

    // --- mutation-coverage closers (SECREM-02 Phase 8.3) ---
    // These pin accessors + the open() parent-dir guard that the cargo-mutants
    // run on 2026-06-11 left as surviving mutants (mem-authz/src/audit.rs).

    #[test]
    fn is_empty_and_len_track_records() {
        // Kills: `is_empty -> true` and `is_empty -> false` mutants (audit.rs:280).
        let mut c = AuditChain::new();
        assert!(c.is_empty(), "a fresh chain is empty");
        assert_eq!(c.len(), 0);
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append");
        assert!(!c.is_empty(), "after one append the chain is not empty");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn log_path_is_some_for_persistent_and_none_for_in_memory() {
        // Kills: `log_path -> None` mutant (audit.rs:237).
        let mem = AuditChain::new();
        assert!(mem.log_path().is_none(), "an in-memory chain has no backing log");
        let path = temp_log("logpath");
        let c = AuditChain::open(&path).expect("open");
        assert_eq!(c.log_path(), Some(path.as_path()), "a persistent chain exposes its log path");
        cleanup(&path);
    }

    #[test]
    fn open_creates_missing_parent_directory() {
        // Kills: `delete ! in AuditChain::open` (audit.rs:149) — the guard that
        // only calls create_dir_all when the parent path is non-empty. Open a log
        // under a not-yet-existing nested dir and confirm it is created + usable.
        let base = {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let mut p = std::env::temp_dir();
            p.push(format!("memauthz-nested-{}-{nanos}", std::process::id()));
            p
        };
        let nested = base.join("a").join("b");
        let path = nested.join("audit.jsonl");
        assert!(!nested.exists(), "parent dir does not exist yet");
        let mut c = AuditChain::open(&path).expect("open creates the parent dir");
        assert!(nested.exists(), "open must create the missing parent directory");
        c.append(MemoryEvent::Read, "agent", "repo:x/memory", "recall", 1).expect("append works");
        cleanup(&path);
        let _ = std::fs::remove_dir_all(&base);
    }
}
