//! `mem-gateway` — an HTTP transport over the citrate-memories DAG.
//!
//! This crate is **net-new transport + control plane**; all the hard parts
//! (content-addressed store, recall/search, signed assertions, capability grants,
//! tamper-evident audit) are reused verbatim from the `mem-*` crates. The gateway
//! adds three things the library layers deliberately don't have:
//!
//!   1. an **HTTP surface** (axum) the Memrizz webapp's BFF calls server-to-server;
//!   2. an **Org / membership control plane** (`control.json`) — Org isolation is
//!      physical (one store per Org), never a shared store behind a query filter;
//!   3. **OIDC verification** of the user's id_token (the gateway is the trust
//!      boundary; it re-verifies iss + aud + exp, RS256, against the JWKS).
//!
//! See `handoffs/MEMRIZZ_BACKEND_DEPLOY_HANDOFF_2026-06-15.md` for the contract
//! and `PLANSET/05_SPRINTS_AND_WPS.md` (WP-2.1: "MCP transport over the policy
//! boundary") for where this sits in the plan.
//!
//! The library (control model, scene projection, grant minting) builds without
//! the `server` feature so it stays unit-testable; the axum server and OIDC
//! verifier live behind `#[cfg(feature = "server")]`.

pub mod auth;
pub mod control;
pub mod scene;

#[cfg(feature = "server")]
pub mod http;

/// Wall-clock epoch milliseconds. A backwards clock yields 0 rather than panicking.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
