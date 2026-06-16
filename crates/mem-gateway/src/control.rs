//! The gateway control plane: Orgs and memberships, persisted to `control.json`.
//!
//! This is net-new gateway state (the `mem-*` crates model tenants as the `repo`
//! field on every node; they have no concept of an Org or who may read it). An
//! **Org** is a named, isolated unit that owns exactly one store (Org isolation
//! is physical — never a shared store behind a query filter). A **membership**
//! binds an OIDC `sub` to a role within one Org.
//!
//! Persistence is crash-atomic: writes go to `control.json.tmp` then rename over
//! the live file. Losing `control.json` loses memberships, so it must live on a
//! durable, backed-up volume (see the deploy handoff §1e).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("control file io: {0}")]
    Io(String),
    #[error("control file parse: {0}")]
    Parse(String),
}

/// Lifecycle of an Org. `Suspended` Orgs reject all routes (fail closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgStatus {
    Active,
    Suspended,
}

/// A role within one Org. `OrgOwner` implicitly holds `*` read+write, which is
/// why the founding owner's `scopes` is empty (see the handoff bootstrap §1e).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    OrgOwner,
    Admin,
    Member,
    ReadOnly,
}

impl Role {
    /// Owners and admins manage memberships; members and read-only cannot.
    pub fn can_administer(self) -> bool {
        matches!(self, Role::OrgOwner | Role::Admin)
    }
}

/// A single resource grant for non-owner members. `resource_id` is a repo tenant
/// such as `"citrate-chain"`, or `"*"` for the whole Org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub resource_id: String,
    #[serde(default)]
    pub can_read: bool,
    #[serde(default)]
    pub can_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub store_path: String,
    pub status: OrgStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub sub: String,
    pub org: String,
    pub role: Role,
    /// Extra scopes for non-owner roles. Ignored for `OrgOwner` (implicit `*`).
    #[serde(default)]
    pub scopes: Vec<Scope>,
    /// The membership this one was delegated from, if any (attenuation audit).
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Control {
    #[serde(default)]
    pub orgs: Vec<Org>,
    #[serde(default)]
    pub memberships: Vec<Membership>,
}

impl Control {
    /// Load from disk, or return an empty control plane if the file is absent.
    /// A present-but-corrupt file is an error (fail closed — never silently
    /// start with no memberships when one was expected).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| ControlError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Control::default()),
            Err(e) => Err(ControlError::Io(e.to_string())),
        }
    }

    /// Atomically persist (`tmp` + rename). Creates parent dirs as needed.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ControlError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| ControlError::Io(e.to_string()))?;
            }
        }
        let json =
            serde_json::to_string_pretty(self).map_err(|e| ControlError::Parse(e.to_string()))?;
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes()).map_err(|e| ControlError::Io(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| ControlError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn org(&self, id: &str) -> Option<&Org> {
        self.orgs.iter().find(|o| o.id == id)
    }

    pub fn membership(&self, sub: &str, org: &str) -> Option<&Membership> {
        self.memberships
            .iter()
            .find(|m| m.sub == sub && m.org == org)
    }

    /// Ensure an Org row exists (idempotent). Returns `true` if newly created.
    pub fn ensure_org(&mut self, id: &str, store_path: &str, now_ms: u64) -> bool {
        if self.org(id).is_some() {
            return false;
        }
        self.orgs.push(Org {
            id: id.to_string(),
            name: id.to_string(),
            created_at_ms: now_ms,
            store_path: store_path.to_string(),
            status: OrgStatus::Active,
        });
        true
    }

    /// Upsert a membership at the given role (used by `--bootstrap-owner`).
    /// Returns `true` if it created or changed a row.
    pub fn upsert_membership(&mut self, sub: &str, org: &str, role: Role) -> bool {
        if let Some(m) = self
            .memberships
            .iter_mut()
            .find(|m| m.sub == sub && m.org == org)
        {
            if m.role == role {
                return false;
            }
            m.role = role;
            return true;
        }
        self.memberships.push(Membership {
            sub: sub.to_string(),
            org: org.to_string(),
            role,
            scopes: Vec::new(),
            parent: None,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_implicit_admin_others_not() {
        assert!(Role::OrgOwner.can_administer());
        assert!(Role::Admin.can_administer());
        assert!(!Role::Member.can_administer());
        assert!(!Role::ReadOnly.can_administer());
    }

    #[test]
    fn ensure_org_and_membership_are_idempotent() {
        let mut c = Control::default();
        assert!(c.ensure_org("citrate-federation", "/x.memdag", 1));
        assert!(!c.ensure_org("citrate-federation", "/x.memdag", 2));
        assert!(c.upsert_membership("sub-1", "citrate-federation", Role::OrgOwner));
        assert!(!c.upsert_membership("sub-1", "citrate-federation", Role::OrgOwner));
        assert!(c.upsert_membership("sub-1", "citrate-federation", Role::Admin));
        let m = c.membership("sub-1", "citrate-federation").unwrap();
        assert_eq!(m.role, Role::Admin);
    }

    #[test]
    fn load_missing_is_empty_not_error() {
        let c = Control::load("/nonexistent/path/control.json").unwrap();
        assert!(c.orgs.is_empty() && c.memberships.is_empty());
    }
}
