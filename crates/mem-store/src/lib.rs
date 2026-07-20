//! `mem-store` — a generic, content-addressed DAG over a [`kv::KvStore`] backend
//! (WP-0.2).
//!
//! This is the v1 replacement for `citrate_consensus::dag_store::DagStore`, which
//! is hardcoded to GHOSTDAG `Block`s and could not be reused (verified
//! 2026-06-05). `MemoryDagStore<N>` stores arbitrary content-addressed nodes plus
//! typed edges, with atomic batch commits and prefix-scanned neighbour queries.
//!
//! Grow-only + content-addressed = the substrate for the Belnap-CRDT federation
//! merge (a later WP): re-adding the same node/edge is idempotent.

pub mod kv;
#[cfg(feature = "rocksdb")]
pub mod rocks;
pub mod shred;

use std::collections::{HashMap, HashSet, VecDeque};

use serde::de::DeserializeOwned;
use serde::Serialize;

use mem_core::{ContentHash, Edge, EdgeKind, MemoryNode};

use kv::{KvOp, KvStore};

pub mod cf {
    pub const NODES: &str = "mem_nodes";
    pub const EDGES_OUT: &str = "mem_edges_out"; // key: from ‖ to ‖ kind
    pub const EDGES_IN: &str = "mem_edges_in"; // key: to ‖ from ‖ kind
    pub const META: &str = "mem_meta"; // small operational state: cursor, freshness watermark
    pub const KEYS: &str = "mem_keys"; // per-tenant crypto-shred keyring (WP-1.6)
}

/// Every column family a [`MemoryDagStore`] uses. Pass to `RocksKv::open`.
pub const ALL_CFS: &[&str] = &[cf::NODES, cf::EDGES_OUT, cf::EDGES_IN, cf::META, cf::KEYS];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("serialization error: {0}")]
    Serde(String),
    /// Sealing/opening an at-rest envelope failed (WP-1.6). Authentication
    /// failures are hard errors — tampering never reads as "missing".
    #[error("crypto error: {0}")]
    Crypto(String),
}

/// Why a supersession could not be applied. Mirrors the guards of the TLA+
/// `SupersededDag.Supersede` action — every rejected case here is a state the
/// spec never reaches.
#[derive(Debug, thiserror::Error)]
pub enum SupersessionError {
    #[error("edge kind is {0:?}, not Supersedes")]
    NotSupersedes(EdgeKind),
    #[error("a node cannot supersede itself")]
    SelfSupersede,
    #[error("node {0} not in store")]
    MissingNode(String),
    #[error("edge would close a supersession cycle")]
    Cycle,
    #[error("edge is quarantined (a proposal, not a load-bearing supersession)")]
    Quarantined,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What [`MemoryDagStore::apply_supersession`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupersessionReport {
    /// `true` if the target transitioned Active → Superseded now; `false` if it
    /// was already superseded/archived (idempotent re-apply — the edge is still
    /// written, but `status`/`valid_to` are left untouched).
    pub transitioned: bool,
}

/// What [`MemoryDagStore::migrate_seal_v2`] did (ENCRYPT-S1 WP-6 one-shot
/// migration: seal pre-WP-6 plaintext edge/meta rows in place).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SealMigrationReport {
    /// The marker was already present — nothing was scanned.
    pub already_done: bool,
    /// Plaintext edge rows (out + in adjacency) sealed in place.
    pub edges_sealed: usize,
    /// Plaintext meta rows sealed in place.
    pub meta_sealed: usize,
    /// Rows left plaintext because no owning tenant could be determined
    /// (dangling edge with both endpoints gone, or a meta key without the
    /// `prefix:{tenant}` shape). Reported, never silently dropped.
    pub skipped_unknown_tenant: usize,
}

/// META-cf marker row gating [`MemoryDagStore::migrate_seal_v2`]. Plaintext by
/// design (purely operational, carries nothing tenant-derived).
const SEAL_V2_MARKER_KEY: &[u8] = b"__shred_seal_v2_done";

/// Anything the DAG can store must know its own content-addressed id.
pub trait Identified {
    fn id(&self) -> ContentHash;
}

impl Identified for MemoryNode {
    fn id(&self) -> ContentHash {
        self.compute_id()
    }
}

/// Anything the DAG can store belongs to exactly one tenant — the unit of
/// crypto-shredding (WP-1.6: one key per tenant; forget = destroy the key).
pub trait Tenanted {
    fn tenant(&self) -> &str;
}

impl Tenanted for MemoryNode {
    fn tenant(&self) -> &str {
        &self.repo
    }
}

/// A content-addressed DAG of nodes `N` and [`Edge`]s, backed by any [`KvStore`].
pub struct MemoryDagStore<N> {
    kv: Box<dyn KvStore>,
    /// WP-1.6 + ENCRYPT-S1 WP-6: seal node payloads, edge values and
    /// operational-meta values (freshness watermark, anchors) with the owning
    /// tenant's key before they touch the backend. Storage KEYS stay plaintext
    /// — every edge read is a prefix scan by raw node id, issued by readers
    /// that don't know the owning tenant (see `shred` module docs for the
    /// accepted topology-visibility residual).
    encrypt_at_rest: bool,
    _node: std::marker::PhantomData<N>,
}

impl<N> MemoryDagStore<N>
where
    N: Identified + Tenanted + Serialize + DeserializeOwned + Clone,
{
    pub fn new(kv: Box<dyn KvStore>) -> Self {
        Self {
            kv,
            encrypt_at_rest: false,
            _node: std::marker::PhantomData,
        }
    }

    /// A store that seals every node payload, edge value and meta value under
    /// its tenant's key (WP-1.6 + WP-6). Reading is backward-compatible:
    /// plaintext values written by an unencrypted (or pre-WP-6) store still
    /// parse — run [`migrate_seal_v2`](Self::migrate_seal_v2) to seal them.
    pub fn new_encrypted(kv: Box<dyn KvStore>) -> Self {
        Self {
            kv,
            encrypt_at_rest: true,
            _node: std::marker::PhantomData,
        }
    }

    /// Open in whatever mode the data already is: encrypted iff the keyring CF
    /// has entries. Keeps operators (daemon, backfill refresh) from accidentally
    /// writing plaintext into an encrypted store — reads work either way, but
    /// the write mode must match.
    ///
    /// WP-6: on an encrypted store this also runs the one-shot
    /// [`migrate_seal_v2`](Self::migrate_seal_v2) (marker-gated — a single
    /// point read once migrated), so pre-WP-6 stores with plaintext edge/meta
    /// rows get sealed on their first post-upgrade open.
    pub fn new_auto(kv: Box<dyn KvStore>) -> Result<Self, StoreError> {
        let encrypted = !kv.kv_iter_cf(cf::KEYS).map_err(StoreError::Backend)?.is_empty();
        let s = Self {
            kv,
            encrypt_at_rest: encrypted,
            _node: std::marker::PhantomData,
        };
        if encrypted {
            s.migrate_seal_v2()?;
        }
        Ok(s)
    }

    /// Whether this store seals node payloads on write.
    pub fn is_encrypted_at_rest(&self) -> bool {
        self.encrypt_at_rest
    }

    /// Open a durable RocksDB-backed store at `path`, wiring all required column
    /// families. Requires the `rocksdb` feature.
    #[cfg(feature = "rocksdb")]
    pub fn open_rocksdb<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StoreError> {
        let kv = crate::rocks::RocksKv::open(path, ALL_CFS).map_err(StoreError::Backend)?;
        Ok(Self::new(Box::new(kv)))
    }

    /// [`open_rocksdb`](Self::open_rocksdb), but encrypting at rest (WP-1.6).
    #[cfg(feature = "rocksdb")]
    pub fn open_rocksdb_encrypted<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StoreError> {
        let kv = crate::rocks::RocksKv::open(path, ALL_CFS).map_err(StoreError::Backend)?;
        Ok(Self::new_encrypted(Box::new(kv)))
    }

    /// [`open_rocksdb`](Self::open_rocksdb), matching the DB's existing mode:
    /// encrypted iff its keyring has entries (see [`new_auto`](Self::new_auto)).
    #[cfg(feature = "rocksdb")]
    pub fn open_rocksdb_auto<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StoreError> {
        let kv = crate::rocks::RocksKv::open(path, ALL_CFS).map_err(StoreError::Backend)?;
        Self::new_auto(Box::new(kv))
    }

    // ---- crypto-shred keyring (WP-1.6) ----

    fn keyring_entry(&self, tenant: &str) -> Result<Option<shred::KeyringEntry>, StoreError> {
        match self.kv.kv_get(cf::KEYS, tenant.as_bytes()).map_err(StoreError::Backend)? {
            None => Ok(None),
            Some(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).map_err(|e| StoreError::Serde(e.to_string()))?,
            )),
        }
    }

    fn put_keyring_entry(&self, tenant: &str, entry: &shred::KeyringEntry) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(entry).map_err(|e| StoreError::Serde(e.to_string()))?;
        self.kv.kv_put(cf::KEYS, tenant.as_bytes(), &bytes).map_err(StoreError::Backend)
    }

    /// The tenant's live (generation, master key), minting generation 1 — or
    /// the next generation after a shred — on first use.
    fn ensure_tenant_key(&self, tenant: &str) -> Result<(u32, [u8; shred::KEY_LEN]), StoreError> {
        let (gen, existing) = match self.keyring_entry(tenant)? {
            Some(e) => (e.gen, e.key_bytes()?),
            None => (0, None),
        };
        if let Some(key) = existing {
            return Ok((gen, key));
        }
        // Absent or shredded: mint the next generation with fresh key material.
        let key = shred::generate_key()?;
        let entry = shred::KeyringEntry { gen: gen + 1, key: Some(hex::encode(key)) };
        self.put_keyring_entry(tenant, &entry)?;
        Ok((gen + 1, key))
    }

    /// Crypto-shred a tenant: destroy its key material (WP-1.6, decision #10).
    /// Every payload sealed under the destroyed generation becomes permanently
    /// unreadable — reads return `None`/skip, as if forgotten — while the
    /// ciphertext itself stays put (it can keep federating). The generation
    /// counter survives, so a tenant that writes again gets a *fresh* key under
    /// the next generation and its pre-shred history stays lost. Returns
    /// whether live key material existed.
    pub fn shred_tenant(&self, tenant: &str) -> Result<bool, StoreError> {
        match self.keyring_entry(tenant)? {
            None => Ok(false),
            Some(e) => {
                let existed = e.key.is_some();
                self.put_keyring_entry(tenant, &shred::KeyringEntry { gen: e.gen, key: None })?;
                Ok(existed)
            }
        }
    }

    /// One-shot WP-6 migration: seal every pre-WP-6 **plaintext** edge and
    /// meta row in place under its owning tenant's key. Chosen over
    /// rebuild-from-ingest because only the Derived plane is rebuildable —
    /// Asserted-plane edges (signed propose/confirm proposals) exist nowhere
    /// but in this store, so they must be re-encrypted, not re-derived.
    ///
    /// Properties:
    /// - **Marker-gated**: once complete, a `__shred_seal_v2_done` row in the
    ///   META cf short-circuits every later call to a single point read
    ///   ([`new_auto`](Self::new_auto) runs this on every encrypted open).
    /// - **Idempotent**: already-sealed rows are skipped by shape, so a crash
    ///   mid-migration (marker unwritten) safely re-runs.
    /// - **Per-tenant**: each row is sealed under its own tenant's live key
    ///   (edges via their `from`/`to` endpoint, meta via its `prefix:{tenant}`
    ///   key shape), minting keys for tenants that never wrote a node.
    /// - Rows whose tenant cannot be determined are left plaintext and
    ///   **counted** in the report — the marker is still written (those rows
    ///   are unprotectable dangling data, and re-scanning every open would
    ///   not change that).
    pub fn migrate_seal_v2(&self) -> Result<SealMigrationReport, StoreError> {
        if !self.encrypt_at_rest {
            return Err(StoreError::Crypto("seal migration requires an encrypted store".into()));
        }
        if self.kv.kv_get(cf::META, SEAL_V2_MARKER_KEY).map_err(StoreError::Backend)?.is_some() {
            return Ok(SealMigrationReport { already_done: true, ..Default::default() });
        }
        let mut report = SealMigrationReport::default();
        let empty_batch = HashMap::new();
        for cf_name in [cf::EDGES_OUT, cf::EDGES_IN] {
            for (k, v) in self.kv.kv_iter_cf(cf_name).map_err(StoreError::Backend)? {
                if shred::parse_envelope(&v).is_some() {
                    continue; // already sealed
                }
                let edge: Edge = serde_json::from_slice(&v).map_err(|e| StoreError::Serde(e.to_string()))?;
                let tenant = match self.edge_tenant(&edge, &empty_batch) {
                    Ok(t) => t,
                    // Dangling edge, both endpoints gone: unprotectable — count it.
                    Err(StoreError::Crypto(_)) => {
                        report.skipped_unknown_tenant += 1;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let (gen, master) = self.ensure_tenant_key(&tenant)?;
                let sealed = shred::seal_at(&master, &tenant, gen, cf_name, &k, &v)?;
                self.kv.kv_put(cf_name, &k, &sealed).map_err(StoreError::Backend)?;
                report.edges_sealed += 1;
            }
        }
        for (k, v) in self.kv.kv_iter_cf(cf::META).map_err(StoreError::Backend)? {
            if k == SEAL_V2_MARKER_KEY || shred::parse_envelope(&v).is_some() {
                continue;
            }
            // Meta keys are `prefix:{tenant}` (derived_watermark:, anchor:,
            // chain_anchor:) — the tenant is the suffix after the first ':'.
            let tenant = k
                .iter()
                .position(|&b| b == b':')
                .and_then(|i| std::str::from_utf8(&k[i + 1..]).ok())
                .filter(|t| !t.is_empty());
            let Some(tenant) = tenant else {
                report.skipped_unknown_tenant += 1;
                continue;
            };
            let (gen, master) = self.ensure_tenant_key(tenant)?;
            let sealed = shred::seal_at(&master, tenant, gen, cf::META, &k, &v)?;
            self.kv.kv_put(cf::META, &k, &sealed).map_err(StoreError::Backend)?;
            report.meta_sealed += 1;
        }
        self.kv
            .kv_put(cf::META, SEAL_V2_MARKER_KEY, b"1")
            .map_err(StoreError::Backend)?;
        Ok(report)
    }

    /// Serialize a node for storage: plaintext JSON, or a sealed envelope when
    /// encrypting at rest.
    fn encode_node(&self, node: &N) -> Result<Vec<u8>, StoreError> {
        let plain = serde_json::to_vec(node).map_err(|e| StoreError::Serde(e.to_string()))?;
        if !self.encrypt_at_rest {
            return Ok(plain);
        }
        let tenant = node.tenant();
        let (gen, key) = self.ensure_tenant_key(tenant)?;
        shred::seal(&key, tenant, gen, &node.id(), &plain)
    }

    /// Decode a stored node value. `Ok(None)` means the value is sealed under a
    /// destroyed key (the tenant was crypto-shredded — its generation's key
    /// material is gone, or the whole keyring row is). An authentication
    /// failure under the *live* matching generation is a hard error: that is
    /// tampering, not forgetting.
    fn decode_node(&self, id: &ContentHash, bytes: &[u8]) -> Result<Option<N>, StoreError> {
        let Some(env) = shred::parse_envelope(bytes) else {
            return Ok(Some(serde_json::from_slice(bytes).map_err(|e| StoreError::Serde(e.to_string()))?));
        };
        let Some(entry) = self.keyring_entry(&env.tenant)? else {
            return Ok(None);
        };
        let Some(key) = entry.key_bytes()? else {
            return Ok(None);
        };
        if entry.gen != env.kgen {
            return Ok(None);
        }
        let plain = shred::open(&key, &env, id)?;
        Ok(Some(serde_json::from_slice(&plain).map_err(|e| StoreError::Serde(e.to_string()))?))
    }

    // ---- edge/meta sealing (ENCRYPT-S1 WP-6) ----

    /// The tenant that owns a stored node, WITHOUT decrypting it: a sealed
    /// envelope carries its tenant label in the clear (the reader must know
    /// which key to fetch), and a plaintext node carries its repo. `None` if
    /// the node isn't stored. Works even for crypto-shredded nodes — an edge
    /// written after its endpoint's shred still resolves to the right tenant
    /// (and gets sealed under that tenant's next-generation key).
    fn stored_node_tenant(&self, id: &ContentHash) -> Result<Option<String>, StoreError> {
        let Some(bytes) = self.kv.kv_get(cf::NODES, id.as_bytes()).map_err(StoreError::Backend)? else {
            return Ok(None);
        };
        if let Some(env) = shred::parse_envelope(&bytes) {
            return Ok(Some(env.tenant));
        }
        let n: N = serde_json::from_slice(&bytes).map_err(|e| StoreError::Serde(e.to_string()))?;
        Ok(Some(n.tenant().to_string()))
    }

    /// The tenant an edge is sealed under: the tenant of its `from` (asserting)
    /// endpoint, falling back to `to`. `batch` lets `commit` resolve endpoints
    /// that arrive in the same atomic write. Only called when encrypting at
    /// rest — there, an edge with no known endpoint is unprotectable and is
    /// rejected rather than silently stored in the clear.
    fn edge_tenant(&self, edge: &Edge, batch: &HashMap<ContentHash, String>) -> Result<String, StoreError> {
        for id in [&edge.from, &edge.to] {
            if let Some(t) = batch.get(id) {
                return Ok(t.clone());
            }
            if let Some(t) = self.stored_node_tenant(id)? {
                return Ok(t);
            }
        }
        Err(StoreError::Crypto(format!(
            "cannot seal edge {} -> {}: neither endpoint is in the store, so no tenant key applies",
            edge.from.to_hex(),
            edge.to.to_hex()
        )))
    }

    /// The two `KvOp::Put`s an edge write expands to (out- and in-adjacency),
    /// sealing each row under the owning tenant's key when encrypting at rest.
    /// The AAD binds tenant ‖ cf ‖ storage-key, so the two rows carry distinct
    /// ciphertexts and neither can be replayed into the other's slot.
    fn edge_put_ops(&self, edge: &Edge, batch: &HashMap<ContentHash, String>) -> Result<[KvOp; 2], StoreError> {
        let plain = serde_json::to_vec(edge).map_err(|e| StoreError::Serde(e.to_string()))?;
        let (out_key, in_key) = (edge.key(), Self::in_key(edge));
        let (out_val, in_val) = if self.encrypt_at_rest {
            let tenant = self.edge_tenant(edge, batch)?;
            let (gen, key) = self.ensure_tenant_key(&tenant)?;
            (
                shred::seal_at(&key, &tenant, gen, cf::EDGES_OUT, &out_key, &plain)?,
                shred::seal_at(&key, &tenant, gen, cf::EDGES_IN, &in_key, &plain)?,
            )
        } else {
            (plain.clone(), plain)
        };
        Ok([
            KvOp::Put { cf: cf::EDGES_OUT.into(), key: out_key, value: out_val },
            KvOp::Put { cf: cf::EDGES_IN.into(), key: in_key, value: in_val },
        ])
    }

    /// Decode a stored edge value. `Ok(None)` means sealed under a destroyed
    /// key — the tenant was crypto-shredded, so its relationships read as
    /// forgotten (same contract as [`decode_node`](Self::decode_node)); a
    /// failure under the live matching generation is tampering and errors.
    /// `keys` caches keyring lookups across one scan.
    fn decode_edge(
        &self,
        cf: &str,
        key: &[u8],
        bytes: &[u8],
        keys: &mut HashMap<String, Option<(u32, [u8; shred::KEY_LEN])>>,
    ) -> Result<Option<Edge>, StoreError> {
        let Some(env) = shred::parse_envelope(bytes) else {
            return Ok(Some(serde_json::from_slice(bytes).map_err(|e| StoreError::Serde(e.to_string()))?));
        };
        let live = match keys.get(&env.tenant) {
            Some(cached) => *cached,
            None => {
                let live = match self.keyring_entry(&env.tenant)? {
                    None => None,
                    Some(e) => e.key_bytes()?.map(|k| (e.gen, k)),
                };
                keys.insert(env.tenant.clone(), live);
                live
            }
        };
        let Some((gen, master)) = live else { return Ok(None) };
        if gen != env.kgen {
            return Ok(None);
        }
        let plain = shred::open_at(&master, &env, cf, key)?;
        Ok(Some(serde_json::from_slice(&plain).map_err(|e| StoreError::Serde(e.to_string()))?))
    }

    // ---- nodes ----

    /// Insert (or overwrite-in-place) a node. Content-addressing means an
    /// overwrite with the same id carries the same identity-bearing content;
    /// only advisory fields (embedding, confidence, status) can differ.
    pub fn put_node(&self, node: &N) -> Result<ContentHash, StoreError> {
        let id = node.id();
        let bytes = self.encode_node(node)?;
        self.kv
            .kv_put(cf::NODES, id.as_bytes(), &bytes)
            .map_err(StoreError::Backend)?;
        Ok(id)
    }

    /// `Ok(None)` for absent nodes — and for crypto-shredded ones (WP-1.6):
    /// a tenant whose key was destroyed reads as forgotten.
    pub fn get_node(&self, id: &ContentHash) -> Result<Option<N>, StoreError> {
        match self.kv.kv_get(cf::NODES, id.as_bytes()).map_err(StoreError::Backend)? {
            None => Ok(None),
            Some(bytes) => self.decode_node(id, &bytes),
        }
    }

    pub fn has_node(&self, id: &ContentHash) -> Result<bool, StoreError> {
        self.kv.kv_exists(cf::NODES, id.as_bytes()).map_err(StoreError::Backend)
    }

    pub fn node_count(&self) -> Result<usize, StoreError> {
        Ok(self.kv.kv_iter_cf(cf::NODES).map_err(StoreError::Backend)?.len())
    }

    pub fn edge_count(&self) -> Result<usize, StoreError> {
        Ok(self.kv.kv_iter_cf(cf::EDGES_OUT).map_err(StoreError::Backend)?.len())
    }

    /// Write a consistent point-in-time snapshot of the store to `dest` (which
    /// must not already exist). For the RocksDB backend this is a cheap,
    /// hard-linked recovery point; the in-memory backend reports `Unsupported`.
    /// The snapshot is a complete, independently-openable store (sealed values +
    /// keyring carried verbatim — no decryption involved). The daemon uses this
    /// to keep rolling recovery points so an unclean death can't strand the only
    /// on-disk copy.
    pub fn checkpoint<P: AsRef<std::path::Path>>(&self, dest: P) -> Result<(), StoreError> {
        self.kv.kv_checkpoint(dest.as_ref()).map_err(StoreError::Backend)
    }

    /// Deserialize every node. Linear scan — fine for CLI/report use; recall uses
    /// targeted queries instead. Crypto-shredded nodes (sealed, key destroyed)
    /// are skipped — forgotten, not an error.
    pub fn all_nodes(&self) -> Result<Vec<N>, StoreError> {
        let mut out = Vec::new();
        for (k, v) in self.kv.kv_iter_cf(cf::NODES).map_err(StoreError::Backend)? {
            let id_bytes: [u8; 32] = k
                .as_slice()
                .try_into()
                .map_err(|_| StoreError::Serde("node key is not a 32-byte content hash".into()))?;
            if let Some(node) = self.decode_node(&ContentHash(id_bytes), &v)? {
                out.push(node);
            }
        }
        Ok(out)
    }

    /// Every edge in the store (one scan of the out-edge CF). For readers that need
    /// global graph structure — e.g. reachability of a commit from a branch tip
    /// (ADR-09 in-flight layer). O(edges).
    pub fn all_edges(&self) -> Result<Vec<Edge>, StoreError> {
        self.scan_prefix(cf::EDGES_OUT, &[])
    }

    // ---- operational meta (cursor, freshness watermark) ----

    /// Store a small operational value (not part of the graph) owned by
    /// `tenant` — the freshness watermark and sync anchors are per-repo state,
    /// and WP-6 seals them under that repo's key so a crypto-shred forgets a
    /// tenant's freshness/anchor trail along with its nodes and edges.
    pub fn put_meta(&self, tenant: &str, key: &[u8], value: &[u8]) -> Result<(), StoreError> {
        let bytes = if self.encrypt_at_rest {
            let (gen, master) = self.ensure_tenant_key(tenant)?;
            shred::seal_at(&master, tenant, gen, cf::META, key, value)?
        } else {
            value.to_vec()
        };
        self.kv.kv_put(cf::META, key, &bytes).map_err(StoreError::Backend)
    }

    /// `Ok(None)` for absent values — and for values sealed under a destroyed
    /// key (the owning tenant was crypto-shredded). Plaintext (pre-WP-6)
    /// values pass through unchanged.
    pub fn get_meta(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        let Some(bytes) = self.kv.kv_get(cf::META, key).map_err(StoreError::Backend)? else {
            return Ok(None);
        };
        let Some(env) = shred::parse_envelope(&bytes) else {
            return Ok(Some(bytes));
        };
        let Some(entry) = self.keyring_entry(&env.tenant)? else {
            return Ok(None);
        };
        let Some(master) = entry.key_bytes()? else {
            return Ok(None);
        };
        if entry.gen != env.kgen {
            return Ok(None);
        }
        Ok(Some(shred::open_at(&master, &env, cf::META, key)?))
    }

    // ---- edges ----

    fn in_key(edge: &Edge) -> Vec<u8> {
        let mut k = Vec::with_capacity(65);
        k.extend_from_slice(edge.to.as_bytes());
        k.extend_from_slice(edge.from.as_bytes());
        k.push(edge.kind.tag());
        k
    }

    /// Add an edge. Written to both the out- and in-adjacency CFs in one atomic
    /// batch so a half-written edge can never be observed. Idempotent by
    /// (from, to, kind). When encrypting at rest the value is sealed under the
    /// `from` endpoint's tenant key (fallback: `to`) — see `shred` module docs.
    pub fn add_edge(&self, edge: &Edge) -> Result<(), StoreError> {
        let ops = self.edge_put_ops(edge, &HashMap::new())?;
        self.kv.kv_write_batch(&ops).map_err(StoreError::Backend)
    }

    /// Atomically commit a batch of nodes and edges (the unit a memory-diff
    /// merge or an ingest tick uses). Edges may reference nodes arriving in
    /// the same batch — tenant resolution sees them before they are written.
    pub fn commit(&self, nodes: &[N], edges: &[Edge]) -> Result<(), StoreError> {
        let mut ops = Vec::with_capacity(nodes.len() + edges.len() * 2);
        let mut batch_tenants: HashMap<ContentHash, String> = HashMap::with_capacity(nodes.len());
        for n in nodes {
            let bytes = self.encode_node(n)?;
            batch_tenants.insert(n.id(), n.tenant().to_string());
            ops.push(KvOp::Put {
                cf: cf::NODES.into(),
                key: n.id().as_bytes().to_vec(),
                value: bytes,
            });
        }
        for e in edges {
            ops.extend(self.edge_put_ops(e, &batch_tenants)?);
        }
        self.kv.kv_write_batch(&ops).map_err(StoreError::Backend)
    }

    fn scan_prefix(&self, cf: &str, prefix: &[u8]) -> Result<Vec<Edge>, StoreError> {
        let mut out = Vec::new();
        let mut keys = HashMap::new();
        for (k, v) in self.kv.kv_iter_cf(cf).map_err(StoreError::Backend)? {
            if k.starts_with(prefix) {
                // Crypto-shredded edges decode to None: forgotten, not an error.
                if let Some(e) = self.decode_edge(cf, &k, &v, &mut keys)? {
                    out.push(e);
                }
            }
        }
        Ok(out)
    }

    /// Edges leaving `from`.
    pub fn out_edges(&self, from: &ContentHash) -> Result<Vec<Edge>, StoreError> {
        self.scan_prefix(cf::EDGES_OUT, from.as_bytes())
    }

    /// Edges entering `to`.
    pub fn in_edges(&self, to: &ContentHash) -> Result<Vec<Edge>, StoreError> {
        self.scan_prefix(cf::EDGES_IN, to.as_bytes())
    }

    /// BFS over out-edges of a single kind. Returns reachable node ids (excluding
    /// the start). Cycle-safe via a visited set — important because supersession
    /// is *supposed* to be acyclic but we must never loop even if a bad edge
    /// slips in.
    pub fn reachable_via(&self, start: &ContentHash, kind: EdgeKind) -> Result<Vec<ContentHash>, StoreError> {
        let mut seen: HashSet<ContentHash> = HashSet::new();
        let mut queue: VecDeque<ContentHash> = VecDeque::new();
        queue.push_back(*start);
        seen.insert(*start);
        let mut result = Vec::new();
        while let Some(cur) = queue.pop_front() {
            for e in self.out_edges(&cur)? {
                if e.kind == kind && seen.insert(e.to) {
                    result.push(e.to);
                    queue.push_back(e.to);
                }
            }
        }
        Ok(result)
    }

    /// Would adding `from -supersedes-> to` create a cycle? (i.e. is `from`
    /// already reachable from `to` via Supersedes?) Enforces the TLA+
    /// `SupersededDag.Acyclic` invariant at the write boundary.
    pub fn would_cycle_supersedes(&self, from: &ContentHash, to: &ContentHash) -> Result<bool, StoreError> {
        Ok(self.reachable_via(to, EdgeKind::Supersedes)?.contains(from))
    }
}

/// What [`MemoryDagStore::confirm_edge`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// The edge was quarantined and is now load-bearing. For a `Supersedes`
    /// edge this includes the status transition (routed through
    /// `apply_supersession`).
    Confirmed,
    /// The edge was already load-bearing — idempotent no-op.
    AlreadyConfirmed,
    /// No such edge.
    NotFound,
}

impl MemoryDagStore<MemoryNode> {
    /// Promote a quarantined (proposed) edge to load-bearing (MEM-S4 WP-4.1,
    /// the confirm half of the propose→quarantine→confirm lifecycle, R1).
    /// Identified by (from, to, kind) — the edge's identity key. The
    /// `quarantined` flag is outside the signed message (`Edge::key`), so
    /// flipping it preserves the proposer's signature. Confirming a
    /// `Supersedes` edge routes through [`apply_supersession`]
    /// (cycle-guarded, atomic edge+status), so a proposal can never transition
    /// anyone's status before confirmation.
    pub fn confirm_edge(
        &self,
        from: &ContentHash,
        to: &ContentHash,
        kind: EdgeKind,
    ) -> Result<ConfirmOutcome, SupersessionError> {
        let Some(mut edge) = self
            .out_edges(from)?
            .into_iter()
            .find(|e| e.to == *to && e.kind == kind)
        else {
            return Ok(ConfirmOutcome::NotFound);
        };
        if !edge.quarantined {
            return Ok(ConfirmOutcome::AlreadyConfirmed);
        }
        edge.quarantined = false;
        if edge.kind == EdgeKind::Supersedes {
            self.apply_supersession(&edge)?;
        } else {
            self.add_edge(&edge)?;
        }
        Ok(ConfirmOutcome::Confirmed)
    }

    /// Apply a supersession (WP-1.4): write `from -Supersedes-> to` and
    /// transition the target Active → Superseded (stamping `valid_to` from the
    /// edge's provenance time) in ONE atomic batch — a reader can never observe
    /// the edge without the status, or vice versa.
    ///
    /// This is the runtime image of the TLA+ `SupersededDag.Supersede` action:
    /// both endpoints must exist, self-supersession is rejected, the
    /// `would_cycle_supersedes` guard preserves `Acyclic`, and the transition
    /// preserves `LatestWellDefined` (an Active node has no incoming
    /// supersedes edge). Re-applying the same edge is idempotent: the edge
    /// rewrites in place and an already-superseded target keeps its original
    /// `status`/`valid_to`. Quarantined edges are rejected — a proposal must be
    /// confirmed (de-quarantined) before it can change anyone's status.
    pub fn apply_supersession(&self, edge: &Edge) -> Result<SupersessionReport, SupersessionError> {
        if edge.kind != EdgeKind::Supersedes {
            return Err(SupersessionError::NotSupersedes(edge.kind));
        }
        if edge.quarantined {
            return Err(SupersessionError::Quarantined);
        }
        if edge.from == edge.to {
            return Err(SupersessionError::SelfSupersede);
        }
        if !self.has_node(&edge.from)? {
            return Err(SupersessionError::MissingNode(edge.from.to_hex()));
        }
        let mut target = self
            .get_node(&edge.to)?
            .ok_or_else(|| SupersessionError::MissingNode(edge.to.to_hex()))?;
        if self.would_cycle_supersedes(&edge.from, &edge.to)? {
            return Err(SupersessionError::Cycle);
        }

        let transitioned = target.status == mem_core::Status::Active;
        // Sealed under the from-endpoint's tenant key when encrypting at rest
        // (both endpoints are guaranteed present by the guards above).
        let mut ops: Vec<KvOp> = self.edge_put_ops(edge, &HashMap::new())?.into();
        if transitioned {
            target.status = mem_core::Status::Superseded;
            target.valid_to = Some(edge.provenance.at);
            // status/valid_to are excluded from compute_id, so this overwrites in
            // place (sealed under the tenant's key when encrypting at rest).
            let node_bytes = self.encode_node(&target)?;
            ops.push(KvOp::Put { cf: cf::NODES.into(), key: edge.to.as_bytes().to_vec(), value: node_bytes });
        }
        self.kv.kv_write_batch(&ops).map_err(StoreError::Backend)?;
        Ok(SupersessionReport { transitioned })
    }

    /// Overwrite a node's advisory `status` (and optionally `valid_to`) in place.
    /// `status`/`valid_to` are outside `compute_id`, so the node id is preserved —
    /// this is an advisory update, not a new node. Returns `true` if the node
    /// existed and its status actually changed. Used by the in-flight branch layer
    /// (ADR-09 B.3) to archive merged/deleted branch nodes.
    pub fn set_node_status(
        &self,
        id: &ContentHash,
        status: mem_core::Status,
        valid_to: Option<mem_core::Timestamp>,
    ) -> Result<bool, StoreError> {
        let mut node = match self.get_node(id)? {
            Some(n) => n,
            None => return Ok(false),
        };
        if node.status == status {
            return Ok(false);
        }
        node.status = status;
        if valid_to.is_some() {
            node.valid_to = valid_to;
        }
        let node_bytes = self.encode_node(&node)?;
        self.kv
            .kv_write_batch(&[KvOp::Put { cf: cf::NODES.into(), key: id.as_bytes().to_vec(), value: node_bytes }])
            .map_err(StoreError::Backend)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kv::InMemoryKv;
    use mem_core::{
        Edge, EdgeKind, EdgeMethod, EdgeProvenance, MemoryNode, NodeKind, Plane, SourceRef, Status,
        TrustTier, SCHEMA_VERSION,
    };

    fn node(content: &str) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Commit,
            repo: "r".into(),
            author: "ingest".into(),
            source_ref: SourceRef::DagNative { key: content.into() },
            content: content.as_bytes().to_vec(),
            valid_from: 0,
            valid_to: None,
            observed_at: 0,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: None,
            confidence: vec![],
            anchors: vec![],
            status: Status::Active,
        }
    }

    fn edge(from: &MemoryNode, to: &MemoryNode, kind: EdgeKind) -> Edge {
        Edge {
            from: from.compute_id(),
            to: to.compute_id(),
            kind,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance {
                method: EdgeMethod::Ingest,
                asserter: "ingest".into(),
                at: 0,
                evidence: None,
            },
            confidence: vec![],
            quarantined: false,
            signature: None,
        }
    }

    fn store() -> MemoryDagStore<MemoryNode> {
        MemoryDagStore::new(Box::new(InMemoryKv::new()))
    }

    #[test]
    fn put_then_get_roundtrips() {
        let s = store();
        let n = node("a");
        let id = s.put_node(&n).unwrap();
        assert_eq!(id, n.compute_id());
        assert_eq!(s.get_node(&id).unwrap(), Some(n));
        assert!(s.has_node(&id).unwrap());
        assert_eq!(s.node_count().unwrap(), 1);
    }

    #[test]
    fn put_is_idempotent_by_id() {
        let s = store();
        let n = node("a");
        s.put_node(&n).unwrap();
        s.put_node(&n).unwrap();
        assert_eq!(s.node_count().unwrap(), 1, "same content -> same id -> one node");
    }

    #[test]
    fn edges_are_queryable_both_directions() {
        let s = store();
        let (a, b) = (node("a"), node("b"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        s.add_edge(&edge(&a, &b, EdgeKind::Implements)).unwrap();

        let out = s.out_edges(&a.compute_id()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, b.compute_id());

        let inc = s.in_edges(&b.compute_id()).unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].from, a.compute_id());

        // No phantom edges the other way.
        assert!(s.out_edges(&b.compute_id()).unwrap().is_empty());
    }

    #[test]
    fn commit_is_atomic_batch() {
        let s = store();
        let (a, b, c) = (node("a"), node("b"), node("c"));
        let edges = vec![edge(&a, &b, EdgeKind::TemporalNext), edge(&b, &c, EdgeKind::TemporalNext)];
        s.commit(&[a.clone(), b.clone(), c.clone()], &edges).unwrap();
        assert_eq!(s.node_count().unwrap(), 3);
        assert_eq!(s.out_edges(&a.compute_id()).unwrap().len(), 1);
    }

    #[test]
    fn reachable_and_cycle_guard() {
        let s = store();
        let (a, b, c) = (node("a"), node("b"), node("c"));
        for n in [&a, &b, &c] {
            s.put_node(n).unwrap();
        }
        // a supersedes b, b supersedes c
        s.add_edge(&edge(&a, &b, EdgeKind::Supersedes)).unwrap();
        s.add_edge(&edge(&b, &c, EdgeKind::Supersedes)).unwrap();

        let reach = s.reachable_via(&a.compute_id(), EdgeKind::Supersedes).unwrap();
        assert!(reach.contains(&b.compute_id()));
        assert!(reach.contains(&c.compute_id()));

        // Adding c -supersedes-> a would close a cycle a->b->c->a.
        assert!(s
            .would_cycle_supersedes(&c.compute_id(), &a.compute_id())
            .unwrap());
        // Adding a fresh node superseding a does not.
        let d = node("d");
        s.put_node(&d).unwrap();
        assert!(!s
            .would_cycle_supersedes(&d.compute_id(), &a.compute_id())
            .unwrap());
    }

    fn supersedes_at(from: &MemoryNode, to: &MemoryNode, at: u64) -> Edge {
        let mut e = edge(from, to, EdgeKind::Supersedes);
        e.provenance.at = at;
        e
    }

    #[test]
    fn apply_supersession_transitions_target_atomically() {
        let s = store();
        let (new, old) = (node("new understanding"), node("old understanding"));
        s.put_node(&new).unwrap();
        s.put_node(&old).unwrap();

        let report = s.apply_supersession(&supersedes_at(&new, &old, 42)).unwrap();
        assert!(report.transitioned);

        let old_now = s.get_node(&old.compute_id()).unwrap().unwrap();
        assert_eq!(old_now.status, Status::Superseded);
        assert_eq!(old_now.valid_to, Some(42), "valid_to stamped from edge provenance");
        // Edge visible in both directions (LatestWellDefined: active ⇒ no in-edge).
        assert_eq!(s.out_edges(&new.compute_id()).unwrap().len(), 1);
        assert_eq!(s.in_edges(&old.compute_id()).unwrap().len(), 1);
        // The superseder itself stays active.
        assert_eq!(s.get_node(&new.compute_id()).unwrap().unwrap().status, Status::Active);
    }

    #[test]
    fn apply_supersession_is_idempotent_and_preserves_first_valid_to() {
        let s = store();
        let (a, b, c) = (node("a"), node("b"), node("c"));
        for n in [&a, &b, &c] {
            s.put_node(n).unwrap();
        }
        assert!(s.apply_supersession(&supersedes_at(&a, &c, 10)).unwrap().transitioned);
        // Re-apply: no transition, valid_to untouched.
        assert!(!s.apply_supersession(&supersedes_at(&a, &c, 99)).unwrap().transitioned);
        // A second superseder of the same (already superseded) target: edge lands,
        // but status/valid_to stay as the first transition wrote them.
        assert!(!s.apply_supersession(&supersedes_at(&b, &c, 99)).unwrap().transitioned);
        let c_now = s.get_node(&c.compute_id()).unwrap().unwrap();
        assert_eq!(c_now.status, Status::Superseded);
        assert_eq!(c_now.valid_to, Some(10));
        assert_eq!(s.in_edges(&c.compute_id()).unwrap().len(), 2);
    }

    #[test]
    fn apply_supersession_rejects_cycle_without_partial_write() {
        let s = store();
        let (a, b) = (node("a"), node("b"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        s.apply_supersession(&supersedes_at(&a, &b, 1)).unwrap();

        let err = s.apply_supersession(&supersedes_at(&b, &a, 2)).unwrap_err();
        assert!(matches!(err, SupersessionError::Cycle));
        // Rejection is total: no edge written, superseder a still active.
        assert!(s.in_edges(&a.compute_id()).unwrap().is_empty());
        assert_eq!(s.get_node(&a.compute_id()).unwrap().unwrap().status, Status::Active);
    }

    fn tenant_node(repo: &str, content: &str) -> MemoryNode {
        let mut n = node(content);
        n.repo = repo.into();
        n
    }

    fn enc_store() -> MemoryDagStore<MemoryNode> {
        MemoryDagStore::new_encrypted(Box::new(InMemoryKv::new()))
    }

    #[test]
    fn encrypted_store_roundtrips_and_hides_plaintext_at_rest() {
        let s = enc_store();
        let n = tenant_node("r1", "TOP-SECRET design rationale");
        let id = s.put_node(&n).unwrap();
        assert_eq!(s.get_node(&id).unwrap(), Some(n.clone()), "sealed roundtrip");
        assert_eq!(s.all_nodes().unwrap(), vec![n]);

        // The REAL at-rest check: raw backend bytes are an envelope and contain
        // no plaintext.
        let raw = s.kv.kv_get(cf::NODES, id.as_bytes()).unwrap().unwrap();
        assert!(shred::parse_envelope(&raw).is_some(), "value stored as sealed envelope");
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(!raw_str.contains("TOP-SECRET"), "plaintext must not touch the backend");

        // Determinism: re-putting the same node writes identical bytes.
        s.put_node(&s.get_node(&id).unwrap().unwrap()).unwrap();
        assert_eq!(s.kv.kv_get(cf::NODES, id.as_bytes()).unwrap().unwrap(), raw);
    }

    #[test]
    fn encrypted_store_seals_edges_at_rest() {
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "node a"), tenant_node("r1", "node b"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        let mut e = edge(&a, &b, EdgeKind::Refutes);
        e.provenance.asserter = "SECRET-ASSERTER".into();
        e.provenance.evidence = Some("SECRET-EVIDENCE rationale".into());
        s.add_edge(&e).unwrap();

        // Round-trip through both adjacency views.
        assert_eq!(s.out_edges(&a.compute_id()).unwrap(), vec![e.clone()]);
        assert_eq!(s.in_edges(&b.compute_id()).unwrap(), vec![e.clone()]);

        // The at-rest probe: both raw rows are envelopes and leak no content.
        for (cf_name, key) in [(cf::EDGES_OUT, e.key()), (cf::EDGES_IN, MemoryDagStore::<MemoryNode>::in_key(&e))] {
            let raw = s.kv.kv_get(cf_name, &key).unwrap().unwrap();
            assert!(shred::parse_envelope(&raw).is_some(), "{cf_name} value stored sealed");
            let raw_str = String::from_utf8_lossy(&raw);
            assert!(!raw_str.contains("SECRET"), "{cf_name} must not leak edge content");
            assert!(!raw_str.contains("quarantined"), "{cf_name} must not leak edge structure fields");
        }

        // Determinism: re-adding the same edge rewrites identical bytes.
        let raw_before = s.kv.kv_get(cf::EDGES_OUT, &e.key()).unwrap().unwrap();
        s.add_edge(&e).unwrap();
        assert_eq!(s.kv.kv_get(cf::EDGES_OUT, &e.key()).unwrap().unwrap(), raw_before);
    }

    #[test]
    fn encrypted_store_seals_meta_at_rest() {
        let s = enc_store();
        let key = b"derived_watermark:r1";
        s.put_meta("r1", key, b"SECRET-HEAD-SHA state").unwrap();
        assert_eq!(s.get_meta(key).unwrap().unwrap(), b"SECRET-HEAD-SHA state");

        let raw = s.kv.kv_get(cf::META, key).unwrap().unwrap();
        assert!(shred::parse_envelope(&raw).is_some(), "meta value stored sealed");
        assert!(!String::from_utf8_lossy(&raw).contains("SECRET"), "meta must not leak");
    }

    #[test]
    fn encrypted_commit_resolves_tenants_from_the_batch() {
        // Edges referencing nodes that arrive in the SAME atomic batch.
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "batch a"), tenant_node("r1", "batch b"));
        let e = edge(&a, &b, EdgeKind::TemporalNext);
        s.commit(&[a.clone(), b.clone()], std::slice::from_ref(&e)).unwrap();
        assert_eq!(s.out_edges(&a.compute_id()).unwrap(), vec![e.clone()]);
        let raw = s.kv.kv_get(cf::EDGES_OUT, &e.key()).unwrap().unwrap();
        assert!(shred::parse_envelope(&raw).is_some());
    }

    #[test]
    fn encrypted_add_edge_rejects_unknown_endpoints() {
        // An edge with no stored endpoint has no tenant key to seal under —
        // storing it in the clear would silently undermine crypto-shred.
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "never stored a"), tenant_node("r1", "never stored b"));
        let err = s.add_edge(&edge(&a, &b, EdgeKind::References)).unwrap_err();
        assert!(matches!(err, StoreError::Crypto(_)));
    }

    #[test]
    fn tampered_edge_ciphertext_is_a_hard_error_not_a_skip() {
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "ta"), tenant_node("r1", "tb"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        let e = edge(&a, &b, EdgeKind::Implements);
        s.add_edge(&e).unwrap();

        let mut raw = s.kv.kv_get(cf::EDGES_OUT, &e.key()).unwrap().unwrap();
        let pos = String::from_utf8_lossy(&raw).find("\"ct\":\"").unwrap() + 7;
        raw[pos] = if raw[pos] == b'0' { b'1' } else { b'0' };
        s.kv.kv_put(cf::EDGES_OUT, &e.key(), &raw).unwrap();
        assert!(matches!(s.out_edges(&a.compute_id()).unwrap_err(), StoreError::Crypto(_)));

        // Replaying a valid ciphertext into another slot also fails (AAD binds
        // tenant ‖ cf ‖ storage-key): copy the untouched IN row over the OUT row.
        let in_raw = s.kv.kv_get(cf::EDGES_IN, &MemoryDagStore::<MemoryNode>::in_key(&e)).unwrap().unwrap();
        s.kv.kv_put(cf::EDGES_OUT, &e.key(), &in_raw).unwrap();
        assert!(matches!(s.out_edges(&a.compute_id()).unwrap_err(), StoreError::Crypto(_)));
    }

    #[test]
    fn encrypted_store_reads_plaintext_edges_and_meta() {
        // Pre-WP-6 rows (sealed nodes, plaintext edges/meta) stay readable
        // before the migration runs.
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "pa"), tenant_node("r1", "pb"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        let e = edge(&a, &b, EdgeKind::References);
        let plain = serde_json::to_vec(&e).unwrap();
        s.kv.kv_put(cf::EDGES_OUT, &e.key(), &plain).unwrap();
        s.kv.kv_put(cf::EDGES_IN, &MemoryDagStore::<MemoryNode>::in_key(&e), &plain).unwrap();
        s.kv.kv_put(cf::META, b"derived_watermark:r1", b"legacy watermark").unwrap();

        assert_eq!(s.out_edges(&a.compute_id()).unwrap(), vec![e]);
        assert_eq!(s.get_meta(b"derived_watermark:r1").unwrap().unwrap(), b"legacy watermark");
    }

    #[test]
    fn migration_seals_plaintext_edges_and_meta_in_place() {
        // A pre-WP-6 store: nodes sealed, edges + meta plaintext.
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "ma"), tenant_node("r1", "mb"));
        let c = tenant_node("r2", "mc other tenant");
        for n in [&a, &b, &c] {
            s.put_node(n).unwrap();
        }
        let e1 = edge(&a, &b, EdgeKind::Implements);
        let e2 = edge(&b, &c, EdgeKind::References); // owned by r1 (from-endpoint)
        for e in [&e1, &e2] {
            let plain = serde_json::to_vec(e).unwrap();
            s.kv.kv_put(cf::EDGES_OUT, &e.key(), &plain).unwrap();
            s.kv.kv_put(cf::EDGES_IN, &MemoryDagStore::<MemoryNode>::in_key(e), &plain).unwrap();
        }
        s.kv.kv_put(cf::META, b"derived_watermark:r1", b"WM-SECRET").unwrap();
        s.kv.kv_put(cf::META, b"anchor:r2", b"ANCHOR-SECRET").unwrap();
        // A dangling edge (both endpoints absent) cannot be protected: counted.
        let orphan = edge(&tenant_node("rx", "gone1"), &tenant_node("rx", "gone2"), EdgeKind::References);
        s.kv.kv_put(cf::EDGES_OUT, &orphan.key(), &serde_json::to_vec(&orphan).unwrap()).unwrap();

        let report = s.migrate_seal_v2().unwrap();
        assert!(!report.already_done);
        assert_eq!(report.edges_sealed, 4, "2 edges x 2 adjacency rows");
        assert_eq!(report.meta_sealed, 2);
        assert_eq!(report.skipped_unknown_tenant, 1);

        // Ciphertext probe post-migration + full round-trip.
        for e in [&e1, &e2] {
            let raw = s.kv.kv_get(cf::EDGES_OUT, &e.key()).unwrap().unwrap();
            assert!(shred::parse_envelope(&raw).is_some());
        }
        let raw = s.kv.kv_get(cf::META, b"derived_watermark:r1").unwrap().unwrap();
        assert!(shred::parse_envelope(&raw).is_some());
        assert!(!String::from_utf8_lossy(&raw).contains("WM-SECRET"));
        assert_eq!(s.out_edges(&a.compute_id()).unwrap(), vec![e1]);
        assert_eq!(s.out_edges(&b.compute_id()).unwrap(), vec![e2.clone()]);
        assert_eq!(s.get_meta(b"derived_watermark:r1").unwrap().unwrap(), b"WM-SECRET");
        assert_eq!(s.get_meta(b"anchor:r2").unwrap().unwrap(), b"ANCHOR-SECRET");

        // Marker-gated: the second run is a no-op point read.
        let again = s.migrate_seal_v2().unwrap();
        assert!(again.already_done);
        assert_eq!(again.edges_sealed, 0);

        // And the sealed edge now dies with its tenant: shred r1 forgets e1+e2.
        s.shred_tenant("r1").unwrap();
        assert!(s.out_edges(&a.compute_id()).unwrap().is_empty());
        assert!(s.out_edges(&b.compute_id()).unwrap().is_empty());
        assert_eq!(s.get_meta(b"derived_watermark:r1").unwrap(), None);
        assert_eq!(s.get_meta(b"anchor:r2").unwrap().unwrap(), b"ANCHOR-SECRET", "r2 unaffected");
    }

    #[test]
    fn cross_tenant_edge_is_owned_by_its_from_endpoint() {
        let s = enc_store();
        let (a, b) = (tenant_node("r1", "anchor"), tenant_node("r2", "analog"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        let e = edge(&a, &b, EdgeKind::AnalogousTo);
        s.add_edge(&e).unwrap();
        let raw = s.kv.kv_get(cf::EDGES_OUT, &e.key()).unwrap().unwrap();
        assert_eq!(shred::parse_envelope(&raw).unwrap().tenant, "r1");

        // Shredding the ASSERTING tenant forgets the relationship — from both
        // adjacency views — while the r2 endpoint node itself survives.
        s.shred_tenant("r1").unwrap();
        assert!(s.out_edges(&a.compute_id()).unwrap().is_empty());
        assert!(s.in_edges(&b.compute_id()).unwrap().is_empty());
        assert_eq!(s.get_node(&b.compute_id()).unwrap(), Some(b));
    }

    #[test]
    fn edge_scan_overhead_is_bounded() {
        // Coarse hot-path guard (recall/neighbors ride out_edges/in_edges):
        // sealing edge values must not blow scans up by an order of magnitude.
        // Bound is deliberately loose to stay CI-safe; the printed ratio is the
        // honest number.
        const N: usize = 150;
        const PASSES: usize = 20;
        let build = |s: &MemoryDagStore<MemoryNode>| {
            let nodes: Vec<MemoryNode> = (0..N).map(|i| tenant_node("r1", &format!("n{i}"))).collect();
            let edges: Vec<Edge> = nodes.windows(2).map(|w| edge(&w[0], &w[1], EdgeKind::TemporalNext)).collect();
            s.commit(&nodes, &edges).unwrap();
            nodes.iter().map(|n| n.compute_id()).collect::<Vec<_>>()
        };
        let time_scans = |s: &MemoryDagStore<MemoryNode>, ids: &[ContentHash]| {
            let start = std::time::Instant::now();
            let mut total = 0usize;
            for _ in 0..PASSES {
                for id in ids {
                    total += s.out_edges(id).unwrap().len() + s.in_edges(id).unwrap().len();
                }
            }
            assert_eq!(total, PASSES * (N - 1) * 2);
            start.elapsed()
        };

        let plain = store();
        let ids = build(&plain);
        let enc = enc_store();
        build(&enc);
        let (t_plain, t_enc) = (time_scans(&plain, &ids), time_scans(&enc, &ids));
        eprintln!(
            "edge-scan overhead: plaintext {t_plain:?}, sealed {t_enc:?} ({:.1}x)",
            t_enc.as_secs_f64() / t_plain.as_secs_f64().max(f64::EPSILON)
        );
        assert!(
            t_enc < t_plain * 25 + std::time::Duration::from_millis(500),
            "sealed edge scans blew the coarse overhead bound: plaintext {t_plain:?} vs sealed {t_enc:?}"
        );
    }

    #[test]
    fn shred_forgets_one_tenant_and_rekeys_cleanly() {
        let s = enc_store();
        let n1 = tenant_node("r1", "tenant one memory");
        let n2 = tenant_node("r2", "tenant two memory");
        let (id1, id2) = (s.put_node(&n1).unwrap(), s.put_node(&n2).unwrap());

        assert!(s.shred_tenant("r1").unwrap(), "live key existed");
        // r1 is forgotten: point reads and scans, no errors.
        assert_eq!(s.get_node(&id1).unwrap(), None);
        assert_eq!(s.all_nodes().unwrap(), vec![n2.clone()]);
        // The ciphertext row itself remains (it may keep federating).
        assert_eq!(s.node_count().unwrap(), 2);
        // r2 unaffected.
        assert_eq!(s.get_node(&id2).unwrap(), Some(n2));
        // Idempotent: shredding again reports no live key.
        assert!(!s.shred_tenant("r1").unwrap());
        // Unknown tenant: no key to destroy.
        assert!(!s.shred_tenant("never-written").unwrap());

        // r1 writes again: fresh key, new data readable — old data still lost.
        let n1b = tenant_node("r1", "post-shred memory");
        let id1b = s.put_node(&n1b).unwrap();
        assert_eq!(s.get_node(&id1b).unwrap(), Some(n1b), "new generation readable");
        assert_eq!(s.get_node(&id1).unwrap(), None, "pre-shred generation stays forgotten");
    }

    #[test]
    fn tampered_ciphertext_is_a_hard_error_not_a_skip() {
        let s = enc_store();
        let id = s.put_node(&tenant_node("r1", "integrity matters")).unwrap();
        let mut raw = s.kv.kv_get(cf::NODES, id.as_bytes()).unwrap().unwrap();
        // Flip one hex char of the ciphertext field.
        let pos = String::from_utf8_lossy(&raw).find("\"ct\":\"").unwrap() + 7;
        raw[pos] = if raw[pos] == b'0' { b'1' } else { b'0' };
        s.kv.kv_put(cf::NODES, id.as_bytes(), &raw).unwrap();
        assert!(
            matches!(s.get_node(&id).unwrap_err(), StoreError::Crypto(_)),
            "tampering must error, never read as absent"
        );
    }

    #[test]
    fn encrypted_store_reads_plaintext_values() {
        // A pre-WP-1.6 (or unencrypted-mode) value must stay readable when the
        // store is later opened in encrypted mode.
        let s = enc_store();
        let n = tenant_node("r1", "legacy plaintext node");
        let plain = serde_json::to_vec(&n).unwrap();
        s.kv.kv_put(cf::NODES, n.compute_id().as_bytes(), &plain).unwrap();
        assert_eq!(s.get_node(&n.compute_id()).unwrap(), Some(n));
    }

    #[test]
    fn auto_mode_follows_keyring() {
        let s = MemoryDagStore::<MemoryNode>::new_auto(Box::new(InMemoryKv::new())).unwrap();
        assert!(!s.is_encrypted_at_rest(), "no keyring → plaintext mode");

        // A kv that an encrypted store has written to carries keyring rows.
        let kv = InMemoryKv::new();
        kv.kv_put(cf::KEYS, b"r1", br#"{"gen":1,"key":null}"#).unwrap();
        let s = MemoryDagStore::<MemoryNode>::new_auto(Box::new(kv)).unwrap();
        assert!(s.is_encrypted_at_rest(), "keyring entries → encrypted writes");
    }

    #[test]
    fn supersession_on_encrypted_store_stays_sealed() {
        let s = enc_store();
        let (new, old) = (tenant_node("r1", "new"), tenant_node("r1", "old"));
        s.put_node(&new).unwrap();
        s.put_node(&old).unwrap();
        s.apply_supersession(&supersedes_at(&new, &old, 9)).unwrap();

        let old_now = s.get_node(&old.compute_id()).unwrap().unwrap();
        assert_eq!(old_now.status, Status::Superseded);
        let raw = s.kv.kv_get(cf::NODES, old.compute_id().as_bytes()).unwrap().unwrap();
        assert!(shred::parse_envelope(&raw).is_some(), "transitioned node re-sealed, not leaked");
        // WP-6: the supersedes edge itself is sealed too, in both adjacency CFs.
        let e = &s.out_edges(&new.compute_id()).unwrap()[0];
        for (cf_name, key) in [(cf::EDGES_OUT, e.key()), (cf::EDGES_IN, MemoryDagStore::<MemoryNode>::in_key(e))] {
            let raw = s.kv.kv_get(cf_name, &key).unwrap().unwrap();
            assert!(shred::parse_envelope(&raw).is_some(), "{cf_name} supersedes edge sealed");
        }
    }

    #[test]
    fn proposed_supersession_is_inert_until_confirmed() {
        let s = store();
        let (new, old) = (node("new view"), node("old view"));
        s.put_node(&new).unwrap();
        s.put_node(&old).unwrap();

        // A quarantined proposal can be STORED as a plain edge but must not
        // transition anyone (apply_supersession rejects it; WP-1.4 guard).
        let mut proposal = supersedes_at(&new, &old, 5);
        proposal.quarantined = true;
        s.add_edge(&proposal).unwrap();
        assert_eq!(s.get_node(&old.compute_id()).unwrap().unwrap().status, Status::Active);

        // Confirming flips it load-bearing AND applies the supersession.
        let outcome = s.confirm_edge(&new.compute_id(), &old.compute_id(), EdgeKind::Supersedes).unwrap();
        assert_eq!(outcome, ConfirmOutcome::Confirmed);
        let old_now = s.get_node(&old.compute_id()).unwrap().unwrap();
        assert_eq!(old_now.status, Status::Superseded);
        assert_eq!(old_now.valid_to, Some(5));
        let stored = &s.out_edges(&new.compute_id()).unwrap()[0];
        assert!(!stored.quarantined, "edge is load-bearing after confirm");

        // Idempotent re-confirm; unknown edge reads NotFound.
        assert_eq!(
            s.confirm_edge(&new.compute_id(), &old.compute_id(), EdgeKind::Supersedes).unwrap(),
            ConfirmOutcome::AlreadyConfirmed
        );
        assert_eq!(
            s.confirm_edge(&old.compute_id(), &new.compute_id(), EdgeKind::References).unwrap(),
            ConfirmOutcome::NotFound
        );
    }

    #[test]
    fn confirming_a_cyclic_supersession_proposal_is_rejected() {
        let s = store();
        let (a, b) = (node("a"), node("b"));
        s.put_node(&a).unwrap();
        s.put_node(&b).unwrap();
        s.apply_supersession(&supersedes_at(&a, &b, 1)).unwrap();

        // Propose the cycle-closing edge; storage is fine (quarantined), but
        // confirmation hits the Acyclic guard and the proposal stays inert.
        let mut proposal = supersedes_at(&b, &a, 2);
        proposal.quarantined = true;
        s.add_edge(&proposal).unwrap();
        let err = s.confirm_edge(&b.compute_id(), &a.compute_id(), EdgeKind::Supersedes).unwrap_err();
        assert!(matches!(err, SupersessionError::Cycle));
        assert!(s.out_edges(&b.compute_id()).unwrap()[0].quarantined, "proposal stays quarantined");
        assert_eq!(s.get_node(&a.compute_id()).unwrap().unwrap().status, Status::Active);
    }

    #[test]
    fn apply_supersession_rejects_bad_inputs() {
        let s = store();
        let (a, b) = (node("a"), node("b"));
        s.put_node(&a).unwrap();

        // wrong kind
        let e = edge(&a, &b, EdgeKind::Implements);
        assert!(matches!(s.apply_supersession(&e).unwrap_err(), SupersessionError::NotSupersedes(_)));
        // self-supersession
        let e = supersedes_at(&a, &a, 1);
        assert!(matches!(s.apply_supersession(&e).unwrap_err(), SupersessionError::SelfSupersede));
        // missing target (b never stored)
        let e = supersedes_at(&a, &b, 1);
        assert!(matches!(s.apply_supersession(&e).unwrap_err(), SupersessionError::MissingNode(_)));
        // quarantined proposals don't change anyone's status
        s.put_node(&b).unwrap();
        let mut e = supersedes_at(&a, &b, 1);
        e.quarantined = true;
        assert!(matches!(s.apply_supersession(&e).unwrap_err(), SupersessionError::Quarantined));
        assert_eq!(s.get_node(&b.compute_id()).unwrap().unwrap().status, Status::Active);
    }
}
