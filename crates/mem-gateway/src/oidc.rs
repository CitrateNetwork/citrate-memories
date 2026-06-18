//! Real OIDC bearer verification (gateway gap **G-2**).
//!
//! Verifies a Citrate-issued **RS256** JWT against the authority's JWKS,
//! enforcing **issuer + audience + expiry**, and returns the subject. This is the
//! production replacement for the dev-only `x-dev-sub` header.
//!
//! **Fail-closed by construction:** an [`OidcVerifier`] only exists when issuer,
//! audience, and a parseable JWKS are all present — there is no "skip the check"
//! path. The algorithm is pinned to RS256 (never read from the attacker-controlled
//! token header), closing the alg-confusion class. Mirrors the webapp seam
//! (`session.ts` / FUA-EXPLORER-01): a token minted for a different relying party
//! (wrong `aud`) or a different authority (wrong `iss`) is refused.

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

/// Why a token was refused. Every variant means "reject" — there is no soft-pass.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OidcError {
    #[error("malformed jwks: {0}")]
    Jwks(String),
    #[error("token header missing kid")]
    NoKid,
    #[error("no jwks key for kid")]
    UnknownKid,
    #[error("token verification failed")]
    Invalid,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
}

/// Verifies authority-issued ID tokens against a fixed JWKS + issuer + audience.
pub struct OidcVerifier {
    issuer: String,
    audience: String,
    jwks: JwkSet,
}

impl OidcVerifier {
    /// Build from the authority's JWKS document (the JSON served at `/jwks`).
    /// Returns an error (so the caller fails closed) if the JWKS is unparseable.
    pub fn from_jwks_json(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_json: &str,
    ) -> Result<Self, OidcError> {
        let jwks: JwkSet = serde_json::from_str(jwks_json).map_err(|e| OidcError::Jwks(e.to_string()))?;
        if jwks.keys.is_empty() {
            return Err(OidcError::Jwks("empty key set".into()));
        }
        Ok(Self { issuer: issuer.into(), audience: audience.into(), jwks })
    }

    /// Verify a bearer token and return its subject, or an error (always = reject).
    pub fn verify(&self, token: &str) -> Result<String, OidcError> {
        let header = decode_header(token).map_err(|_| OidcError::Invalid)?;
        let kid = header.kid.ok_or(OidcError::NoKid)?;
        let jwk = self.jwks.find(&kid).ok_or(OidcError::UnknownKid)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| OidcError::Invalid)?;
        // Pin RS256 — never trust `header.alg` (alg-confusion). Enforce iss + aud + exp.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        let data = decode::<Claims>(token, &key, &validation).map_err(|_| OidcError::Invalid)?;
        if data.claims.sub.is_empty() {
            return Err(OidcError::Invalid);
        }
        Ok(data.claims.sub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    const ISSUER: &str = "https://auth.citrate.ai";
    const AUDIENCE: &str = "memrizz";
    const KID: &str = "mnemo-test-1";

    // Throwaway 2048-bit RSA key (NOT a real credential — test fixture only).
    const TEST_PRIV_PEM: &str = include_str!("testdata/oidc_test_rsa.pem");
    const TEST_N: &str = "ndUDVqo8DrYXkGs7uzDoQ9vMUkF7ZjF6m9XKxwWCHg0MZqaPJtG3Bwi48bfTLmnEvTs_PtHpLtU6XtmCu8rdOzpTM10yK5qLRDd0jcNPhMjIZf5MP5VTebkUmp5A9eGxdhh1tkoSv0SM_BBoLNInq_X6TUVWNmvDxYD21YNY_Y6iNP-VsaDrxdn7BejnNrULCDidjbn2f15ma0F0xZg4kiurVMu72XzXpsbqUCN_on60jEa2auYJXCUShlJIfZQ4e449Se8o0X_NhjhxbgMfSAegRsI5i42oskNmU9z1TihFQXcWQG42aHbJVZmMOpnuOUn7i-_yjey49IFznr4IsQ";
    const TEST_E: &str = "AQAB";

    fn jwks_json() -> String {
        json!({ "keys": [{ "kty": "RSA", "use": "sig", "alg": "RS256", "kid": KID, "n": TEST_N, "e": TEST_E }] }).to_string()
    }

    fn mint(iss: &str, aud: &str, sub: &str, exp_offset_secs: i64) -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let claims = json!({ "iss": iss, "aud": aud, "sub": sub, "iat": now - 60, "exp": now + exp_offset_secs });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.to_string());
        let key = EncodingKey::from_rsa_pem(TEST_PRIV_PEM.as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    fn verifier() -> OidcVerifier {
        OidcVerifier::from_jwks_json(ISSUER, AUDIENCE, &jwks_json()).unwrap()
    }

    #[test]
    fn accepts_a_valid_authority_token_and_returns_sub() {
        let v = verifier();
        let token = mint(ISSUER, AUDIENCE, "did:citrate:aleia", 300);
        assert_eq!(v.verify(&token).unwrap(), "did:citrate:aleia");
    }

    #[test]
    fn rejects_wrong_audience() {
        let v = verifier();
        let token = mint(ISSUER, "some-other-rp", "did:citrate:aleia", 300);
        assert_eq!(v.verify(&token), Err(OidcError::Invalid));
    }

    #[test]
    fn rejects_wrong_issuer() {
        let v = verifier();
        let token = mint("https://evil.example", AUDIENCE, "did:citrate:aleia", 300);
        assert_eq!(v.verify(&token), Err(OidcError::Invalid));
    }

    #[test]
    fn rejects_expired_token() {
        let v = verifier();
        let token = mint(ISSUER, AUDIENCE, "did:citrate:aleia", -120);
        assert_eq!(v.verify(&token), Err(OidcError::Invalid));
    }

    #[test]
    fn rejects_garbage_and_unknown_kid() {
        let v = verifier();
        assert_eq!(v.verify("not.a.jwt"), Err(OidcError::Invalid));
        // a token whose kid isn't in the JWKS
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let claims = json!({ "iss": ISSUER, "aud": AUDIENCE, "sub": "x", "exp": now + 300 });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("unknown-kid".into());
        let key = EncodingKey::from_rsa_pem(TEST_PRIV_PEM.as_bytes()).unwrap();
        let token = encode(&header, &claims, &key).unwrap();
        assert_eq!(v.verify(&token), Err(OidcError::UnknownKid));
    }

    #[test]
    fn unparseable_or_empty_jwks_fails_closed() {
        assert!(OidcVerifier::from_jwks_json(ISSUER, AUDIENCE, "not json").is_err());
        assert!(OidcVerifier::from_jwks_json(ISSUER, AUDIENCE, r#"{"keys":[]}"#).is_err());
    }
}
