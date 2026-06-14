//! The authorization gate: OIDC subject → signed `CapabilityGrant` → scope check,
//! with the **Org boundary checked first**, plus delegation attenuation.
//!
//! The gateway is the **grant issuer**: per request (or session) it mints a signed
//! `CapabilityGrant` whose `recipient` is the OIDC `sub` and whose scopes come from
//! the member's role + grants. This binds authority to the real identity (F-5) and
//! lets us reuse the engine's audited `CapabilityGrant::check` verbatim.

use ed25519_dalek::SigningKey;

use mem_authz::{CapabilityGrant, DelegationStep, Op, PolicyProfile, ResourceScope};

use crate::org::{ControlPlane, Membership, OrgId, OrgStatus, Role};
use crate::GatewayError;

/// Map a role to its policy profile (the engine's coarse capability tier).
fn policy_for(role: Role, scopes: &[ResourceScope]) -> PolicyProfile {
    match role {
        Role::OrgOwner => PolicyProfile::Maintainer,
        Role::OrgAdmin => PolicyProfile::Operator,
        Role::Member => {
            if scopes.iter().any(|s| s.can_write) {
                PolicyProfile::Guided
            } else {
                PolicyProfile::ReadOnly
            }
        }
    }
}

/// Mint a signed `CapabilityGrant` for a membership, valid until `now_ms + ttl_ms`.
/// The grant's `recipient` is the member's `sub` (identity-bound, F-5) and its
/// scopes are the member's *effective* scopes (Org Owner ⇒ `*` within the Org). The
/// delegation chain records the parent so a downstream revocation cascade is
/// reconstructable. Signed with the gateway's issuer key so `check()` passes.
pub fn derive_grant(m: &Membership, now_ms: u64, ttl_ms: u64, issuer_key: &SigningKey) -> CapabilityGrant {
    let scopes = m.effective_scopes();
    let policy = policy_for(m.role, &scopes);
    let delegation_chain = m
        .parent
        .as_ref()
        .map(|p| vec![DelegationStep { delegator: p.clone(), at_ms: now_ms }])
        .unwrap_or_default();
    let mut grant = CapabilityGrant {
        id: format!("{}:{}", m.org.as_str(), m.sub),
        issuer: format!("mem-gateway:{}", m.org.as_str()),
        recipient: m.sub.clone(),
        allowed_resources: scopes,
        policy,
        expires_at_ms: now_ms.saturating_add(ttl_ms),
        revoked: false,
        delegation_chain,
        issuer_pubkey: vec![],
        signature: vec![],
    };
    grant.sign_with(issuer_key);
    grant
}

/// The request authorization gate. Returns `Ok(())` iff:
/// 1. the Org is active, AND
/// 2. `sub` is a member of `org` (the **Org boundary** — checked before anything
///    touches the Org's store), AND
/// 3. the member's derived grant permits `op` on `resource_id`
///    (`repo:<tenant>/memory` or `*`).
#[allow(clippy::too_many_arguments)]
pub fn authorize(
    control: &ControlPlane,
    sub: &str,
    org: &OrgId,
    resource_id: &str,
    op: Op,
    now_ms: u64,
    ttl_ms: u64,
    issuer_key: &SigningKey,
) -> Result<(), GatewayError> {
    // Org must exist + be active. (Coarse: a missing org reads like a non-membership
    // to avoid leaking org existence.)
    match control.org(org) {
        Some(o) if o.status == OrgStatus::Active => {}
        Some(_) => return Err(GatewayError::OrgSuspended),
        None => return Err(GatewayError::NotAMember),
    }
    // Org-boundary gate: membership FIRST.
    let membership = control.membership(sub, org).ok_or(GatewayError::NotAMember)?;
    // Scope check via the engine's audited grant logic.
    let grant = derive_grant(membership, now_ms, ttl_ms, issuer_key);
    grant.check(resource_id, op, now_ms)?;
    Ok(())
}

/// Does `parent_scopes` cover `child` (resource match + no escalation of read/write)?
/// A `*` parent scope covers any child; otherwise resource ids must match exactly.
fn scope_covers(parent_scopes: &[ResourceScope], child: &ResourceScope) -> bool {
    parent_scopes.iter().any(|p| {
        let resource_ok = p.resource_id == "*" || p.resource_id == child.resource_id;
        resource_ok && (p.can_read || !child.can_read) && (p.can_write || !child.can_write)
    })
}

/// Attenuation gate for onboarding/delegation: `delegator` may create a membership
/// with `child_role` + `child_scopes` only if:
/// - the delegator is an admin (Org Owner or Org Admin), AND
/// - the child's role does not exceed the delegator's (only an Owner can mint an
///   Owner/Admin; an Admin can only mint Members ⟦policy⟧), AND
/// - every child scope is covered by the delegator's effective scopes (no granting
///   what you don't hold).
pub fn can_delegate(
    delegator: &Membership,
    child_role: Role,
    child_scopes: &[ResourceScope],
) -> Result<(), GatewayError> {
    if !delegator.role.is_admin() {
        return Err(GatewayError::NotADelegator);
    }
    // Role ceiling: an Org Admin cannot mint Admins or Owners; only an Owner can.
    let role_ok = match delegator.role {
        Role::OrgOwner => true, // can mint any role
        Role::OrgAdmin => matches!(child_role, Role::Member),
        Role::Member => false,
    };
    if !role_ok {
        return Err(GatewayError::Attenuation(format!(
            "{:?} may not create a {:?}",
            delegator.role, child_role
        )));
    }
    let parent_scopes = delegator.effective_scopes();
    for cs in child_scopes {
        if !scope_covers(&parent_scopes, cs) {
            return Err(GatewayError::Attenuation(format!(
                "scope {} (r{} w{}) exceeds delegator authority",
                cs.resource_id, cs.can_read, cs.can_write
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org::{Org, OrgStatus};

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }
    fn scope(id: &str, r: bool, w: bool) -> ResourceScope {
        ResourceScope { resource_id: id.into(), can_read: r, can_write: w }
    }
    fn org_active(id: &str) -> Org {
        Org { id: OrgId::new(id), name: id.into(), created_at_ms: 1, store_path: format!("/d/{id}"), status: OrgStatus::Active }
    }
    fn member(sub: &str, org: &str, role: Role, scopes: Vec<ResourceScope>, parent: Option<&str>) -> Membership {
        Membership { sub: sub.into(), org: OrgId::new(org), role, scopes, parent: parent.map(|s| s.into()) }
    }
    fn cp_with(memberships: Vec<Membership>) -> ControlPlane {
        let mut cp = ControlPlane::new();
        cp.upsert_org(org_active("acme"));
        for m in memberships {
            cp.upsert_membership(m);
        }
        cp
    }

    const TTL: u64 = 3_600_000;

    #[test]
    fn member_can_read_granted_tenant() {
        let cp = cp_with(vec![member("alice", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], None)]);
        assert!(authorize(&cp, "alice", &OrgId::new("acme"), "repo:x/memory", Op::Read, 1, TTL, &key()).is_ok());
    }

    #[test]
    fn member_cannot_write_read_only_scope() {
        let cp = cp_with(vec![member("alice", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], None)]);
        let err = authorize(&cp, "alice", &OrgId::new("acme"), "repo:x/memory", Op::Write, 1, TTL, &key()).unwrap_err();
        assert!(matches!(err, GatewayError::Authz(mem_authz::AuthzError::OperationDenied(Op::Write))));
    }

    #[test]
    fn member_cannot_touch_ungranted_tenant() {
        let cp = cp_with(vec![member("alice", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], None)]);
        let err = authorize(&cp, "alice", &OrgId::new("acme"), "repo:secret/memory", Op::Read, 1, TTL, &key()).unwrap_err();
        assert!(matches!(err, GatewayError::Authz(mem_authz::AuthzError::ResourceDenied(_))));
    }

    #[test]
    fn cross_org_access_is_refused_at_the_boundary() {
        // Alice is a member of acme only.
        let mut cp = cp_with(vec![member("alice", "acme", Role::OrgOwner, vec![], None)]);
        cp.upsert_org(org_active("globex"));
        // Even as an owner in acme, alice has no membership in globex → NotAMember,
        // BEFORE any scope check or store access.
        let err = authorize(&cp, "alice", &OrgId::new("globex"), "repo:x/memory", Op::Read, 1, TTL, &key()).unwrap_err();
        assert_eq!(err, GatewayError::NotAMember);
    }

    #[test]
    fn org_owner_reads_everything_in_their_org() {
        let cp = cp_with(vec![member("o", "acme", Role::OrgOwner, vec![], None)]);
        assert!(authorize(&cp, "o", &OrgId::new("acme"), "repo:anything/memory", Op::Read, 1, TTL, &key()).is_ok());
        assert!(authorize(&cp, "o", &OrgId::new("acme"), "repo:anything/memory", Op::Write, 1, TTL, &key()).is_ok());
    }

    #[test]
    fn suspended_org_is_refused() {
        let mut cp = ControlPlane::new();
        let mut o = org_active("acme");
        o.status = OrgStatus::Suspended;
        cp.upsert_org(o);
        cp.upsert_membership(member("alice", "acme", Role::OrgOwner, vec![], None));
        let err = authorize(&cp, "alice", &OrgId::new("acme"), "repo:x/memory", Op::Read, 1, TTL, &key()).unwrap_err();
        assert_eq!(err, GatewayError::OrgSuspended);
    }

    #[test]
    fn expired_grant_is_refused() {
        let cp = cp_with(vec![member("alice", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], None)]);
        // ttl 0 => expires_at == now => the engine's `now >= expires_at` => Expired.
        let err = authorize(&cp, "alice", &OrgId::new("acme"), "repo:x/memory", Op::Read, 50, 0, &key()).unwrap_err();
        assert!(matches!(err, GatewayError::Authz(mem_authz::AuthzError::Expired)));
        // A live ttl on the same request authorizes fine (control case).
        assert!(authorize(&cp, "alice", &OrgId::new("acme"), "repo:x/memory", Op::Read, 50, TTL, &key()).is_ok());
    }

    // ---- attenuation / delegation ----

    #[test]
    fn admin_can_delegate_a_subset_to_a_member() {
        let admin = member("dana", "acme", Role::OrgAdmin, vec![scope("repo:x/memory", true, true), scope("repo:y/memory", true, false)], Some("owner"));
        // child gets read-only on x — a subset.
        assert!(can_delegate(&admin, Role::Member, &[scope("repo:x/memory", true, false)]).is_ok());
    }

    #[test]
    fn admin_cannot_grant_scope_they_lack() {
        let admin = member("dana", "acme", Role::OrgAdmin, vec![scope("repo:x/memory", true, false)], Some("owner"));
        // can't grant write they don't hold...
        assert!(matches!(can_delegate(&admin, Role::Member, &[scope("repo:x/memory", true, true)]), Err(GatewayError::Attenuation(_))));
        // ...nor a tenant they don't hold.
        assert!(matches!(can_delegate(&admin, Role::Member, &[scope("repo:z/memory", true, false)]), Err(GatewayError::Attenuation(_))));
    }

    #[test]
    fn admin_cannot_mint_another_admin_or_owner() {
        let admin = member("dana", "acme", Role::OrgAdmin, vec![scope("repo:x/memory", true, true)], Some("owner"));
        assert!(matches!(can_delegate(&admin, Role::OrgAdmin, &[]), Err(GatewayError::Attenuation(_))));
        assert!(matches!(can_delegate(&admin, Role::OrgOwner, &[]), Err(GatewayError::Attenuation(_))));
    }

    #[test]
    fn owner_can_mint_admins_with_subset_scopes() {
        let owner = member("o", "acme", Role::OrgOwner, vec![], None); // effective *
        assert!(can_delegate(&owner, Role::OrgAdmin, &[scope("repo:x/memory", true, true)]).is_ok());
        assert!(can_delegate(&owner, Role::Member, &[scope("repo:any/memory", true, false)]).is_ok());
    }

    #[test]
    fn a_member_cannot_delegate_at_all() {
        let m = member("bob", "acme", Role::Member, vec![scope("repo:x/memory", true, false)], Some("dana"));
        assert_eq!(can_delegate(&m, Role::Member, &[scope("repo:x/memory", true, false)]).unwrap_err(), GatewayError::NotADelegator);
    }
}
