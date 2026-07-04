//! Per-tenant crypto-shredding at rest (WP-1.6, locked decision #10).
//!
//! Every tenant (repo) gets its own random 256-bit key; node payloads are
//! sealed with XChaCha20-Poly1305 before they touch the backend. **Forgetting a
//! tenant = destroying its key**: the ciphertext stays (it can keep federating
//! — that's the point of the design), but nothing can ever read it again.
//!
//! Construction notes:
//! - **Deterministic SIV-style nonce**: `nonce = keyed-blake3(nonce_key,
//!   node_id ‖ plaintext)[..24]`. The same (key, id, plaintext) always seals to
//!   the same bytes — Derived-plane rebuilds stay byte-identical — while any
//!   plaintext change (status transitions overwrite in place) derives a fresh
//!   nonce, so the keystream is never reused. No RNG at seal time.
//! - **AAD binds tenant ‖ node id**, so a ciphertext moved to another node's
//!   key (or another tenant) fails authentication instead of decrypting.
//! - **Key separation**: the stored tenant key is a master from which the
//!   cipher key and nonce key are independently derived (`blake3::derive_key`).
//! - Tenant keys come from OS randomness (`getrandom`) — they must not be
//!   derivable from anything, or "destroy the key" would not destroy access.
//!
//! Scope (v1 = WP-1.6, extended by ENCRYPT-S1 WP-6):
//! - **Node payloads** are sealed with AAD `tenant ‖ 0 ‖ node-id`
//!   ([`seal`]/[`open`] — the original v1 layout, kept byte-identical so
//!   pre-WP-6 stores stay readable).
//! - **Edge VALUES and operational-meta VALUES** (freshness watermark, sync
//!   anchors) are sealed with AAD `tenant ‖ 0 ‖ cf ‖ 0 ‖ storage-key`
//!   ([`seal_at`]/[`open_at`]), under the same per-tenant key material as the
//!   tenant's nodes — so crypto-shredding a tenant forgets its relationships
//!   and freshness state along with its nodes. An edge belongs to the tenant
//!   of its `from` endpoint (falling back to `to` for resilience).
//!
//! **What deliberately stays plaintext — and why** (WP-6 residual, accepted):
//! - **Edge storage KEYS** (`from ‖ to ‖ kind` and `to ‖ from ‖ kind`). Every
//!   edge read in the system is a prefix scan by raw node id
//!   (`out_edges`/`in_edges` feeding neighbors, verify, critique, BFS
//!   reachability and the merge/anchor paths), issued by readers that know a
//!   node id but *not* which tenant owns it — and after a shred there is no
//!   key left to compute a keyed-hash prefix with. Per-tenant keyed-hash key
//!   prefixes would therefore break every query path (and shredded edges
//!   could never be matched for scanning again). Consequence: graph
//!   **topology is visible** to a disk-level attacker — which 32-byte ids
//!   connect to which, and the edge *kind* tag — but the relationship
//!   **content** (provenance, asserter, evidence, confidence, quarantine
//!   state, signature) is sealed. Node ids are opaque content hashes, so the
//!   linkage leak is between pseudonymous ids unless the attacker also holds
//!   a live key for an endpoint.
//! - **Meta storage KEYS** (`derived_watermark:{tenant}` etc.) and the
//!   `tenant` label inside every envelope: point lookups need the key, and
//!   the reader must know which tenant's key to fetch. Tenant *names* are
//!   already visible in the keyring CF by construction.
//!
//! The keyring lives in its own column family next to the data so the
//! mechanism is self-contained; moving key custody out of the store
//! (citrate-identity / operator HSM) is the v2 federation step.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use mem_core::ContentHash;

use crate::StoreError;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;

const CIPHER_CONTEXT: &str = "citrate-memories mem-store shred cipher v1";
const NONCE_CONTEXT: &str = "citrate-memories mem-store shred nonce v1";

/// The at-rest envelope a sealed node is stored as. Detected by shape: a plain
/// node's JSON has no `enc`/`ct` fields, so [`parse_envelope`] cleanly
/// distinguishes the two and plaintext stores stay readable.
#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// Envelope version (1).
    pub enc: u8,
    /// Tenant whose key seals this payload (plaintext by design: the reader
    /// must know which key to fetch — and which key destruction forgot it).
    pub tenant: String,
    /// Key generation that sealed this payload. A shred destroys a generation's
    /// key material; a tenant that writes again mints the next generation — so
    /// pre-shred envelopes read as *forgotten* (generation mismatch), never as
    /// a spurious authentication failure under the new key.
    pub kgen: u32,
    /// Hex-encoded 24-byte XChaCha20 nonce.
    pub nonce: String,
    /// Hex-encoded ciphertext + Poly1305 tag.
    pub ct: String,
}

/// A tenant's keyring row. `key: None` is the shredded state: the generation
/// counter survives (so the next key gets a fresh generation) but the key
/// material is destroyed.
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyringEntry {
    pub gen: u32,
    /// Hex-encoded 32-byte master key; `None` after a shred.
    pub key: Option<String>,
}

impl KeyringEntry {
    pub fn key_bytes(&self) -> Result<Option<[u8; KEY_LEN]>, StoreError> {
        let Some(k) = &self.key else { return Ok(None) };
        let bytes = hex::decode(k).map_err(|e| StoreError::Crypto(format!("keyring hex: {e}")))?;
        let key: [u8; KEY_LEN] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| StoreError::Crypto("keyring entry has wrong length".into()))?;
        Ok(Some(key))
    }
}

/// Parse `bytes` as an [`Envelope`] if that's what they are. `None` means the
/// value is a plaintext (pre-WP-1.6 or unencrypted-store) node.
pub fn parse_envelope(bytes: &[u8]) -> Option<Envelope> {
    let env: Envelope = serde_json::from_slice(bytes).ok()?;
    (env.enc == 1).then_some(env)
}

/// Generate a fresh random tenant key.
pub fn generate_key() -> Result<[u8; KEY_LEN], StoreError> {
    let mut key = [0u8; KEY_LEN];
    getrandom::getrandom(&mut key).map_err(|e| StoreError::Crypto(format!("os rng: {e}")))?;
    Ok(key)
}

/// v1 node AAD: `tenant ‖ 0 ‖ node-id`. Kept byte-identical to WP-1.6 so
/// every already-sealed node stays readable.
fn aad(tenant: &str, id: &ContentHash) -> Vec<u8> {
    let mut a = Vec::with_capacity(tenant.len() + 1 + 32);
    a.extend_from_slice(tenant.as_bytes());
    a.push(0);
    a.extend_from_slice(id.as_bytes());
    a
}

/// WP-6 edge/meta AAD: `tenant ‖ 0 ‖ cf ‖ 0 ‖ storage-key` — binds the value
/// to its exact slot, so a ciphertext moved to another row, another column
/// family, or another tenant fails authentication instead of decrypting.
fn aad_at(tenant: &str, cf: &str, key: &[u8]) -> Vec<u8> {
    let mut a = Vec::with_capacity(tenant.len() + 1 + cf.len() + 1 + key.len());
    a.extend_from_slice(tenant.as_bytes());
    a.push(0);
    a.extend_from_slice(cf.as_bytes());
    a.push(0);
    a.extend_from_slice(key);
    a
}

/// SIV-style deterministic nonce over `parts ‖ plaintext` (see module docs):
/// same slot + same plaintext → same bytes; any change → fresh nonce.
fn derive_nonce_parts(master: &[u8; KEY_LEN], parts: &[&[u8]], plaintext: &[u8]) -> [u8; NONCE_LEN] {
    let nonce_key = blake3::derive_key(NONCE_CONTEXT, master);
    let mut h = blake3::Hasher::new_keyed(&nonce_key);
    for p in parts {
        h.update(p);
    }
    h.update(plaintext);
    let mut nonce = [0u8; NONCE_LEN];
    h.finalize_xof().fill(&mut nonce);
    nonce
}

fn derive_nonce(master: &[u8; KEY_LEN], id: &ContentHash, plaintext: &[u8]) -> [u8; NONCE_LEN] {
    derive_nonce_parts(master, &[id.as_bytes()], plaintext)
}

fn cipher(master: &[u8; KEY_LEN]) -> XChaCha20Poly1305 {
    let cipher_key = blake3::derive_key(CIPHER_CONTEXT, master);
    XChaCha20Poly1305::new((&cipher_key).into())
}

/// Seal a node's serialized bytes under its tenant's key.
pub fn seal(
    master: &[u8; KEY_LEN],
    tenant: &str,
    kgen: u32,
    id: &ContentHash,
    plaintext: &[u8],
) -> Result<Vec<u8>, StoreError> {
    let nonce = derive_nonce(master, id, plaintext);
    let ct = cipher(master)
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload { msg: plaintext, aad: &aad(tenant, id) },
        )
        .map_err(|_| StoreError::Crypto("seal failed".into()))?;
    let env = Envelope {
        enc: 1,
        tenant: tenant.to_string(),
        kgen,
        nonce: hex::encode(nonce),
        ct: hex::encode(ct),
    };
    serde_json::to_vec(&env).map_err(|e| StoreError::Serde(e.to_string()))
}

/// Open an envelope with its tenant's key. Authentication failure (tampered or
/// swapped ciphertext) is a hard error — never silently empty.
pub fn open(master: &[u8; KEY_LEN], env: &Envelope, id: &ContentHash) -> Result<Vec<u8>, StoreError> {
    open_with_aad(master, env, &aad(&env.tenant, id))
}

/// Seal an edge or operational-meta value under its tenant's key, bound to its
/// exact storage slot (`cf`, `key`) — ENCRYPT-S1 WP-6. Same envelope shape as
/// nodes; the AAD layout differs (see [`aad_at`]).
pub fn seal_at(
    master: &[u8; KEY_LEN],
    tenant: &str,
    kgen: u32,
    cf: &str,
    key: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, StoreError> {
    let nonce = derive_nonce_parts(master, &[cf.as_bytes(), &[0], key], plaintext);
    let ct = cipher(master)
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload { msg: plaintext, aad: &aad_at(tenant, cf, key) },
        )
        .map_err(|_| StoreError::Crypto("seal failed".into()))?;
    let env = Envelope {
        enc: 1,
        tenant: tenant.to_string(),
        kgen,
        nonce: hex::encode(nonce),
        ct: hex::encode(ct),
    };
    serde_json::to_vec(&env).map_err(|e| StoreError::Serde(e.to_string()))
}

/// Open a [`seal_at`] envelope. Same hard-error contract as [`open`].
pub fn open_at(master: &[u8; KEY_LEN], env: &Envelope, cf: &str, key: &[u8]) -> Result<Vec<u8>, StoreError> {
    open_with_aad(master, env, &aad_at(&env.tenant, cf, key))
}

fn open_with_aad(master: &[u8; KEY_LEN], env: &Envelope, aad: &[u8]) -> Result<Vec<u8>, StoreError> {
    let nonce = hex::decode(&env.nonce).map_err(|e| StoreError::Crypto(format!("bad nonce hex: {e}")))?;
    if nonce.len() != NONCE_LEN {
        return Err(StoreError::Crypto(format!("nonce length {} != {NONCE_LEN}", nonce.len())));
    }
    let ct = hex::decode(&env.ct).map_err(|e| StoreError::Crypto(format!("bad ciphertext hex: {e}")))?;
    cipher(master)
        .decrypt(XNonce::from_slice(&nonce), Payload { msg: &ct, aad })
        .map_err(|_| StoreError::Crypto("decryption failed: tampered, swapped, or wrong key".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_core::IdBuilder;

    fn id(s: &str) -> ContentHash {
        IdBuilder::new("test").field_str(1, s).finish()
    }

    #[test]
    fn seal_open_roundtrip_and_determinism() {
        let key = generate_key().unwrap();
        let nid = id("n1");
        let sealed = seal(&key, "repo-a", 1, &nid, b"secret payload").unwrap();
        let env = parse_envelope(&sealed).expect("envelope shape");
        assert_eq!(open(&key, &env, &nid).unwrap(), b"secret payload");
        // Same inputs → same bytes (Derived-plane rebuilds are byte-identical).
        assert_eq!(seal(&key, "repo-a", 1, &nid, b"secret payload").unwrap(), sealed);
        // Different plaintext → different nonce (no keystream reuse).
        let sealed2 = seal(&key, "repo-a", 1, &nid, b"secret payload v2").unwrap();
        let env2 = parse_envelope(&sealed2).unwrap();
        assert_ne!(env.nonce, env2.nonce);
    }

    #[test]
    fn tamper_and_swap_fail_authentication() {
        let key = generate_key().unwrap();
        let (id_a, id_b) = (id("a"), id("b"));
        let sealed = seal(&key, "repo-a", 1, &id_a, b"payload").unwrap();
        let mut env = parse_envelope(&sealed).unwrap();

        // Swapped to another node id → AAD mismatch.
        assert!(open(&key, &env, &id_b).is_err());
        // Swapped to another tenant label → AAD mismatch.
        env.tenant = "repo-b".into();
        assert!(open(&key, &env, &id_a).is_err());
        // Bit-flipped ciphertext → Poly1305 failure.
        let mut env = parse_envelope(&sealed).unwrap();
        let mut ct = hex::decode(&env.ct).unwrap();
        ct[0] ^= 1;
        env.ct = hex::encode(ct);
        assert!(open(&key, &env, &id_a).is_err());
        // Wrong key → failure.
        let other = generate_key().unwrap();
        assert!(open(&other, &parse_envelope(&sealed).unwrap(), &id_a).is_err());
    }

    #[test]
    fn plain_node_json_is_not_an_envelope() {
        assert!(parse_envelope(br#"{"schema_version":1,"repo":"r","content":[1,2]}"#).is_none());
    }

    #[test]
    fn seal_at_roundtrip_determinism_and_slot_binding() {
        let key = generate_key().unwrap();
        let sealed = seal_at(&key, "repo-a", 1, "mem_edges_out", b"edge-key", b"edge payload").unwrap();
        let env = parse_envelope(&sealed).expect("envelope shape");
        assert_eq!(open_at(&key, &env, "mem_edges_out", b"edge-key").unwrap(), b"edge payload");
        // Deterministic: same slot + plaintext → identical bytes (idempotent re-add).
        assert_eq!(seal_at(&key, "repo-a", 1, "mem_edges_out", b"edge-key", b"edge payload").unwrap(), sealed);

        // AAD binds the slot: another key, another cf, or another tenant all fail.
        assert!(open_at(&key, &env, "mem_edges_out", b"other-key").is_err());
        assert!(open_at(&key, &env, "mem_edges_in", b"edge-key").is_err());
        let mut env2 = parse_envelope(&sealed).unwrap();
        env2.tenant = "repo-b".into();
        assert!(open_at(&key, &env2, "mem_edges_out", b"edge-key").is_err());
        // A v1 node envelope cannot be opened as an at-slot envelope and vice versa.
        let nid = id("n1");
        let node_sealed = seal(&key, "repo-a", 1, &nid, b"node payload").unwrap();
        let node_env = parse_envelope(&node_sealed).unwrap();
        assert!(open_at(&key, &node_env, "mem_nodes", nid.as_bytes()).is_err());
        assert!(open(&key, &env, &nid).is_err());
    }

    #[test]
    fn seal_at_different_slots_never_reuse_a_nonce() {
        let key = generate_key().unwrap();
        // Same plaintext in the out- and in-adjacency rows: different nonces.
        let a = parse_envelope(&seal_at(&key, "r", 1, "mem_edges_out", b"k1", b"same").unwrap()).unwrap();
        let b = parse_envelope(&seal_at(&key, "r", 1, "mem_edges_in", b"k2", b"same").unwrap()).unwrap();
        assert_ne!(a.nonce, b.nonce);
        // Same slot, changed plaintext (e.g. de-quarantined edge): fresh nonce.
        let c = parse_envelope(&seal_at(&key, "r", 1, "mem_edges_out", b"k1", b"changed").unwrap()).unwrap();
        assert_ne!(a.nonce, c.nonce);
    }
}
