//! `mem-authz` — authorization and accountability for citrate-memories.
//!
//! [`CapabilityGrant`] is the signed capability an agent presents; [`AuditChain`]
//! is the tamper-evident log of what it did. Together they answer "may this agent
//! do this?" and "what did it do?".

pub mod audit;
pub mod grant;

pub use audit::{AuditChain, AuditError, AuditRecord, MemoryEvent};
pub use grant::{AuthzError, CapabilityGrant, DelegationStep, Op, PolicyProfile, ResourceScope};
