//! `mem-gateway` — the Memrizz backend seam.
//!
//! Turns the headless citrate-memories engine into a multi-Org SaaS surface. This
//! crate's M0 core is the **security-critical** part: Org isolation, the
//! control-plane (who is in which Org, in what role), and the authorization gate
//! that maps an authenticated OIDC subject → a signed `CapabilityGrant` and checks
//! the **Org boundary first**, then the repo-tenant scope. The HTTP/SSE + BYOM
//! MCP-over-HTTP layer sits on top of this core (next milestone).
//!
//! Design: `PLANSET/06_WEBAPP_FRONTEND_SPEC.md`. Two hard rules this core enforces:
//! 1. **Org isolation is physical** — each Org's memory lives in its own store
//!    ([`registry`]); the gateway routes by Org and never lets a query cross Orgs.
//! 2. **Authority attenuates** — a delegated membership can never exceed its
//!    delegator's scopes/role ([`authz::can_delegate`]); revocation cascades
//!    ([`org::ControlPlane::revoke_cascade`]).

pub mod authz;
pub mod layout;
pub mod oidc;
pub mod org;
pub mod registry;

#[cfg(feature = "server")]
pub mod http;

pub use authz::{authorize, can_delegate, derive_grant};
pub use org::{ControlPlane, Membership, Org, OrgId, OrgStatus, Role};

/// Everything that can go wrong at the gateway boundary. Each variant fails
/// **closed** — the caller is refused and the reason is audit-loggable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GatewayError {
    /// The subject is not a member of the requested Org — the Org-boundary gate.
    /// Deliberately coarse (don't leak whether the Org exists).
    #[error("not a member of org")]
    NotAMember,
    /// The Org exists but is suspended (Platform Operator action).
    #[error("org suspended")]
    OrgSuspended,
    /// No engine instance is available for the Org (not provisioned / store missing).
    #[error("org engine unavailable: {0}")]
    EngineUnavailable(String),
    /// Scope/expiry/signature check on the derived grant failed.
    #[error("authorization denied: {0}")]
    Authz(#[from] mem_authz::AuthzError),
    /// A delegation that would exceed the delegator's authority (attenuation).
    #[error("delegation exceeds delegator authority: {0}")]
    Attenuation(String),
    /// The acting member lacks admin rights to delegate at all.
    #[error("not authorized to delegate")]
    NotADelegator,
    #[error("store error: {0}")]
    Store(String),
}
