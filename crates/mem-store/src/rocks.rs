//! Durable RocksDB backend (WP-0.2b), feature-gated behind `rocksdb`.
//!
//! Implements the same [`KvStore`](crate::kv::KvStore) trait as the in-memory
//! backend, so `MemoryDagStore` is backend-agnostic. `kv_write_batch` uses a real
//! `WriteBatch` with `set_sync(true)` — the same durability discipline as
//! citrate-chain's storage layer (audit REM-2): either the whole batch is durably
//! applied or none of it is.

use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options, WriteBatch, WriteOptions, DB};

use crate::kv::{KvOp, KvStore};

pub struct RocksKv {
    db: DB,
}

impl RocksKv {
    /// Open (creating if absent) a RocksDB at `path` with the given column
    /// families. Missing CFs are created.
    pub fn open<P: AsRef<std::path::Path>>(path: P, cfs: &[&str]) -> Result<Self, String> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        let descriptors: Vec<ColumnFamilyDescriptor> = cfs
            .iter()
            .map(|name| ColumnFamilyDescriptor::new(*name, Options::default()))
            .collect();
        let db = DB::open_cf_descriptors(&opts, path, descriptors).map_err(|e| e.to_string())?;
        Ok(Self { db })
    }

    fn cf(&self, cf: &str) -> Result<&rocksdb::ColumnFamily, String> {
        self.db
            .cf_handle(cf)
            .ok_or_else(|| format!("unknown column family: {cf}"))
    }

    /// Rebuild a damaged RocksDB's MANIFEST/catalog from the SST files actually
    /// present on disk (e.g. after a daemon was killed mid-compaction and left a
    /// stale MANIFEST referencing a since-deleted SST). This operates at the
    /// RocksDB layer only — the app-level XChaCha20 envelopes are opaque value
    /// bytes to RocksDB, so repair never needs (and never sees) the tenant keys.
    /// Any range that existed *only* in a genuinely-missing SST cannot be
    /// recovered; ranges that were compacted into surviving SSTs are preserved.
    /// Always back up the directory before calling this.
    pub fn repair<P: AsRef<std::path::Path>>(path: P) -> Result<(), String> {
        let opts = Options::default();
        DB::repair(&opts, path).map_err(|e| e.to_string())
    }
}

impl KvStore for RocksKv {
    fn kv_get(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let h = self.cf(cf)?;
        self.db.get_cf(h, key).map_err(|e| e.to_string())
    }

    fn kv_put(&self, cf: &str, key: &[u8], value: &[u8]) -> Result<(), String> {
        let h = self.cf(cf)?;
        self.db.put_cf(h, key, value).map_err(|e| e.to_string())
    }

    fn kv_delete(&self, cf: &str, key: &[u8]) -> Result<(), String> {
        let h = self.cf(cf)?;
        self.db.delete_cf(h, key).map_err(|e| e.to_string())
    }

    fn kv_exists(&self, cf: &str, key: &[u8]) -> Result<bool, String> {
        Ok(self.kv_get(cf, key)?.is_some())
    }

    fn kv_checkpoint(&self, dest: &std::path::Path) -> Result<(), String> {
        // RocksDB checkpoints hard-link the live SSTs into `dest`, so they are
        // cheap + consistent and — crucially — pin the data even if a later
        // compaction deletes the original SST (the hard link keeps it alive).
        // The result is a complete, independently-openable store; because the
        // XChaCha20 envelopes are value bytes, the snapshot carries the sealed
        // data + the keyring CF verbatim (no tenant key needed to checkpoint).
        let cp = rocksdb::checkpoint::Checkpoint::new(&self.db).map_err(|e| e.to_string())?;
        cp.create_checkpoint(dest).map_err(|e| e.to_string())
    }

    fn kv_iter_cf(&self, cf: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        let h = self.cf(cf)?;
        let mut out = Vec::new();
        for item in self.db.iterator_cf(h, IteratorMode::Start) {
            let (k, v) = item.map_err(|e| e.to_string())?;
            out.push((k.to_vec(), v.to_vec()));
        }
        Ok(out)
    }

    /// Real atomic + durable batch (the contract the default impl warns about).
    fn kv_write_batch(&self, ops: &[KvOp]) -> Result<(), String> {
        let mut batch = WriteBatch::default();
        for op in ops {
            match op {
                KvOp::Put { cf, key, value } => {
                    let h = self.cf(cf)?;
                    batch.put_cf(h, key, value);
                }
                KvOp::Delete { cf, key } => {
                    let h = self.cf(cf)?;
                    batch.delete_cf(h, key);
                }
            }
        }
        let mut wo = WriteOptions::default();
        wo.set_sync(true);
        self.db.write_opt(batch, &wo).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::run_conformance;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!("memstore-rocks-{tag}-{}-{nanos}", std::process::id()));
        p
    }

    #[test]
    fn rocksdb_satisfies_kvstore_contract() {
        let dir = temp_dir("conf");
        // run_conformance touches CFs "cf", "it", "ba".
        let kv = RocksKv::open(&dir, &["cf", "it", "ba"]).expect("open rocks");
        run_conformance(&kv);
        drop(kv);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_survives_reopen() {
        let dir = temp_dir("persist");
        {
            let kv = RocksKv::open(&dir, &["cf"]).expect("open");
            kv.kv_put("cf", b"durable", b"yes").expect("put");
        }
        {
            let kv = RocksKv::open(&dir, &["cf"]).expect("reopen");
            assert_eq!(kv.kv_get("cf", b"durable").expect("get"), Some(b"yes".to_vec()));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- MEM-S6 WP-6.5 daemon durability: checkpoints are recovery points ---

    #[test]
    fn checkpoint_is_an_independently_openable_copy() {
        let src = temp_dir("ckpt-src");
        let dest = temp_dir("ckpt-dest");
        {
            let kv = RocksKv::open(&src, &["cf"]).expect("open src");
            kv.kv_put("cf", b"k1", b"v1").expect("put");
            kv.kv_put("cf", b"k2", b"v2").expect("put");
            // dest must NOT exist before create_checkpoint.
            kv.kv_checkpoint(&dest).expect("checkpoint");
        }
        // The checkpoint opens on its own and carries the data.
        let restored = RocksKv::open(&dest, &["cf"]).expect("open checkpoint");
        assert_eq!(restored.kv_get("cf", b"k1").expect("get"), Some(b"v1".to_vec()));
        assert_eq!(restored.kv_get("cf", b"k2").expect("get"), Some(b"v2".to_vec()));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn checkpoint_survives_source_deletion() {
        // The whole point: a recovery point must outlive the original. After
        // checkpointing, blow away the source dir entirely and confirm the
        // checkpoint still opens with the data (hard-linked SSTs keep it alive).
        let src = temp_dir("ckpt-outlive-src");
        let dest = temp_dir("ckpt-outlive-dest");
        {
            let kv = RocksKv::open(&src, &["cf"]).expect("open src");
            kv.kv_put("cf", b"survivor", b"present").expect("put");
            kv.kv_checkpoint(&dest).expect("checkpoint");
        }
        std::fs::remove_dir_all(&src).expect("nuke the source");
        let restored = RocksKv::open(&dest, &["cf"]).expect("checkpoint still opens after source is gone");
        assert_eq!(restored.kv_get("cf", b"survivor").expect("get"), Some(b"present".to_vec()));
        let _ = std::fs::remove_dir_all(&dest);
    }
}
