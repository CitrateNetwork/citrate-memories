//! Key-value backend abstraction.
//!
//! `KvStore` and `KvOp` mirror `citrate_consensus::dag_store::{KvStore, KvOp}`
//! byte-for-byte (verified 2026-06-05). v1 keeps a local copy so the workspace
//! builds standalone; the v2 rebuild switches `use mem_store::kv::KvStore` to
//! `use citrate_consensus::dag_store::KvStore` with no other change.
//!
//! The default `kv_write_batch` is sequential; backends with real atomicity
//! (the in-memory backend here, and the RocksDB backend in a later WP) MUST
//! override it so a partial batch can never be observed.

use std::collections::BTreeMap;
use std::sync::RwLock;

/// Single operation inside a [`KvStore::kv_write_batch`] payload.
#[derive(Debug, Clone)]
pub enum KvOp {
    Put {
        cf: String,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        cf: String,
        key: Vec<u8>,
    },
}

pub trait KvStore: Send + Sync {
    fn kv_get(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, String>;
    fn kv_put(&self, cf: &str, key: &[u8], value: &[u8]) -> Result<(), String>;
    fn kv_delete(&self, cf: &str, key: &[u8]) -> Result<(), String>;
    fn kv_exists(&self, cf: &str, key: &[u8]) -> Result<bool, String>;
    #[allow(clippy::type_complexity)]
    fn kv_iter_cf(&self, cf: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String>;

    /// Apply a sequence of operations as a single atomic write.
    /// Contract: either every op is durably applied, or none is.
    fn kv_write_batch(&self, ops: &[KvOp]) -> Result<(), String> {
        for op in ops {
            match op {
                KvOp::Put { cf, key, value } => self.kv_put(cf, key, value)?,
                KvOp::Delete { cf, key } => self.kv_delete(cf, key)?,
            }
        }
        Ok(())
    }

    /// Create a consistent point-in-time snapshot of the whole store at `dest`
    /// (a fresh, independently-openable copy). Backends that support it (RocksDB)
    /// produce a recovery point cheaply; the default is `Unsupported` so the
    /// in-memory backend doesn't pretend to be durable. `dest` must not exist.
    fn kv_checkpoint(&self, _dest: &std::path::Path) -> Result<(), String> {
        Err("checkpoint not supported by this backend".to_string())
    }
}

/// A real, fully-functional in-memory backend (not a mock — Rule 1). Used as the
/// default for tests and ephemeral runs. Iteration is key-sorted (BTreeMap), so
/// `kv_iter_cf` order is deterministic. The RocksDB backend lands in a later WP
/// behind a feature flag and implements the same trait.
/// cf name -> (key -> value).
type CfMap = BTreeMap<String, BTreeMap<Vec<u8>, Vec<u8>>>;

#[derive(Default)]
pub struct InMemoryKv {
    data: RwLock<CfMap>,
}

impl InMemoryKv {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KvStore for InMemoryKv {
    fn kv_get(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let data = self.data.read().map_err(|_| "kv lock poisoned".to_string())?;
        Ok(data.get(cf).and_then(|m| m.get(key)).cloned())
    }

    fn kv_put(&self, cf: &str, key: &[u8], value: &[u8]) -> Result<(), String> {
        let mut data = self.data.write().map_err(|_| "kv lock poisoned".to_string())?;
        data.entry(cf.to_string())
            .or_default()
            .insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn kv_delete(&self, cf: &str, key: &[u8]) -> Result<(), String> {
        let mut data = self.data.write().map_err(|_| "kv lock poisoned".to_string())?;
        if let Some(m) = data.get_mut(cf) {
            m.remove(key);
        }
        Ok(())
    }

    fn kv_exists(&self, cf: &str, key: &[u8]) -> Result<bool, String> {
        let data = self.data.read().map_err(|_| "kv lock poisoned".to_string())?;
        Ok(data.get(cf).map(|m| m.contains_key(key)).unwrap_or(false))
    }

    fn kv_iter_cf(&self, cf: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        let data = self.data.read().map_err(|_| "kv lock poisoned".to_string())?;
        Ok(data
            .get(cf)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default())
    }

    /// Real atomicity: the whole batch is applied under one write lock, so no
    /// reader can observe a partial batch.
    fn kv_write_batch(&self, ops: &[KvOp]) -> Result<(), String> {
        let mut data = self.data.write().map_err(|_| "kv lock poisoned".to_string())?;
        for op in ops {
            match op {
                KvOp::Put { cf, key, value } => {
                    data.entry(cf.clone()).or_default().insert(key.clone(), value.clone());
                }
                KvOp::Delete { cf, key } => {
                    if let Some(m) = data.get_mut(cf) {
                        m.remove(key);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Shared contract every `KvStore` backend must satisfy. Called by both the
/// in-memory tests here and the RocksDB tests, so the two backends are held to
/// the identical contract.
#[cfg(test)]
pub(crate) fn run_conformance(kv: &dyn KvStore) {
    // put / get / exists / delete
    assert_eq!(kv.kv_get("cf", b"k").expect("get"), None);
    kv.kv_put("cf", b"k", b"v").expect("put");
    assert_eq!(kv.kv_get("cf", b"k").expect("get"), Some(b"v".to_vec()));
    assert!(kv.kv_exists("cf", b"k").expect("exists"));
    // overwrite in place
    kv.kv_put("cf", b"k", b"v2").expect("overwrite");
    assert_eq!(kv.kv_get("cf", b"k").expect("get"), Some(b"v2".to_vec()));
    kv.kv_delete("cf", b"k").expect("delete");
    assert!(!kv.kv_exists("cf", b"k").expect("exists"));

    // iteration is key-sorted
    kv.kv_put("it", b"b", b"2").expect("put");
    kv.kv_put("it", b"a", b"1").expect("put");
    kv.kv_put("it", b"c", b"3").expect("put");
    let keys: Vec<_> = kv.kv_iter_cf("it").expect("iter").into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);

    // atomic batch: all puts land, the delete removes
    kv.kv_put("ba", b"old", b"x").expect("put");
    let ops = vec![
        KvOp::Put { cf: "ba".into(), key: b"a".to_vec(), value: b"1".to_vec() },
        KvOp::Put { cf: "ba".into(), key: b"b".to_vec(), value: b"2".to_vec() },
        KvOp::Delete { cf: "ba".into(), key: b"old".to_vec() },
    ];
    kv.kv_write_batch(&ops).expect("batch");
    assert_eq!(kv.kv_get("ba", b"a").expect("get"), Some(b"1".to_vec()));
    assert_eq!(kv.kv_get("ba", b"b").expect("get"), Some(b"2".to_vec()));
    assert_eq!(kv.kv_get("ba", b"old").expect("get"), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_satisfies_kvstore_contract() {
        run_conformance(&InMemoryKv::new());
    }

    #[test]
    fn put_get_delete_exists() {
        let kv = InMemoryKv::new();
        assert_eq!(kv.kv_get("cf", b"k").unwrap(), None);
        kv.kv_put("cf", b"k", b"v").unwrap();
        assert_eq!(kv.kv_get("cf", b"k").unwrap(), Some(b"v".to_vec()));
        assert!(kv.kv_exists("cf", b"k").unwrap());
        kv.kv_delete("cf", b"k").unwrap();
        assert!(!kv.kv_exists("cf", b"k").unwrap());
    }

    #[test]
    fn iter_is_sorted() {
        let kv = InMemoryKv::new();
        kv.kv_put("cf", b"b", b"2").unwrap();
        kv.kv_put("cf", b"a", b"1").unwrap();
        kv.kv_put("cf", b"c", b"3").unwrap();
        let keys: Vec<_> = kv.kv_iter_cf("cf").unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn write_batch_applies_all() {
        let kv = InMemoryKv::new();
        kv.kv_put("cf", b"old", b"x").unwrap();
        let ops = vec![
            KvOp::Put { cf: "cf".into(), key: b"a".to_vec(), value: b"1".to_vec() },
            KvOp::Put { cf: "cf".into(), key: b"b".to_vec(), value: b"2".to_vec() },
            KvOp::Delete { cf: "cf".into(), key: b"old".to_vec() },
        ];
        kv.kv_write_batch(&ops).unwrap();
        assert_eq!(kv.kv_get("cf", b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(kv.kv_get("cf", b"b").unwrap(), Some(b"2".to_vec()));
        assert_eq!(kv.kv_get("cf", b"old").unwrap(), None);
    }
}
