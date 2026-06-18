//! The SaaS control-plane: Orgs, memberships, and roles.
//!
//! **Org** (a.k.a. Workspace) is the SaaS isolation boundary — the customer. It is
//! NOT the engine's `tenant` (which is a *repo* inside an Org's federation). Every
//! Org has its own isolated memory store; this module holds the *control plane*
//! (who belongs to which Org, in what role, with what scopes) — never memory
//! content. See `PLANSET/06_WEBAPP_FRONTEND_SPEC.md §2.0/§3`.

use serde::{Deserialize, Serialize};

use mem_authz::ResourceScope;

/// A SaaS Org / Workspace identifier (a stable slug).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OrgId(pub String);

impl OrgId {
    pub fn new(s: impl Into<String>) -> Self {
        OrgId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lifecycle of an Org (the Platform Operator manages this; it never reads content).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgStatus {
    Active,
    Suspended,
}

/// A SaaS customer Org. `store_path` locates its *isolated* engine store; two Orgs
/// never share a store (isolation by construction, not by query filter).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    pub id: OrgId,
    pub name: String,
    pub created_at_ms: u64,
    /// Filesystem path of this Org's isolated encrypted store.
    pub store_path: String,
    pub status: OrgStatus,
}

/// A member's role *within one Org*. The two admin tiers the product needs are
/// `OrgOwner` (super admin) and `OrgAdmin` (regular admin); `Member` is a leaf.
/// (The Platform Operator is a separate, content-walled role, not modeled here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Member,
    OrgAdmin,
    OrgOwner,
}

impl Role {
    /// Can this role administer other members (onboard/grant) at all?
    pub fn is_admin(&self) -> bool {
        matches!(self, Role::OrgAdmin | Role::OrgOwner)
    }
}

/// One person's membership in one Org: their role, the repo-tenant scopes they hold,
/// and who delegated them (the parent in the RBAC tree, by `sub`). `None` parent ⇒ a
/// root grant (the founding Org Owner).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// The OIDC subject (global principal) this membership belongs to.
    pub sub: String,
    pub org: OrgId,
    pub role: Role,
    /// The repo-tenant scopes this member holds *within the Org*. An Org Owner's
    /// effective scope is `*` regardless of this list (see `effective_scopes`).
    pub scopes: Vec<ResourceScope>,
    /// The `sub` of the member who delegated this one (the RBAC parent). `None` for
    /// the founding Org Owner. The delegation tree is reconstructed from these links
    /// (and is what a revocation cascade walks — F-7).
    pub parent: Option<String>,
}

impl Membership {
    /// The scopes this membership *effectively* holds. An Org Owner implicitly holds
    /// `*` within the Org; everyone else holds exactly their granted `scopes`.
    pub fn effective_scopes(&self) -> Vec<ResourceScope> {
        if self.role == Role::OrgOwner {
            vec![ResourceScope { resource_id: "*".into(), can_read: true, can_write: true }]
        } else {
            self.scopes.clone()
        }
    }
}

/// The control-plane store: the source of truth for Orgs + memberships. M0 keeps it
/// in memory with JSON (de)serialization for persistence; the production backend
/// (RocksDB control CF or Postgres) implements the same shape behind this type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ControlPlane {
    orgs: Vec<Org>,
    memberships: Vec<Membership>,
}

impl ControlPlane {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// G-3 — durable load. Read the control plane from `path`, or return a fresh
    /// empty one if the file does not exist yet (first boot). Any other IO/parse
    /// error is surfaced so a corrupt control plane fails LOUD rather than silently
    /// resetting Org membership.
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_json(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("read control plane {}: {e}", path.display())),
        }
    }

    /// G-3 — durable, **atomic** save. Serialize, write to a temp sibling, fsync,
    /// then rename over the target (a crash mid-write leaves the previous good file
    /// intact — never a half-written control plane). Creates the parent dir if
    /// needed.
    pub fn save_atomic(&self, path: &std::path::Path) -> Result<(), String> {
        use std::io::Write;
        let json = self.to_json()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
            f.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn upsert_org(&mut self, org: Org) {
        if let Some(existing) = self.orgs.iter_mut().find(|o| o.id == org.id) {
            *existing = org;
        } else {
            self.orgs.push(org);
        }
    }

    pub fn org(&self, id: &OrgId) -> Option<&Org> {
        self.orgs.iter().find(|o| &o.id == id)
    }

    pub fn orgs(&self) -> &[Org] {
        &self.orgs
    }

    /// The Org(s) a `sub` belongs to (a user may be in several — e.g. a consultant).
    pub fn orgs_for(&self, sub: &str) -> Vec<&Org> {
        self.memberships
            .iter()
            .filter(|m| m.sub == sub)
            .filter_map(|m| self.org(&m.org))
            .collect()
    }

    /// A `sub`'s membership in a specific Org, if any. This is the **Org-boundary
    /// gate**: `None` ⇒ the caller is not in this Org and must be refused before any
    /// scope check or store access.
    pub fn membership(&self, sub: &str, org: &OrgId) -> Option<&Membership> {
        self.memberships.iter().find(|m| m.sub == sub && &m.org == org)
    }

    /// All memberships in an Org (the roster + delegation tree). Org-scoped read.
    pub fn members_of(&self, org: &OrgId) -> Vec<&Membership> {
        self.memberships.iter().filter(|m| &m.org == org).collect()
    }

    /// All direct children of a member in an Org's delegation tree (by parent `sub`).
    pub fn children(&self, org: &OrgId, parent_sub: &str) -> Vec<&Membership> {
        self.memberships
            .iter()
            .filter(|m| &m.org == org && m.parent.as_deref() == Some(parent_sub))
            .collect()
    }

    /// Insert or replace a membership (caller must have already checked the
    /// delegating admin's authority + attenuation — see `authz::can_delegate`).
    pub fn upsert_membership(&mut self, m: Membership) {
        if let Some(existing) = self.memberships.iter_mut().find(|x| x.sub == m.sub && x.org == m.org) {
            *existing = m;
        } else {
            self.memberships.push(m);
        }
    }

    /// Remove a membership AND everything delegated beneath it, transitively (the
    /// revocation cascade, F-7). Returns the `sub`s removed (for audit).
    pub fn revoke_cascade(&mut self, org: &OrgId, sub: &str) -> Vec<String> {
        // Collect the subtree (BFS over parent links) before removing.
        let mut to_remove = vec![sub.to_string()];
        let mut frontier = vec![sub.to_string()];
        while let Some(p) = frontier.pop() {
            for child in self.children(org, &p) {
                let cs = child.sub.clone();
                if !to_remove.contains(&cs) {
                    to_remove.push(cs.clone());
                    frontier.push(cs);
                }
            }
        }
        self.memberships.retain(|m| !(&m.org == org && to_remove.contains(&m.sub)));
        to_remove
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(id: &str, r: bool, w: bool) -> ResourceScope {
        ResourceScope { resource_id: id.into(), can_read: r, can_write: w }
    }

    fn org(id: &str) -> Org {
        Org { id: OrgId::new(id), name: id.into(), created_at_ms: 1, store_path: format!("/data/{id}"), status: OrgStatus::Active }
    }

    fn member(sub: &str, org: &str, role: Role, scopes: Vec<ResourceScope>, parent: Option<&str>) -> Membership {
        Membership { sub: sub.into(), org: OrgId::new(org), role, scopes, parent: parent.map(|s| s.into()) }
    }

    #[test]
    fn membership_is_the_org_boundary() {
        let mut cp = ControlPlane::new();
        cp.upsert_org(org("acme"));
        cp.upsert_org(org("globex"));
        cp.upsert_membership(member("alice", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], None));

        assert!(cp.membership("alice", &OrgId::new("acme")).is_some());
        // Alice is NOT in globex — the boundary gate returns None.
        assert!(cp.membership("alice", &OrgId::new("globex")).is_none());
        // A stranger is in no org.
        assert!(cp.membership("mallory", &OrgId::new("acme")).is_none());
    }

    #[test]
    fn control_plane_persists_durably_and_atomically() {
        // G-3: a saved control plane survives a reload (process restart proxy).
        let dir = std::env::temp_dir().join(format!("mnemo-cp-{}", std::process::id()));
        let path = dir.join("control.json");
        let _ = std::fs::remove_dir_all(&dir);

        // Absent file → fresh empty plane (first boot).
        assert!(ControlPlane::load_or_default(&path).unwrap().orgs().is_empty());

        let mut cp = ControlPlane::new();
        cp.upsert_org(org("acme"));
        cp.upsert_membership(member("alice", "acme", Role::OrgOwner, vec![], None));
        cp.save_atomic(&path).unwrap();

        // Reload reconstructs orgs + memberships exactly.
        let loaded = ControlPlane::load_or_default(&path).unwrap();
        assert_eq!(loaded.org(&OrgId::new("acme")).unwrap().name, "acme");
        assert!(loaded.membership("alice", &OrgId::new("acme")).is_some());

        // The temp sidecar must not linger after a successful atomic rename.
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn org_owner_effective_scope_is_star() {
        let owner = member("o", "acme", Role::OrgOwner, vec![], None);
        let eff = owner.effective_scopes();
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].resource_id, "*");
        assert!(eff[0].can_read && eff[0].can_write);
    }

    #[test]
    fn revoke_cascades_down_the_delegation_tree() {
        let mut cp = ControlPlane::new();
        cp.upsert_org(org("acme"));
        // owner -> admin(dana) -> member(bob) -> member(carol)
        cp.upsert_membership(member("owner", "acme", Role::OrgOwner, vec![], None));
        cp.upsert_membership(member("dana", "acme", Role::OrgAdmin, vec![scope("repo:x/memory", true, true)], Some("owner")));
        cp.upsert_membership(member("bob", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], Some("dana")));
        cp.upsert_membership(member("carol", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], Some("bob")));

        let acme = OrgId::new("acme");
        // Revoking dana removes dana + bob + carol, but not the owner.
        let removed = cp.revoke_cascade(&acme, "dana");
        assert!(removed.contains(&"dana".to_string()));
        assert!(removed.contains(&"bob".to_string()));
        assert!(removed.contains(&"carol".to_string()));
        assert!(cp.membership("owner", &acme).is_some());
        assert!(cp.membership("dana", &acme).is_none());
        assert!(cp.membership("bob", &acme).is_none());
        assert!(cp.membership("carol", &acme).is_none());
    }

    #[test]
    fn control_plane_round_trips_through_json() {
        let mut cp = ControlPlane::new();
        cp.upsert_org(org("acme"));
        cp.upsert_membership(member("alice", "acme", Role::OrgAdmin, vec![scope("repo:x/memory", true, true)], Some("owner")));
        let j = cp.to_json().unwrap();
        let back = ControlPlane::from_json(&j).unwrap();
        assert_eq!(back.membership("alice", &OrgId::new("acme")).unwrap().role, Role::OrgAdmin);
    }
}
