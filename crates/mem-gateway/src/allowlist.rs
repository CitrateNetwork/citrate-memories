//! Team allowlist → just-in-time (JIT) membership provisioning.
//!
//! The gateway authorizes on an opaque OIDC `sub` (see [`crate::control`]). A
//! human's `sub` is not known until their first login, so a teammate cannot be
//! pre-authorized by the identifiers an operator actually holds — their **email**
//! or **wallet address**. This module closes that gap: an operator maintains a
//! small JSON allowlist keyed on verified email and/or wallet; on a teammate's
//! first authenticated request the gateway matches the token's claims against it
//! and provisions the membership just-in-time.
//!
//! **Fail-closed by construction.** An absent or unparseable file matches nobody
//! (the caller keeps its existing 403). Only a *verified* email is trusted
//! (`email_verified == true`) — an unverified email never grants access. Matching
//! is case-insensitive for email and lowercased for wallet addresses.

use serde::Deserialize;
use std::path::Path;

use crate::control::{Role, Scope};

/// One allowlisted teammate. At least one of `email` / `wallet` should be set;
/// an entry with neither can never match (and so grants nothing).
#[derive(Debug, Clone, Deserialize)]
pub struct AllowEntry {
    /// Human label for operators reading the file. Not used for matching.
    #[serde(default)]
    pub name: Option<String>,
    /// Verified email to match (case-insensitive). Only trusted when the token's
    /// `email_verified` is true.
    #[serde(default)]
    pub email: Option<String>,
    /// Wallet address to match (lowercased). Matches with today's `wallet` scope.
    #[serde(default)]
    pub wallet: Option<String>,
    /// Role to provision at. `Member`/`ReadOnly` should carry `scopes`; `OrgOwner`
    /// holds implicit `*` and needs none.
    pub role: Role,
    /// Resource scopes for non-owner roles (e.g. `{resource_id:"*",can_read:true}`).
    #[serde(default)]
    pub scopes: Vec<Scope>,
}

/// The parsed team allowlist. The wire format is `{ "members": [ AllowEntry... ] }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TeamAllowlist {
    #[serde(default)]
    pub members: Vec<AllowEntry>,
}

impl TeamAllowlist {
    /// Parse an allowlist from JSON text. Errors on malformed JSON.
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// Best-effort load that never errors: a missing file, an unreadable file, or
    /// malformed JSON all yield an empty allowlist (JIT provisions nobody this
    /// pass). A parse failure is logged so an operator can see the file is broken
    /// without the gateway failing open.
    pub fn load_or_empty(path: impl AsRef<Path>) -> Self {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(s) => Self::from_json(&s).unwrap_or_else(|e| {
                eprintln!(
                    "mem-gateway: team allowlist at {} failed to parse ({e}); JIT provisioning disabled this pass",
                    path.as_ref().display()
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Match a verified identity to an allowlist entry. An entry matches when its
    /// email equals the token's VERIFIED email (case-insensitive) OR its wallet
    /// equals the token's wallet (lowercased). Returns the first match.
    ///
    /// `email` is only considered when `email_verified` is true — an unverified
    /// email is treated as absent, closing the "claim any email" hole.
    pub fn match_identity(
        &self,
        email: Option<&str>,
        email_verified: bool,
        wallet: Option<&str>,
    ) -> Option<&AllowEntry> {
        let want_email = if email_verified {
            email.map(norm).filter(|s| !s.is_empty())
        } else {
            None
        };
        let want_wallet = wallet.map(norm).filter(|s| !s.is_empty());
        if want_email.is_none() && want_wallet.is_none() {
            return None;
        }
        self.members.iter().find(|m| {
            let m_email = m.email.as_deref().map(norm).filter(|s| !s.is_empty());
            let m_wallet = m.wallet.as_deref().map(norm).filter(|s| !s.is_empty());
            let email_hit = matches!((&want_email, &m_email), (Some(a), Some(b)) if a == b);
            let wallet_hit = matches!((&want_wallet, &m_wallet), (Some(a), Some(b)) if a == b);
            email_hit || wallet_hit
        })
    }
}

/// Trim + lowercase for case-insensitive identifier comparison.
fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> TeamAllowlist {
        TeamAllowlist::from_json(
            r#"{
              "members": [
                { "name": "Larry Klosowski", "email": "Larryklosowski@proton.me", "role": "OrgOwner" },
                { "name": "Lauren", "email": "lauren@projekt-blackfox.com",
                  "wallet": "0xABCdef0000000000000000000000000000000001",
                  "role": "Member", "scopes": [ { "resource_id": "*", "can_read": true } ] }
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn matches_verified_email_case_insensitively() {
        let a = fixture();
        let m = a
            .match_identity(Some("LAUREN@projekt-blackfox.com"), true, None)
            .expect("verified email should match");
        assert_eq!(m.role, Role::Member);
    }

    #[test]
    fn unverified_email_never_matches() {
        let a = fixture();
        assert!(
            a.match_identity(Some("lauren@projekt-blackfox.com"), false, None)
                .is_none(),
            "an unverified email must not grant access"
        );
    }

    #[test]
    fn matches_wallet_case_insensitively() {
        let a = fixture();
        let m = a
            .match_identity(
                None,
                false,
                Some("0xabcDEF0000000000000000000000000000000001"),
            )
            .expect("wallet should match regardless of email verification");
        assert_eq!(m.role, Role::Member);
    }

    #[test]
    fn no_identity_and_no_match_return_none() {
        let a = fixture();
        assert!(a.match_identity(None, false, None).is_none());
        assert!(a
            .match_identity(Some("stranger@example.com"), true, Some("0xdead"))
            .is_none());
    }

    #[test]
    fn missing_or_malformed_file_is_empty_not_error() {
        let empty = TeamAllowlist::load_or_empty("/nonexistent/team-allowlist.json");
        assert!(empty.members.is_empty());
        assert!(empty.match_identity(Some("a@b.com"), true, None).is_none());
    }
}
