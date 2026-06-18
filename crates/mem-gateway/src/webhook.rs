//! MEM-S7 WP-7.2 — GitHub webhook receiver core (verification, allowlist, parse).
//!
//! These are pure functions with no axum/tokio dependency so they unit-test
//! without the `server` feature and stay easy to fuzz. The HTTP route in
//! `http.rs` composes them: verify the signature → allowlist the repo → parse the
//! push → enqueue an ingest job. The job is drained by the single-writer ingest
//! worker (WP-7.3) which owns the store lock; the receiver never writes the store
//! itself, and it never touches a working clone.
//!
//! Security posture (the "secure" in the secure feed):
//!   - Authenticity FIRST: an unverified body is never parsed for effect.
//!   - HMAC-SHA256 over the RAW bytes, keyed by the shared secret, constant-time
//!     compared. Fail CLOSED when no secret is configured.
//!   - Org allowlist: only `CitrateNetwork/<repo>` is accepted; the repo name is
//!     validated so a crafted name can never reach a shell or escape a path.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// The org whose repos this feed will ingest. A push for any other owner is
/// rejected before parsing.
pub const ALLOWED_OWNER: &str = "CitrateNetwork";

/// A decoded, trusted push event (only the fields the feed needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushEvent {
    /// Bare repository name (e.g. `citrate-identity`), already validated safe.
    pub repo: String,
    /// The ref that was pushed (e.g. `refs/heads/main`).
    pub git_ref: String,
    /// The new tip sha after the push (may be all-zero on a branch delete).
    pub after: String,
}

/// Verify a GitHub `X-Hub-Signature-256` header against the raw body.
///
/// The header is `sha256=<hex>`; we recompute HMAC-SHA256(secret, body) and
/// compare in constant time (the `hmac` crate's `verify_slice` is constant-time).
/// Returns false for a missing/malformed header or any mismatch. An empty secret
/// is a misconfiguration — callers MUST treat "no secret" as fail-closed BEFORE
/// calling this; passing an empty secret here always returns false.
pub fn verify_signature(secret: &[u8], signature_header: Option<&str>, body: &[u8]) -> bool {
    if secret.is_empty() {
        return false;
    }
    let header = match signature_header {
        Some(h) => h,
        None => return false,
    };
    let hex_sig = match header.strip_prefix("sha256=") {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let provided = match hex::decode(hex_sig) {
        Ok(b) => b,
        Err(_) => return false,
    };
    // new_from_slice never errors for HMAC (any key length is valid).
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

/// A bare repo name is safe iff it matches GitHub's own charset: ASCII
/// alphanumerics plus `-`, `_`, `.`, non-empty, not `.`/`..`. This guarantees it
/// can never be a path-traversal or shell metacharacter when later joined to a
/// workspace path or handed to `git`.
pub fn is_safe_repo_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." || name.len() > 100 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Decide whether a `full_name` ("owner/repo") is in-scope and safe. Returns the
/// bare repo name when accepted, else None (foreign owner or unsafe name).
pub fn allowed_repo(full_name: &str) -> Option<String> {
    let (owner, repo) = full_name.split_once('/')?;
    if owner != ALLOWED_OWNER {
        return None;
    }
    if !is_safe_repo_name(repo) {
        return None;
    }
    Some(repo.to_string())
}

/// Parse a GitHub push-event body into a {@link PushEvent}, enforcing the owner
/// allowlist and repo-name safety. Returns None for any payload that is not a
/// well-formed, in-scope push (malformed JSON, missing fields, foreign owner,
/// unsafe name). Call ONLY after {@link verify_signature} returned true.
pub fn parse_push_event(body: &[u8]) -> Option<PushEvent> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let full_name = v.get("repository")?.get("full_name")?.as_str()?;
    let repo = allowed_repo(full_name)?;
    let git_ref = v.get("ref")?.as_str()?.to_string();
    // `after` is present on push events; default to empty rather than failing so a
    // branch-delete (all-zero after) still parses and is handled downstream.
    let after = v.get("after").and_then(|a| a.as_str()).unwrap_or("").to_string();
    Some(PushEvent { repo, git_ref, after })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"whsec_test_secret";

    fn sign(secret: &[u8], body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn valid_signature_verifies() {
        let body = br#"{"hello":"world"}"#;
        let sig = sign(SECRET, body);
        assert!(verify_signature(SECRET, Some(&sig), body));
    }

    #[test]
    fn forged_or_missing_signature_rejected() {
        let body = br#"{"hello":"world"}"#;
        assert!(!verify_signature(SECRET, Some("sha256=deadbeef"), body));
        assert!(!verify_signature(SECRET, None, body));
        assert!(!verify_signature(SECRET, Some("garbage"), body));
        assert!(!verify_signature(SECRET, Some(""), body));
        // right MAC, wrong secret
        let other = sign(b"other-secret", body);
        assert!(!verify_signature(SECRET, Some(&other), body));
    }

    #[test]
    fn empty_secret_fails_closed() {
        let body = br#"{}"#;
        assert!(!verify_signature(b"", Some(&sign(b"", body)), body));
    }

    #[test]
    fn tampered_body_rejected() {
        let sig = sign(SECRET, br#"{"a":1}"#);
        assert!(!verify_signature(SECRET, Some(&sig), br#"{"a":2}"#));
    }

    #[test]
    fn allowlist_accepts_only_the_org_and_safe_names() {
        assert_eq!(allowed_repo("CitrateNetwork/citrate-identity").as_deref(), Some("citrate-identity"));
        assert_eq!(allowed_repo("evilcorp/citrate-identity"), None);
        assert_eq!(allowed_repo("CitrateNetwork/../etc/passwd"), None);
        assert_eq!(allowed_repo("CitrateNetwork/a;rm -rf"), None);
        assert_eq!(allowed_repo("no-slash"), None);
        assert_eq!(allowed_repo("CitrateNetwork/"), None);
    }

    #[test]
    fn parses_in_scope_push() {
        let body = br#"{"ref":"refs/heads/main","after":"abc123",
            "repository":{"full_name":"CitrateNetwork/citrate-chain"}}"#;
        let ev = parse_push_event(body).expect("should parse");
        assert_eq!(ev.repo, "citrate-chain");
        assert_eq!(ev.git_ref, "refs/heads/main");
        assert_eq!(ev.after, "abc123");
    }

    #[test]
    fn rejects_foreign_owner_and_malformed() {
        assert!(parse_push_event(br#"{"ref":"refs/heads/main","repository":{"full_name":"evil/x"}}"#).is_none());
        assert!(parse_push_event(b"not json").is_none());
        assert!(parse_push_event(br#"{"ref":"refs/heads/main"}"#).is_none()); // no repository
        assert!(parse_push_event(br#"{"repository":{"full_name":"CitrateNetwork/x"}}"#).is_none()); // no ref
    }
}
