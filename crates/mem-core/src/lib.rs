//! `mem-core` — the ontology and content-addressed identity for citrate-memories.
//!
//! This crate is pure data + hashing: no I/O, no async, no storage. It defines
//! what a memory node and edge *are*, and the one rule everything else depends
//! on — that identity is a function of content and structure only.
//!
//! See `PLANSET/02_ARCHITECTURE.md` for the full design and `00_OVERVIEW.md` for
//! the core invariant.

pub mod belnap;
pub mod edge;
pub mod identity;
pub mod node;

pub use belnap::{join_confidence, BelnapValue};
pub use edge::{Edge, EdgeKind, EdgeMethod, EdgeProvenance};
pub use identity::{ContentHash, IdBuilder};
pub use node::{
    ClaimStatus, CodeAnchor, MemoryNode, NarrativeKind, NodeKind, Plane, SourceRef, SprintPhase,
    Status, Timestamp, TrustTier, VersionedVector, SCHEMA_VERSION,
};
