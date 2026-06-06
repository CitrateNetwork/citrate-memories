//! Content-addressed identity (WP-0.3).
//!
//! The cardinal rule (`00_OVERVIEW.md` core invariant): a node's id is a function
//! of its *content and structure only* — never of its embedding, timestamps,
//! confidence, signature, or status. This is what makes the Derived plane
//! reproducible on every machine and what keeps edges stable across re-embedding.

use std::fmt;

/// A 32-byte BLAKE3 content hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse a 64-char hex string. Returns `None` on malformed input
    /// (no panics — Rule 8).
    pub fn from_hex(s: &str) -> Option<Self> {
        let bytes = hex::decode(s).ok()?;
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some(ContentHash(arr))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short prefix is enough to recognise a node in logs.
        write!(f, "ContentHash({}…)", &self.to_hex()[..12])
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Deterministic, domain-separated identity builder.
///
/// Every contributing field is fed as `tag ‖ len(LE u64) ‖ bytes`, which makes
/// the encoding unambiguous (no two distinct field sets can collide by
/// concatenation). Mirrors the `signing_preimage` discipline already used by
/// `CapabilityGrant` in citrate-agent-runtime.
pub struct IdBuilder {
    hasher: blake3::Hasher,
}

impl IdBuilder {
    /// Start a builder bound to a domain (e.g. `"memory-node:v1"`), so ids from
    /// different schema versions or record kinds never collide.
    pub fn new(domain: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        Self::feed_raw(&mut hasher, 0x00, domain.as_bytes());
        IdBuilder { hasher }
    }

    fn feed_raw(hasher: &mut blake3::Hasher, tag: u8, bytes: &[u8]) {
        hasher.update(&[tag]);
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }

    /// Feed one identity-contributing field. `tag` distinguishes fields so that
    /// swapping two fields of equal length changes the hash.
    pub fn field(mut self, tag: u8, bytes: &[u8]) -> Self {
        Self::feed_raw(&mut self.hasher, tag, bytes);
        self
    }

    pub fn field_str(self, tag: u8, s: &str) -> Self {
        self.field(tag, s.as_bytes())
    }

    pub fn field_u16(self, tag: u8, v: u16) -> Self {
        self.field(tag, &v.to_le_bytes())
    }

    pub fn finish(self) -> ContentHash {
        ContentHash(*self.hasher.finalize().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let h = IdBuilder::new("test").field_str(1, "hello").finish();
        let s = h.to_hex();
        assert_eq!(ContentHash::from_hex(&s), Some(h));
    }

    #[test]
    fn from_hex_rejects_garbage_without_panic() {
        assert_eq!(ContentHash::from_hex("not-hex"), None);
        assert_eq!(ContentHash::from_hex("ab"), None); // too short
    }

    #[test]
    fn field_order_and_tags_matter() {
        let a = IdBuilder::new("d").field_str(1, "x").field_str(2, "y").finish();
        let b = IdBuilder::new("d").field_str(1, "y").field_str(2, "x").finish();
        let c = IdBuilder::new("d").field_str(2, "x").field_str(1, "y").finish();
        assert_ne!(a, b, "swapping field values must change the id");
        assert_ne!(a, c, "swapping field tags must change the id");
    }

    #[test]
    fn length_prefix_prevents_concat_collision() {
        // ("ab","c") must not collide with ("a","bc") under the same tag.
        let a = IdBuilder::new("d").field_str(1, "ab").field_str(1, "c").finish();
        let b = IdBuilder::new("d").field_str(1, "a").field_str(1, "bc").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn domain_separation() {
        let a = IdBuilder::new("domain-a").field_str(1, "x").finish();
        let b = IdBuilder::new("domain-b").field_str(1, "x").finish();
        assert_ne!(a, b);
    }
}
