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
//! v1 scope: node payloads are encrypted; edge records and the freshness
//! watermark are structural metadata (content hashes, repo name, head sha) and
//! stay plaintext. The keyring lives in its own column family next to the data
//! so the mechanism is self-contained; moving key custody out of the store
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

fn aad(tenant: &str, id: &ContentHash) -> Vec<u8> {
    let mut a = Vec::with_capacity(tenant.len() + 1 + 32);
    a.extend_from_slice(tenant.as_bytes());
    a.push(0);
    a.extend_from_slice(id.as_bytes());
    a
}

fn derive_nonce(master: &[u8; KEY_LEN], id: &ContentHash, plaintext: &[u8]) -> [u8; NONCE_LEN] {
    let nonce_key = blake3::derive_key(NONCE_CONTEXT, master);
    let mut h = blake3::Hasher::new_keyed(&nonce_key);
    h.update(id.as_bytes());
    h.update(plaintext);
    let mut nonce = [0u8; NONCE_LEN];
    h.finalize_xof().fill(&mut nonce);
    nonce
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
    let nonce = hex::decode(&env.nonce).map_err(|e| StoreError::Crypto(format!("bad nonce hex: {e}")))?;
    if nonce.len() != NONCE_LEN {
        return Err(StoreError::Crypto(format!("nonce length {} != {NONCE_LEN}", nonce.len())));
    }
    let ct = hex::decode(&env.ct).map_err(|e| StoreError::Crypto(format!("bad ciphertext hex: {e}")))?;
    cipher(master)
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload { msg: &ct, aad: &aad(&env.tenant, id) },
        )
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
}
