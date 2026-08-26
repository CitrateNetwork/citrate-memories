//! Authentication + authorization for the gateway.
//!
//! Two trust roots:
//!   * **OIDC** (production): the webapp BFF forwards the user's id_token; we
//!     independently verify it (iss + aud + exp, RS256) against the authority's
//!     JWKS. This is the only way in once OIDC is configured — the gateway fails
//!     closed on any unverified request.
//!   * **dev-auth** (bootstrap only): `x-dev-sub` header, gated behind
//!     `MEM_GATEWAY_ALLOW_DEV_AUTH=1` and ignored entirely once OIDC is on.
//!
//! Once a `sub` is established and looked up in the control plane, we mint an
//! in-process [`CapabilityGrant`] from the membership role and run every
//! operation through the same `grant.check()` the MCP server uses — so the HTTP
//! and MCP surfaces share one authorization primitive.

use ed25519_dalek::SigningKey;
use mem_authz::{CapabilityGrant, PolicyProfile, ResourceScope};

use crate::control::{Membership, Role};

/// Derive the gateway's stable ed25519 key from `MEM_GATEWAY_ASSERTER_SEED`
/// (blake3 → 32 bytes). Used both to sign minted grants and as the write
/// asserter; keeping it stable keeps signatures verifiable across restarts.
pub fn signing_key_from_seed(seed: &str) -> SigningKey {
    let digest = blake3::hash(seed.as_bytes());
    SigningKey::from_bytes(digest.as_bytes())
}

fn policy_for(role: Role) -> PolicyProfile {
    match role {
        Role::OrgOwner => PolicyProfile::Maintainer,
        Role::Admin => PolicyProfile::Operator,
        Role::Member => PolicyProfile::Guided,
        Role::ReadOnly => PolicyProfile::ReadOnly,
    }
}

/// Normalize a control-plane scope id to the resource id the routes check.
/// `"*"` stays a wildcard; an already-qualified `"repo:.../memory"` passes
/// through; a bare tenant name (`"citrate-chain"`) is expanded — so humans can
/// author `control.json` with plain repo names.
fn normalize_resource(id: &str) -> String {
    if id == "*" || id.starts_with("repo:") {
        id.to_string()
    } else {
        repo_resource(id)
    }
}

fn resources_for(m: &Membership) -> Vec<ResourceScope> {
    match m.role {
        // Owner/Admin implicitly hold the whole Org.
        Role::OrgOwner | Role::Admin => vec![ResourceScope {
            resource_id: "*".into(),
            can_read: true,
            can_write: true,
        }],
        Role::Member => m
            .scopes
            .iter()
            .map(|s| ResourceScope {
                resource_id: normalize_resource(&s.resource_id),
                can_read: s.can_read,
                can_write: s.can_write,
            })
            .collect(),
        // ReadOnly: same scopes, write forced off.
        Role::ReadOnly => m
            .scopes
            .iter()
            .map(|s| ResourceScope {
                resource_id: normalize_resource(&s.resource_id),
                can_read: s.can_read,
                can_write: false,
            })
            .collect(),
    }
}

/// Mint a signed, short-lived in-process grant for a verified principal. The
/// signature uses the gateway key so the grant is well-formed (verifiable) even
/// though it never leaves the process.
pub fn mint_grant(
    sk: &SigningKey,
    issuer: &str,
    m: &Membership,
    now_ms: u64,
    ttl_ms: u64,
) -> CapabilityGrant {
    let mut grant = CapabilityGrant {
        id: format!("gw:{}:{}", m.org, now_ms),
        issuer: issuer.to_string(),
        recipient: m.sub.clone(),
        allowed_resources: resources_for(m),
        policy: policy_for(m.role),
        expires_at_ms: now_ms.saturating_add(ttl_ms),
        revoked: false,
        delegation_chain: Vec::new(),
        issuer_pubkey: Vec::new(),
        signature: Vec::new(),
    };
    grant.sign_with(sk);
    grant
}

/// Resource id convention shared with the MCP surface: a repo tenant maps to
/// `repo:<tenant>/memory`; Org-wide operations check `*`.
pub fn repo_resource(tenant: &str) -> String {
    format!("repo:{tenant}/memory")
}

// ---------------------------------------------------------------------------
// OIDC + connect-token verification (needs jsonwebtoken → server feature only).
// ---------------------------------------------------------------------------

#[cfg(feature = "server")]
mod verify {
    use std::collections::HashMap;

    use jsonwebtoken::{
        decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
    };
    use serde::{Deserialize, Serialize};
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum AuthError {
        #[error("jwks parse: {0}")]
        Jwks(String),
        #[error("no usable RSA verifying key in JWKS")]
        NoKeys,
        #[error("token rejected: {0}")]
        Token(String),
        #[error("no key matches token kid")]
        NoMatchingKey,
    }

    #[derive(Debug, Deserialize)]
    struct Claims {
        sub: String,
    }

    #[derive(Debug, Deserialize)]
    struct Jwk {
        kty: String,
        #[serde(default)]
        kid: Option<String>,
        #[serde(default)]
        n: Option<String>,
        #[serde(default)]
        e: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct JwkSet {
        keys: Vec<Jwk>,
    }

    /// Verifies RS256 id_tokens against a fixed JWKS (no startup network fetch by
    /// design — rotate by redeploying the file).
    pub struct OidcVerifier {
        issuer: String,
        audience: String,
        by_kid: HashMap<String, DecodingKey>,
        // Keys with no `kid` — tried when a token carries no/unknown kid.
        anonymous: Vec<DecodingKey>,
    }

    impl OidcVerifier {
        pub fn from_jwks(issuer: &str, audience: &str, jwks_json: &str) -> Result<Self, AuthError> {
            let set: JwkSet =
                serde_json::from_str(jwks_json).map_err(|e| AuthError::Jwks(e.to_string()))?;
            let mut by_kid = HashMap::new();
            let mut anonymous = Vec::new();
            for k in set.keys {
                if k.kty != "RSA" {
                    continue;
                }
                let (n, e) = match (k.n.as_deref(), k.e.as_deref()) {
                    (Some(n), Some(e)) => (n, e),
                    _ => continue,
                };
                let key = DecodingKey::from_rsa_components(n, e)
                    .map_err(|e| AuthError::Jwks(e.to_string()))?;
                match k.kid {
                    Some(kid) => {
                        by_kid.insert(kid, key);
                    }
                    None => anonymous.push(key),
                }
            }
            if by_kid.is_empty() && anonymous.is_empty() {
                return Err(AuthError::NoKeys);
            }
            Ok(OidcVerifier {
                issuer: issuer.to_string(),
                audience: audience.to_string(),
                by_kid,
                anonymous,
            })
        }

        fn validation(&self) -> Validation {
            let mut v = Validation::new(Algorithm::RS256);
            v.set_issuer(&[self.issuer.as_str()]);
            v.set_audience(&[self.audience.as_str()]);
            v.validate_exp = true;
            v
        }

        /// Verify a bearer token, returning the `sub` on success.
        pub fn verify(&self, token: &str) -> Result<String, AuthError> {
            let header = decode_header(token).map_err(|e| AuthError::Token(e.to_string()))?;
            let validation = self.validation();

            // Prefer the key whose kid matches; otherwise try anonymous keys.
            let candidates: Vec<&DecodingKey> = match header.kid.as_deref() {
                Some(kid) => match self.by_kid.get(kid) {
                    Some(k) => vec![k],
                    None if !self.anonymous.is_empty() => self.anonymous.iter().collect(),
                    None => return Err(AuthError::NoMatchingKey),
                },
                None => {
                    let mut v: Vec<&DecodingKey> = self.anonymous.iter().collect();
                    v.extend(self.by_kid.values());
                    v
                }
            };

            let mut last = AuthError::NoMatchingKey;
            for key in candidates {
                match decode::<Claims>(token, key, &validation) {
                    Ok(data) => return Ok(data.claims.sub),
                    Err(e) => last = AuthError::Token(e.to_string()),
                }
            }
            Err(last)
        }
    }

    /// Verify a BYOM connect token (HS256, `MEM_CONNECT_SECRET`). Returns the
    /// token's `sub`, which the caller must match against the path `:sub`.
    pub fn verify_connect_token(secret: &str, token: &str) -> Result<String, AuthError> {
        let key = DecodingKey::from_secret(secret.as_bytes());
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        // Connect tokens are gateway-minted; no iss/aud constraints here.
        validation.validate_aud = false;
        decode::<Claims>(token, &key, &validation)
            .map(|d| d.claims.sub)
            .map_err(|e| AuthError::Token(e.to_string()))
    }

    #[derive(Debug, Serialize)]
    struct MintClaims {
        sub: String,
        exp: usize,
    }

    /// Mint an HS256 connect token for `sub`, expiring `ttl_secs` after
    /// `now_secs`. Symmetric with [`verify_connect_token`] — signed with the same
    /// `MEM_CONNECT_SECRET`, so a token minted here verifies there. This is what
    /// lets a client trade a verified OIDC id_token for a long-lived agent token,
    /// so a user never handles a raw secret.
    pub fn mint_connect_token(
        secret: &str,
        sub: &str,
        now_secs: usize,
        ttl_secs: usize,
    ) -> Result<String, AuthError> {
        let claims = MintClaims {
            sub: sub.to_string(),
            exp: now_secs.saturating_add(ttl_secs),
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .map_err(|e| AuthError::Token(e.to_string()))
    }
}

#[cfg(feature = "server")]
pub use verify::{AuthError, OidcVerifier};
#[cfg(feature = "server")]
pub use verify::{mint_connect_token, verify_connect_token};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{Membership, Role, Scope};

    fn membership(role: Role, scopes: Vec<Scope>) -> Membership {
        Membership {
            sub: "sub-1".into(),
            org: "citrate-federation".into(),
            role,
            scopes,
            parent: None,
        }
    }

    #[cfg(feature = "server")]
    fn now_secs() -> usize {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize
    }

    #[cfg(feature = "server")]
    #[test]
    fn minted_connect_token_verifies_and_carries_sub() {
        let secret = "connect-secret-xyz";
        // exp in the real future (jsonwebtoken validates exp against system time)
        let token = mint_connect_token(secret, "user-42", now_secs(), 3600).unwrap();
        assert_eq!(verify_connect_token(secret, &token).unwrap(), "user-42");
        // a different secret rejects it
        assert!(verify_connect_token("wrong-secret", &token).is_err());
    }

    #[cfg(feature = "server")]
    #[test]
    fn expired_connect_token_is_rejected() {
        let secret = "s";
        // minted well in the past (beyond jsonwebtoken's default 60s leeway)
        let token = mint_connect_token(secret, "u", now_secs() - 1000, 1).unwrap();
        assert!(verify_connect_token(secret, &token).is_err());
    }

    #[test]
    fn seed_key_is_deterministic() {
        let a = signing_key_from_seed("hunter2");
        let b = signing_key_from_seed("hunter2");
        assert_eq!(a.to_bytes(), b.to_bytes());
        let c = signing_key_from_seed("other");
        assert_ne!(a.to_bytes(), c.to_bytes());
    }

    #[test]
    fn owner_grant_is_wildcard_rw_and_verifies() {
        let sk = signing_key_from_seed("seed");
        let g = mint_grant(&sk, "https://auth.citrate.ai", &membership(Role::OrgOwner, vec![]), 1000, 60_000);
        g.verify_signature().expect("minted grant must verify");
        assert!(g.check("repo:citrate-chain/memory", mem_authz::Op::Write, 1500).is_ok());
        assert!(g.check("anything", mem_authz::Op::Read, 1500).is_ok());
    }

    #[test]
    fn readonly_cannot_write() {
        let sk = signing_key_from_seed("seed");
        let m = membership(
            Role::ReadOnly,
            vec![Scope { resource_id: "citrate-chain".into(), can_read: true, can_write: true }],
        );
        let g = mint_grant(&sk, "iss", &m, 1000, 60_000);
        // bare tenant name in the scope is normalized to the route resource id
        assert!(g.check("repo:citrate-chain/memory", mem_authz::Op::Read, 1500).is_ok());
        assert!(g.check("repo:citrate-chain/memory", mem_authz::Op::Write, 1500).is_err());
    }

    #[test]
    fn expired_grant_is_rejected() {
        let sk = signing_key_from_seed("seed");
        let g = mint_grant(&sk, "iss", &membership(Role::OrgOwner, vec![]), 1000, 60_000);
        assert!(g.check("*", mem_authz::Op::Read, 1000 + 60_001).is_err());
    }
}
