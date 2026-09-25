//! Signed capability grants (WP-2.2).
//!
//! v1 local model that mirrors `citrate-agent-runtime`'s `CapabilityGrant` and adds
//! the verified-needed extensions: per-resource read/write scopes and a delegation
//! chain. v2 imports the real (extended) type from agent-runtime. Identity binding
//! to a SIWE/OIDC wallet (citrate-identity) is a v2 integration — here the grant is
//! the capability and its ed25519 signature is the trust root.

use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Read,
    Write,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthzError {
    #[error("grant revoked")]
    Revoked,
    #[error("grant expired")]
    Expired,
    #[error("grant unsigned")]
    Unsigned,
    #[error("grant signature invalid")]
    BadSignature,
    #[error("resource not in grant scope: {0}")]
    ResourceDenied(String),
    #[error("operation {0:?} not permitted on resource")]
    OperationDenied(Op),
}

/// A single logical resource the grant covers, e.g. `repo:citrate-chain/memory`.
/// `*` matches any resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceScope {
    pub resource_id: String,
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyProfile {
    ReadOnly,
    Guided,
    Operator,
    Maintainer,
}

/// One hop in a delegation chain (human → supervisor → worker). Minimal in v1:
/// records who delegated and when, so a revocation cascade can be reconstructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationStep {
    pub delegator: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub id: String,
    pub issuer: String,
    pub recipient: String,
    pub allowed_resources: Vec<ResourceScope>,
    pub policy: PolicyProfile,
    pub expires_at_ms: u64,
    pub revoked: bool,
    pub delegation_chain: Vec<DelegationStep>,
    pub issuer_pubkey: Vec<u8>, // 32-byte ed25519 verifying key
    pub signature: Vec<u8>,     // 64-byte ed25519 signature over signing_preimage
}

fn feed(buf: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
    buf.push(tag);
    buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn resource_matches(scope: &str, requested: &str) -> bool {
    scope == "*" || scope == requested
}

impl CapabilityGrant {
    /// Domain-separated, length-prefixed encoding of the identity-bearing fields.
    /// Excludes `signature` (derived); includes `issuer_pubkey` so the signing key
    /// is bound to the grant.
    ///
    /// PBA-L6b-019 follow-up: `revoked` is covered WHEN SET (tag 11), so a grant
    /// the issuer signed as revoked cannot be un-revoked by the presenter flipping
    /// the flag back. A never-revoked grant's preimage is unchanged, so every
    /// existing signature still verifies. (Revoking an already-issued grant still
    /// needs a server-side deny list keyed by grant id — the holder keeps the old
    /// copy — see `mem-sync::transport::PeerAuth::revoked_ids`.)
    pub fn signing_preimage(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        feed(&mut buf, 0, b"mem-authz:grant:v1");
        feed(&mut buf, 1, self.id.as_bytes());
        feed(&mut buf, 2, self.issuer.as_bytes());
        feed(&mut buf, 3, self.recipient.as_bytes());
        for r in &self.allowed_resources {
            feed(&mut buf, 4, r.resource_id.as_bytes());
            feed(&mut buf, 5, &[r.can_read as u8, r.can_write as u8]);
        }
        feed(&mut buf, 6, format!("{:?}", self.policy).as_bytes());
        feed(&mut buf, 7, &self.expires_at_ms.to_le_bytes());
        for d in &self.delegation_chain {
            feed(&mut buf, 8, d.delegator.as_bytes());
            feed(&mut buf, 9, &d.at_ms.to_le_bytes());
        }
        feed(&mut buf, 10, &self.issuer_pubkey);
        if self.revoked {
            feed(&mut buf, 11, b"revoked");
        }
        buf
    }

    /// Sign the grant in place with `sk` (sets `issuer_pubkey` + `signature`).
    pub fn sign_with(&mut self, sk: &SigningKey) {
        use ed25519_dalek::Signer;
        self.issuer_pubkey = sk.verifying_key().to_bytes().to_vec();
        let sig = sk.sign(&self.signing_preimage());
        self.signature = sig.to_bytes().to_vec();
    }

    pub fn verify_signature(&self) -> Result<(), AuthzError> {
        if self.signature.is_empty() || self.issuer_pubkey.is_empty() {
            return Err(AuthzError::Unsigned);
        }
        let pk: [u8; 32] = self.issuer_pubkey.as_slice().try_into().map_err(|_| AuthzError::BadSignature)?;
        let sig_bytes: [u8; 64] = self.signature.as_slice().try_into().map_err(|_| AuthzError::BadSignature)?;
        let vk = VerifyingKey::from_bytes(&pk).map_err(|_| AuthzError::BadSignature)?;
        let sig = Signature::from_bytes(&sig_bytes);
        vk.verify_strict(&self.signing_preimage(), &sig).map_err(|_| AuthzError::BadSignature)
    }

    /// Authorize `op` on `resource_id` at `now_ms`. The full gate: revocation,
    /// expiry, signature, resource scope, operation.
    pub fn check(&self, resource_id: &str, op: Op, now_ms: u64) -> Result<(), AuthzError> {
        if self.revoked {
            return Err(AuthzError::Revoked);
        }
        if now_ms >= self.expires_at_ms {
            return Err(AuthzError::Expired);
        }
        self.verify_signature()?;
        let scope = self
            .allowed_resources
            .iter()
            .find(|r| resource_matches(&r.resource_id, resource_id))
            .ok_or_else(|| AuthzError::ResourceDenied(resource_id.to_string()))?;
        let allowed = match op {
            Op::Read => scope.can_read,
            Op::Write => scope.can_write,
        };
        if allowed {
            Ok(())
        } else {
            Err(AuthzError::OperationDenied(op))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn grant() -> CapabilityGrant {
        let mut g = CapabilityGrant {
            id: "g1".into(),
            issuer: "did:human".into(),
            recipient: "agent:worker".into(),
            allowed_resources: vec![ResourceScope {
                resource_id: "repo:citrate-chain/memory".into(),
                can_read: true,
                can_write: false,
            }],
            policy: PolicyProfile::ReadOnly,
            expires_at_ms: 10_000,
            revoked: false,
            delegation_chain: vec![],
            issuer_pubkey: vec![],
            signature: vec![],
        };
        g.sign_with(&key());
        g
    }

    #[test]
    fn signed_grant_allows_scoped_read() {
        let g = grant();
        assert_eq!(g.check("repo:citrate-chain/memory", Op::Read, 0), Ok(()));
    }

    #[test]
    fn denies_write_when_read_only() {
        assert_eq!(
            grant().check("repo:citrate-chain/memory", Op::Write, 0),
            Err(AuthzError::OperationDenied(Op::Write))
        );
    }

    #[test]
    fn denies_unscoped_resource() {
        assert!(matches!(
            grant().check("repo:citrate-identity/memory", Op::Read, 0),
            Err(AuthzError::ResourceDenied(_))
        ));
    }

    #[test]
    fn expiry_and_revocation_block() {
        assert_eq!(grant().check("repo:citrate-chain/memory", Op::Read, 10_000), Err(AuthzError::Expired));
        let mut g = grant();
        g.revoked = true;
        assert_eq!(g.check("repo:citrate-chain/memory", Op::Read, 0), Err(AuthzError::Revoked));
    }

    #[test]
    fn tampering_invalidates_signature() {
        let mut g = grant();
        // widen scope after signing → preimage changes → signature fails
        g.allowed_resources[0].can_write = true;
        assert_eq!(g.check("repo:citrate-chain/memory", Op::Write, 0), Err(AuthzError::BadSignature));
    }

    #[test]
    fn wildcard_scope_matches_any_repo() {
        let mut g = CapabilityGrant {
            allowed_resources: vec![ResourceScope { resource_id: "*".into(), can_read: true, can_write: false }],
            ..grant()
        };
        g.sign_with(&key());
        assert_eq!(g.check("repo:anything/memory", Op::Read, 0), Ok(()));
    }
}
