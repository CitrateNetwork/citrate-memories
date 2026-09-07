//! WP-5.4 — minimal HTTP bundle transport.
//!
//! [`SyncBundle`](crate::SyncBundle) is already a transport-agnostic JSON blob,
//! so federation sync works today over file/scp. This module removes the
//! operator file-move: a peer can **pull** a tenant's bundle or **push** its own
//! into another replica over plain HTTP/1.1.
//!
//! Two routes, kept deliberately tiny (the server is `std::net` only — no async
//! runtime, no web framework):
//! - `GET  /bundle/<repo>` → the peer exports that tenant and returns the bundle.
//! - `POST /merge`         → the body is a bundle; the peer merges it (the CRDT
//!   join, [`merge_bundle`](crate::merge_bundle)) and returns the [`MergeOutcome`].
//!
//! The merge side keeps every safety property of `merge_bundle` — Asserted-plane
//! items must verify, supersessions are cycle-guarded, contradictions surface as
//! Belnap `Both`. **Gossip / peer-discovery is deferred** (a dated note in MEM-S5);
//! this is point-to-point pull/push, which is enough to retire file-moves.
//!
//! **FWA-C10-03 (auth — fail closed by construction).** `merge_bundle` now
//! requires a [`CapabilityGrant`], and so does this transport: [`serve_one`] takes
//! the grant the connecting peer is authorized under and passes it to
//! `merge_bundle`, which authorizes a `Write` to every repo the pushed bundle
//! touches. The transport therefore CANNOT be wired into a binary without an
//! explicit authorization decision — the type system enforces it. (A real
//! deployment binds the grant to the authenticated peer — mTLS / connect-token —
//! rather than a static operator grant; that binding is the operator/v2 step.
//! TLS is still expected at the edge; keep the loopback/relay posture until it
//! lands.)

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use mem_authz::CapabilityGrant;
use mem_core::MemoryNode;
use mem_store::MemoryDagStore;

use crate::{export_tenant, merge_bundle, MergeOutcome, SyncBundle, SyncError};

/// Cap on an inbound request body — a pushed bundle larger than this is refused
/// before allocation (defence-in-depth; the merge itself is also bounded by the
/// store). 64 MiB comfortably holds the whole federation graph as JSON.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

fn io_err(e: std::io::Error) -> SyncError {
    SyncError::Chain(format!("transport io: {e}"))
}

// ---- client ----

/// Pull a tenant's bundle from a peer and return it (does not merge — caller
/// decides). `base_url` is the peer's root, e.g. `http://10.0.0.5:8088`.
pub fn pull_bundle(base_url: &str, repo: &str) -> Result<SyncBundle, SyncError> {
    let url = format!("{}/bundle/{repo}", base_url.trim_end_matches('/'));
    let body = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| SyncError::Chain(format!("pull {url} failed: {e}")))?
        .into_string()
        .map_err(|e| SyncError::Chain(format!("pull {url} body: {e}")))?;
    SyncBundle::from_json(&body)
}

/// Push a local bundle to a peer, which merges it and returns the outcome.
pub fn push_bundle(base_url: &str, bundle: &SyncBundle) -> Result<MergeOutcome, SyncError> {
    let url = format!("{}/merge", base_url.trim_end_matches('/'));
    let json = bundle.to_json()?;
    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(30))
        .set("content-type", "application/json")
        .send_string(&json)
        .map_err(|e| SyncError::Chain(format!("push {url} failed: {e}")))?
        .into_string()
        .map_err(|e| SyncError::Chain(format!("push {url} body: {e}")))?;
    parse_outcome(&resp)
}

/// Pull from a peer AND merge into the local store in one call. The pulled
/// bundle is untrusted (it came off the network), so the caller must supply the
/// `grant` the merge is authorized under and `now_ms` for expiry checks
/// (FWA-C10-01/02/03).
pub fn pull_and_merge(
    store: &MemoryDagStore<MemoryNode>,
    base_url: &str,
    repo: &str,
    grant: &CapabilityGrant,
    trust_root: &[u8],
    now_ms: u64,
) -> Result<MergeOutcome, SyncError> {
    let bundle = pull_bundle(base_url, repo)?;
    merge_bundle(store, &bundle, grant, trust_root, now_ms)
}

// ---- server ----

/// Serve exactly one connection, then return. The store stays in the caller's
/// thread (no `Send` bound needed). Returns a short label of the route handled,
/// or `None` if the connection carried no parseable request. Loop over this for
/// a long-running server: `loop { serve_one(&listener, &store, now)?; }`.
///
/// FWA-C10-03 + MEM-B-004: `grant` is the capability the connecting peer is
/// authorized under and `trust_root` is the ed25519 public key that legitimately
/// issues grants (the operator/gateway key). Both are handed to `merge_bundle`,
/// which requires the grant to be issued by `trust_root` (a self-signed grant is
/// refused) and to authorize a `Write` to every repo a pushed bundle touches.
/// There is no unauthenticated merge path — the signature requires a grant.
pub fn serve_one(
    listener: &TcpListener,
    store: &MemoryDagStore<MemoryNode>,
    grant: &CapabilityGrant,
    trust_root: &[u8],
    now_ms: u64,
) -> Result<Option<String>, SyncError> {
    let (stream, _peer) = listener.accept().map_err(io_err)?;
    handle_conn(stream, store, grant, trust_root, now_ms)
}

fn handle_conn(
    mut stream: TcpStream,
    store: &MemoryDagStore<MemoryNode>,
    grant: &CapabilityGrant,
    trust_root: &[u8],
    now_ms: u64,
) -> Result<Option<String>, SyncError> {
    let mut reader = BufReader::new(stream.try_clone().map_err(io_err)?);

    // Request line.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).map_err(io_err)? == 0 {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // Headers (we only need Content-Length).
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(io_err)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }

    match (method.as_str(), path.as_str()) {
        ("GET", p) if p.starts_with("/bundle/") => {
            let repo = p.trim_start_matches("/bundle/");
            let bundle = export_tenant(store, repo, now_ms)?;
            write_json(&mut stream, 200, "OK", &bundle.to_json()?)?;
            Ok(Some(format!("GET /bundle/{repo}")))
        }
        ("POST", "/merge") => {
            if content_length > MAX_BODY_BYTES {
                write_json(&mut stream, 413, "Payload Too Large", "{\"error\":\"bundle too large\"}")?;
                return Ok(Some("POST /merge (rejected: too large)".into()));
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).map_err(io_err)?;
            let text = String::from_utf8(body).map_err(|e| SyncError::Chain(format!("merge body utf8: {e}")))?;
            match SyncBundle::from_json(&text).and_then(|b| merge_bundle(store, &b, grant, trust_root, now_ms)) {
                Ok(outcome) => {
                    write_json(&mut stream, 200, "OK", &outcome_json(&outcome))?;
                    Ok(Some("POST /merge".into()))
                }
                Err(e) => {
                    write_json(&mut stream, 400, "Bad Request", &format!("{{\"error\":{:?}}}", e.to_string()))?;
                    Ok(Some("POST /merge (rejected)".into()))
                }
            }
        }
        _ => {
            write_json(&mut stream, 404, "Not Found", "{\"error\":\"no such route\"}")?;
            Ok(Some(format!("{method} {path} (404)")))
        }
    }
}

fn write_json(stream: &mut TcpStream, code: u16, reason: &str, body: &str) -> Result<(), SyncError> {
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len() // str::len() is the byte length — exactly Content-Length
    );
    stream.write_all(resp.as_bytes()).map_err(io_err)?;
    stream.flush().map_err(io_err)
}

// MergeOutcome isn't Serialize (it lives in lib with `Copy`-only derives), so we
// render + parse it explicitly here, keeping the wire shape stable + auditable.
fn outcome_json(o: &MergeOutcome) -> String {
    format!(
        "{{\"nodes_added\":{},\"nodes_merged\":{},\"edges_added\":{},\"edges_merged\":{},\"superseded\":{},\"rejected_supersessions\":{},\"rejected_signatures\":{},\"contradictions\":{}}}",
        o.nodes_added, o.nodes_merged, o.edges_added, o.edges_merged, o.superseded, o.rejected_supersessions, o.rejected_signatures, o.contradictions
    )
}

fn parse_outcome(s: &str) -> Result<MergeOutcome, SyncError> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| SyncError::Serde(e.to_string()))?;
    let g = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as usize;
    Ok(MergeOutcome {
        nodes_added: g("nodes_added"),
        nodes_merged: g("nodes_merged"),
        edges_added: g("edges_added"),
        edges_merged: g("edges_merged"),
        superseded: g("superseded"),
        rejected_supersessions: g("rejected_supersessions"),
        rejected_signatures: g("rejected_signatures"),
        contradictions: g("contradictions"),
    })
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use mem_core::{NodeKind, Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION};
    use mem_store::kv::InMemoryKv;

    fn derived_node(repo: &str, subject: &str) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Commit,
            repo: repo.into(),
            author: "t".into(),
            source_ref: SourceRef::GitCommit { repo: repo.into(), sha: subject.into() },
            content: subject.as_bytes().to_vec(),
            valid_from: 1,
            valid_to: None,
            observed_at: 1,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: None,
            confidence: vec![],
            anchors: vec![],
            status: Status::Active,
        }
    }

    /// End-to-end: a bundle pushed over HTTP merges into a peer with no file move.
    /// The server store stays in the main thread; the client runs in a spawned
    /// thread (so no `Send` bound on the store is needed).
    #[test]
    fn push_merges_into_a_peer_over_http() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");

        // Peer (server) store — empty.
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));

        // Source bundle to push.
        let bundle = SyncBundle {
            repo: "citrate-chain".into(),
            exported_at_ms: 1,
            nodes: vec![derived_node("citrate-chain", "ghostdag")],
            edges: vec![],
        };

        // The peer authorizes this connecting client to Write the pushed repo.
        let grant = crate::tests::grant_for(&["citrate-chain"], true);
        let root = crate::tests::trust_root();
        let client = std::thread::spawn(move || push_bundle(&base, &bundle));
        let route = serve_one(&listener, &peer, &grant, &root, 1).expect("serve").unwrap();
        let outcome = client.join().unwrap().expect("push ok");

        assert_eq!(route, "POST /merge");
        assert_eq!(outcome.nodes_added, 1);
        assert_eq!(peer.node_count().unwrap(), 1, "the pushed node landed in the peer with no file move");
    }

    /// End-to-end: a peer's tenant bundle can be pulled over HTTP and merged.
    #[test]
    fn pull_fetches_a_tenant_bundle_over_http() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");

        // Peer (server) store has one node to serve.
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "tip-selection")], &[]).unwrap();

        // GET /bundle is a read/export — the grant is unused on this route, but
        // serve_one still requires one (no unauthenticated merge path can exist).
        let grant = crate::tests::grant_for(&["citrate-chain"], false);
        let root = crate::tests::trust_root();
        let client = std::thread::spawn(move || pull_bundle(&base, "citrate-chain"));
        let route = serve_one(&listener, &peer, &grant, &root, 1).expect("serve").unwrap();
        let bundle = client.join().unwrap().expect("pull ok");

        assert_eq!(route, "GET /bundle/citrate-chain");
        assert_eq!(bundle.nodes.len(), 1);
        assert_eq!(bundle.repo, "citrate-chain");
    }
}
