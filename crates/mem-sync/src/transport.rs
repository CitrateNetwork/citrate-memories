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
//! **FWA-C10-03 / PBA-L6b-019 (auth — per peer, fail closed).** The listener no
//! longer authorizes every TCP connection with one static operator grant (that
//! made any peer that could reach the port a full-authority client). Each
//! request must carry its OWN credential — a [`CapabilityGrant`] issued by the
//! trust root, sent as `Authorization: Bearer <hex(json(grant))>` (see
//! [`encode_credential`]). [`serve_one`] verifies it (signature, issuer ==
//! `trust_root`, not revoked, not expired) and authorizes that request with THAT
//! grant: `GET /bundle/<repo>` needs Read on the tenant, `POST /merge` goes
//! through `merge_bundle`, which needs Write on every repo it touches. A request
//! without a valid credential gets 401. The grant is a bearer credential: keep
//! TLS (or the loopback/relay posture) at the edge so it cannot be sniffed.
//!
//! R2 verifier follow-ups (PBA-L6b-019), all enforced in [`serve_one`] via
//! [`PeerAuth`]:
//! - **Audience.** A credential is honoured only by the peer it was issued for:
//!   the grant must carry the exact scope [`audience_resource`]`(peer_id)` (a
//!   `*` wildcard does not count), so a peer that received it cannot replay it
//!   to another peer sharing the same trust root.
//! - **Revocation.** `revoked` is now covered by the grant signature when set
//!   (mem-authz), and the server consults a deny list of grant ids
//!   ([`PeerAuth::revoked_ids`]) for grants revoked after issuance.
//! - **Clock.** Expiry is checked against the system clock at request time, not
//!   a caller-supplied timestamp that a long-running serve loop would freeze.
//! - **Shipping.** No binary may enable the `http` feature until mTLS/peer
//!   identity lands — `tests::tripwire_no_binary_enables_the_sync_http_feature`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use mem_authz::{CapabilityGrant, Op};
use mem_core::MemoryNode;
use mem_store::MemoryDagStore;

use crate::{export_tenant, merge_bundle, MergeOutcome, SyncBundle, SyncError};

/// Cap on an inbound request body — a pushed bundle larger than this is refused
/// before allocation (defence-in-depth; the merge itself is also bounded by the
/// store). 64 MiB comfortably holds the whole federation graph as JSON.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// PBA-L6b-019: cap on the request line + headers (a peer cannot stream an
/// endless header line into memory).
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Encode a grant as the transport's bearer credential (`hex(json(grant))`).
pub fn encode_credential(grant: &CapabilityGrant) -> Result<String, SyncError> {
    serde_json::to_vec(grant).map(hex::encode).map_err(|e| SyncError::Serde(e.to_string()))
}

/// How a serving peer authenticates the credentials presented to it.
pub struct PeerAuth<'a> {
    /// The ed25519 public key that legitimately issues grants.
    pub trust_root: &'a [u8],
    /// This peer's id; a credential must carry `audience_resource(audience)`.
    pub audience: &'a str,
    /// Grant ids revoked after issuance (the holder keeps its signed copy, so
    /// revocation must be a server-side decision).
    pub revoked_ids: &'a std::collections::HashSet<String>,
}

impl<'a> PeerAuth<'a> {
    /// Build a server's credential policy. Refuses an empty (or whitespace)
    /// audience at startup: it would match a bare `peer:` scope (pass-2 INFO).
    pub fn new(
        trust_root: &'a [u8],
        audience: &'a str,
        revoked_ids: &'a std::collections::HashSet<String>,
    ) -> Result<Self, SyncError> {
        if audience.trim().is_empty() {
            return Err(SyncError::Chain("PeerAuth: audience (this peer's id) must not be empty".into()));
        }
        Ok(PeerAuth { trust_root, audience, revoked_ids })
    }
}

/// The scope a grant must carry to be honoured by peer `peer_id`.
pub fn audience_resource(peer_id: &str) -> String {
    format!("peer:{peer_id}")
}

fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Decode + verify a presented credential: well-formed, validly signed, issued by
/// the trust root, addressed to THIS peer, not revoked (flag or deny list), not
/// expired. `None` on any failure (fail closed).
fn verify_credential(header_value: &str, auth: &PeerAuth<'_>, now_ms: u64) -> Option<CapabilityGrant> {
    let raw = header_value.trim();
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?.trim();
    let bytes = hex::decode(token).ok()?;
    let grant: CapabilityGrant = serde_json::from_slice(&bytes).ok()?;
    if auth.audience.trim().is_empty() {
        return None; // misconfigured server: never match a bare `peer:` scope
    }
    let aud = audience_resource(auth.audience);
    let addressed_here = grant.allowed_resources.iter().any(|r| r.resource_id == aud && r.can_read);
    if grant.verify_signature().is_err()
        || grant.issuer_pubkey.as_slice() != auth.trust_root
        || grant.revoked
        || auth.revoked_ids.contains(&grant.id)
        || !addressed_here
        || grant.expires_at_ms <= now_ms
    {
        return None;
    }
    Some(grant)
}

fn io_err(e: std::io::Error) -> SyncError {
    SyncError::Chain(format!("transport io: {e}"))
}

// ---- client ----

/// Pull a tenant's bundle from a peer and return it (does not merge — caller
/// decides). `base_url` is the peer's root, e.g. `http://10.0.0.5:8088`.
/// `credential` is the grant this client presents to the peer (PBA-L6b-019).
pub fn pull_bundle(base_url: &str, repo: &str, credential: &CapabilityGrant) -> Result<SyncBundle, SyncError> {
    let url = format!("{}/bundle/{repo}", base_url.trim_end_matches('/'));
    let body = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .set("authorization", &format!("Bearer {}", encode_credential(credential)?))
        .call()
        .map_err(|e| SyncError::Chain(format!("pull {url} failed: {e}")))?
        .into_string()
        .map_err(|e| SyncError::Chain(format!("pull {url} body: {e}")))?;
    SyncBundle::from_json(&body)
}

/// Push a local bundle to a peer, which merges it and returns the outcome.
pub fn push_bundle(base_url: &str, bundle: &SyncBundle, credential: &CapabilityGrant) -> Result<MergeOutcome, SyncError> {
    let url = format!("{}/merge", base_url.trim_end_matches('/'));
    let json = bundle.to_json()?;
    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(30))
        .set("authorization", &format!("Bearer {}", encode_credential(credential)?))
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
    credential: &CapabilityGrant,
    grant: &CapabilityGrant,
    trust_root: &[u8],
    now_ms: u64,
) -> Result<MergeOutcome, SyncError> {
    let bundle = pull_bundle(base_url, repo, credential)?;
    merge_bundle(store, &bundle, grant, trust_root, now_ms)
}

// ---- server ----

/// Serve exactly one connection, then return. The store stays in the caller's
/// thread (no `Send` bound needed). Returns a short label of the route handled,
/// or `None` if the connection carried no parseable request. Loop over this for
/// a long-running server: `loop { serve_one(&listener, &store, &auth)?; }`.
///
/// FWA-C10-03 + MEM-B-004 + PBA-L6b-019: there is no server-side grant. Each
/// request is authorized with the credential the PEER presents, checked against
/// `auth` (trust root, audience, deny list) and the system clock at request time.
/// No credential, or an invalid/expired/revoked/foreign one → 401, nothing served.
pub fn serve_one(
    listener: &TcpListener,
    store: &MemoryDagStore<MemoryNode>,
    auth: &PeerAuth<'_>,
) -> Result<Option<String>, SyncError> {
    let (stream, _peer) = listener.accept().map_err(io_err)?;
    handle_conn(stream, store, auth)
}

/// MEM-B-016: a stalled peer must not pin the single-threaded listener forever.
/// Bounds the time spent blocked on any one read/write of a connection.
const CONN_TIMEOUT: Duration = Duration::from_secs(30);

fn handle_conn(
    mut stream: TcpStream,
    store: &MemoryDagStore<MemoryNode>,
    auth: &PeerAuth<'_>,
) -> Result<Option<String>, SyncError> {
    // MEM-B-016: without timeouts, one `curl` that opens a connection and stalls
    // (or lies about Content-Length and never sends the body) blocks `serve_one`
    // in `read_exact` indefinitely — a trivial unauthenticated DoS on a listener
    // that serves one connection at a time. The timeouts apply to the underlying
    // socket, so the cloned `reader` below is bounded too.
    stream.set_read_timeout(Some(CONN_TIMEOUT)).map_err(io_err)?;
    stream.set_write_timeout(Some(CONN_TIMEOUT)).map_err(io_err)?;
    let mut reader = BufReader::new(stream.try_clone().map_err(io_err)?);

    // Request line + headers, bounded in total (PBA-L6b-019).
    let mut budget = MAX_HEADER_BYTES;
    let mut read_bounded = |reader: &mut BufReader<TcpStream>, line: &mut String| -> Result<Option<usize>, SyncError> {
        let n = reader.by_ref().take(budget as u64).read_line(line).map_err(io_err)?;
        budget = budget.saturating_sub(n);
        if n > 0 && !line.ends_with('\n') && budget == 0 {
            return Ok(None); // header section too large
        }
        Ok(Some(n))
    };
    let mut request_line = String::new();
    match read_bounded(&mut reader, &mut request_line)? {
        Some(0) => return Ok(None),
        Some(_) => {}
        None => {
            write_json(&mut stream, 431, "Request Header Fields Too Large", "{\"error\":\"headers too large\"}")?;
            return Ok(Some("(rejected: headers too large)".into()));
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // Headers: Content-Length and the peer credential.
    let mut content_length = 0usize;
    let mut credential: Option<String> = None;
    loop {
        let mut line = String::new();
        match read_bounded(&mut reader, &mut line)? {
            Some(0) => break,
            Some(_) => {}
            None => {
                write_json(&mut stream, 431, "Request Header Fields Too Large", "{\"error\":\"headers too large\"}")?;
                return Ok(Some(format!("{method} {path} (rejected: headers too large)")));
            }
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        } else if lower.starts_with("authorization:") {
            credential = Some(trimmed["authorization:".len()..].trim().to_string());
        }
    }

    // PBA-L6b-019: authenticate THIS peer before routing anything.
    // Expiry is judged by the system clock NOW, not a caller-supplied timestamp.
    let now_ms = system_now_ms();
    let Some(grant) = credential.as_deref().and_then(|c| verify_credential(c, auth, now_ms)) else {
        write_json(&mut stream, 401, "Unauthorized", "{\"error\":\"missing or invalid peer credential\"}")?;
        return Ok(Some(format!("{method} {path} (401)")));
    };
    let grant = &grant;

    match (method.as_str(), path.as_str()) {
        ("GET", p) if p.starts_with("/bundle/") => {
            let repo = p.trim_start_matches("/bundle/");
            // MEM-B-016: `/bundle/<repo>` exports a whole tenant; require the peer's
            // grant to authorize a READ of it, so this route cannot exfiltrate a
            // tenant with no authorization (the merge path already gates writes).
            if grant.check(&crate::resource_for(repo), Op::Read, now_ms).is_err() {
                write_json(&mut stream, 403, "Forbidden", "{\"error\":\"not authorized to read tenant\"}")?;
                return Ok(Some(format!("GET /bundle/{repo} (403)")));
            }
            let bundle = export_tenant(store, repo, now_ms)?;
            write_json(&mut stream, 200, "OK", &bundle.to_json()?)?;
            Ok(Some(format!("GET /bundle/{repo}")))
        }
        ("POST", "/merge") => {
            if content_length > MAX_BODY_BYTES {
                write_json(&mut stream, 413, "Payload Too Large", "{\"error\":\"bundle too large\"}")?;
                return Ok(Some("POST /merge (rejected: too large)".into()));
            }
            // MEM-B-016: do NOT pre-allocate `content_length` bytes — a lying
            // header would let a peer reserve 64 MiB per connection. Stream up to
            // the (already length-checked) bound instead, so memory grows only with
            // bytes actually received and a stall trips the read timeout above.
            let mut body = Vec::new();
            reader
                .by_ref()
                .take(content_length as u64)
                .read_to_end(&mut body)
                .map_err(io_err)?;
            let text = String::from_utf8(body).map_err(|e| SyncError::Chain(format!("merge body utf8: {e}")))?;
            match SyncBundle::from_json(&text).and_then(|b| merge_bundle(store, &b, grant, auth.trust_root, now_ms)) {
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
    use std::collections::HashSet;

    const PEER_ID: &str = "peer-C";

    /// A trust-root-signed credential scoped to `repos` AND addressed to PEER_ID.
    fn peer_grant(repos: &[&str], write: bool) -> CapabilityGrant {
        let mut g = crate::tests::grant_for(repos, write);
        g.allowed_resources.push(mem_authz::ResourceScope {
            resource_id: audience_resource(PEER_ID),
            can_read: true,
            can_write: false,
        });
        g.sign_with(&ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]));
        g
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    fn auth<'a>(root: &'a [u8], revoked: &'a HashSet<String>) -> PeerAuth<'a> {
        PeerAuth { trust_root: root, audience: PEER_ID, revoked_ids: revoked }
    }

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
        let grant = peer_grant(&["citrate-chain"], true);
        let root = crate::tests::trust_root();
        let cred = grant.clone();
        let client = std::thread::spawn(move || push_bundle(&base, &bundle, &cred));
        let route = serve_one(&listener, &peer, &auth(&root, &none())).expect("serve").unwrap();
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

        // GET /bundle is a read/export — MEM-B-016 now requires the grant to
        // authorize a READ of the tenant (a read-only grant suffices).
        let grant = peer_grant(&["citrate-chain"], false);
        let root = crate::tests::trust_root();
        let cred = grant.clone();
        let client = std::thread::spawn(move || pull_bundle(&base, "citrate-chain", &cred));
        let route = serve_one(&listener, &peer, &auth(&root, &none())).expect("serve").unwrap();
        let bundle = client.join().unwrap().expect("pull ok");

        assert_eq!(route, "GET /bundle/citrate-chain");
        assert_eq!(bundle.nodes.len(), 1);
        assert_eq!(bundle.repo, "citrate-chain");
    }

    /// MEM-B-016: a lying `Content-Length` that promises 64 MiB but sends nothing
    /// must NOT pre-allocate or hang `serve_one` — the body is streamed with a
    /// bounded `take`, so an early EOF returns promptly (a rejected merge) instead
    /// of blocking forever waiting on 64 MiB that never arrives.
    #[test]
    fn lying_content_length_does_not_hang_or_preallocate() {
        use std::io::Write as _;
        use std::time::Instant;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let grant = peer_grant(&["citrate-chain"], true);
        let root = crate::tests::trust_root();

        // Client: claim a huge body, send no bytes, then close the connection.
        let cred = encode_credential(&grant).unwrap();
        std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(format!("POST /merge HTTP/1.1\r\nAuthorization: Bearer {cred}\r\nContent-Length: 67108864\r\n\r\n").as_bytes()).unwrap();
            // Drop `s` → EOF, without ever sending the promised 64 MiB.
        });

        let start = Instant::now();
        let route = serve_one(&listener, &peer, &auth(&root, &none())).expect("serve returns");
        assert!(
            start.elapsed() < CONN_TIMEOUT,
            "serve_one must return promptly on early EOF, not block on the lying length"
        );
        assert_eq!(route.as_deref(), Some("POST /merge (rejected)"));
        assert_eq!(peer.node_count().unwrap(), 0, "no bundle merged");
    }

    /// MEM-B-016: `GET /bundle/<repo>` must refuse a peer whose grant cannot READ
    /// that tenant, rather than exporting the whole tenant with no authorization.
    #[test]
    fn bundle_export_requires_read_authorization() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("secret-tenant", "x")], &[]).unwrap();

        // Grant covers a DIFFERENT tenant — no read on `secret-tenant`.
        let grant = peer_grant(&["other-tenant"], false);
        let root = crate::tests::trust_root();
        let cred = grant.clone();
        let client = std::thread::spawn(move || pull_bundle(&base, "secret-tenant", &cred));
        let route = serve_one(&listener, &peer, &auth(&root, &none())).expect("serve").unwrap();
        let pulled = client.join().unwrap();

        assert_eq!(route, "GET /bundle/secret-tenant (403)");
        assert!(pulled.is_err(), "unauthorized pull must not return a bundle");
    }

    /// Send a raw HTTP request to `addr` and return the full response text.
    fn raw_request(addr: std::net::SocketAddr, req: String) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            // Half-close so the server sees EOF after the request bytes.
            let _ = s.shutdown(std::net::Shutdown::Write);
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        })
    }

    /// PBA-L6b-019 (CIT-MEM-02): an UNAUTHENTICATED TCP peer — no credential at
    /// all — must not be served under a static operator grant. Before the fix the
    /// listener authorized every connection with the grant handed to `serve_one`.
    #[test]
    fn pba_l6b_019_unauthenticated_peer_cannot_pull_or_push() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "private tip")], &[]).unwrap();
        let root = crate::tests::trust_root();

        let c = raw_request(addr, "GET /bundle/citrate-chain HTTP/1.1\r\n\r\n".into());
        serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
        let resp = c.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 401"), "PBA-L6b-019: unauthenticated pull served: {resp}");
        assert!(!resp.contains("private tip"), "PBA-L6b-019: tenant content leaked");

        let bundle = SyncBundle { repo: "citrate-chain".into(), exported_at_ms: 1, nodes: vec![derived_node("citrate-chain", "injected")], edges: vec![] }.to_json().unwrap();
        let c = raw_request(addr, format!("POST /merge HTTP/1.1\r\nContent-Length: {}\r\n\r\n{bundle}", bundle.len()));
        serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
        let resp = c.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 401"), "PBA-L6b-019: unauthenticated push served: {resp}");
        assert_eq!(peer.node_count().unwrap(), 1, "nothing merged");
    }

    /// PBA-L6b-019: a credential that is self-signed (not the trust root),
    /// expired, revoked, or garbage is refused; the trust-root grant is served.
    #[test]
    fn pba_l6b_019_only_trust_root_credentials_are_accepted() {
        use ed25519_dalek::SigningKey;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "private tip")], &[]).unwrap();
        let root = crate::tests::trust_root();
        let good = peer_grant(&["citrate-chain"], false);
        let mut forged = good.clone();
        forged.sign_with(&SigningKey::from_bytes(&[7u8; 32]));
        let mut expired = good.clone();
        expired.expires_at_ms = 1;
        expired.sign_with(&SigningKey::from_bytes(&[42u8; 32]));
        let mut revoked = good.clone();
        revoked.revoked = true;
        let enc = |g: &CapabilityGrant| encode_credential(g).unwrap();
        let get = |auth: String| format!("GET /bundle/citrate-chain HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n");

        for (label, hdr) in [
            ("self-signed", format!("Bearer {}", enc(&forged))),
            ("expired", format!("Bearer {}", enc(&expired))),
            ("revoked", format!("Bearer {}", enc(&revoked))),
            ("garbage", "Bearer zz-not-hex".to_string()),
            ("no scheme", enc(&good)),
        ] {
            let c = raw_request(addr, get(hdr));
            serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
            let resp = c.join().unwrap();
            assert!(resp.starts_with("HTTP/1.1 401"), "PBA-L6b-019: {label} credential served: {resp}");
        }
        let c = raw_request(addr, get(format!("Bearer {}", enc(&good))));
        serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
        let resp = c.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 200") && resp.contains("private tip"), "valid credential served: {resp}");
    }

    /// PBA-L6b-019 variant: an endless header line is cut off (431), not buffered.
    #[test]
    fn pba_l6b_019_oversized_headers_are_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let root = crate::tests::trust_root();
        let c = raw_request(addr, format!("GET /bundle/x HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(MAX_HEADER_BYTES * 2)));
        let route = serve_one(&listener, &peer, &auth(&root, &none())).expect("serve").unwrap();
        let resp = c.join().unwrap();
        assert!(route.contains("headers too large"), "{route}");
        assert!(resp.starts_with("HTTP/1.1 431"), "{resp}");
    }

    /// PBA-L6b-019 mutation-hardening: a header section that ends exactly at the
    /// byte budget (then EOF) is not "too large", a final line without a newline
    /// is a normal (unauthenticated → 401) request, and an unknown GET route with
    /// a valid credential is 404, not routed to the bundle export.
    #[test]
    fn pba_l6b_019_header_budget_edges_and_routing() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let root = crate::tests::trust_root();

        // Exactly MAX_HEADER_BYTES of complete lines, then EOF (no blank line).
        let head = "GET /bundle/x HTTP/1.1\r\n";
        let pad_len = MAX_HEADER_BYTES - head.len() - "X-Pad: \r\n".len();
        let exact = format!("{head}X-Pad: {}\r\n", "a".repeat(pad_len));
        assert_eq!(exact.len(), MAX_HEADER_BYTES);
        let c = raw_request(addr, exact);
        serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
        assert!(c.join().unwrap().starts_with("HTTP/1.1 401"), "exact-budget headers are not 431");

        // Request line with no trailing newline, then EOF.
        let c = raw_request(addr, "GET /bundle/x HTTP/1.1".into());
        serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
        assert!(c.join().unwrap().starts_with("HTTP/1.1 401"), "unterminated last line is not 431");

        // Unknown route, valid credential → 404.
        let cred = encode_credential(&peer_grant(&["x"], false)).unwrap();
        let c = raw_request(addr, format!("GET /nope HTTP/1.1\r\nAuthorization: Bearer {cred}\r\n\r\n"));
        serve_one(&listener, &peer, &auth(&root, &none())).expect("serve");
        assert!(c.join().unwrap().starts_with("HTTP/1.1 404"), "unknown route is 404");
    }

    /// PBA-L6b-019 follow-up (verifier probe
    /// `v_l6b019_revoked_flag_is_unsigned_and_grant_replays_across_peers`):
    ///   * a grant signed as revoked cannot be un-revoked by flipping the flag;
    ///   * a grant issued for peer B is refused by peer C (audience binding).
    #[test]
    fn pba_l6b_019_revoked_flip_and_cross_peer_replay_are_refused() {
        let root = crate::tests::trust_root();
        let sk = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let mut revoked = peer_grant(&["citrate-chain"], false);
        revoked.revoked = true;
        revoked.sign_with(&sk);
        let mut flipped = revoked.clone();
        flipped.revoked = false;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "peer-C private")], &[]).unwrap();
        let get = |g: &CapabilityGrant| format!("GET /bundle/citrate-chain HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n", encode_credential(g).unwrap());
        let c = raw_request(addr, get(&flipped));
        serve_one(&listener, &peer, &auth(&root, &none())).unwrap();
        let resp = c.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 401"), "PBA-L6b-019: flipped-revoked grant served: {resp}");

        // A valid grant scoped to peer-B (audience) replayed to this peer (peer-C).
        let mut for_b = crate::tests::grant_for(&["citrate-chain"], false);
        for_b.allowed_resources.push(mem_authz::ResourceScope { resource_id: "peer:peer-B".into(), can_read: true, can_write: false });
        for_b.sign_with(&sk);
        let c = raw_request(addr, get(&for_b));
        serve_one(&listener, &peer, &auth(&root, &none())).unwrap();
        let resp = c.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 401"), "PBA-L6b-019: grant for peer-B replayed to peer-C: {resp}");
    }

    /// PBA-L6b-019 follow-up: a grant revoked AFTER issuance (the holder keeps
    /// its valid signed copy) is refused once its id is on the peer's deny list.
    #[test]
    fn pba_l6b_019_deny_listed_grant_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "private tip")], &[]).unwrap();
        let root = crate::tests::trust_root();
        let g = peer_grant(&["citrate-chain"], false);
        let get = format!("GET /bundle/citrate-chain HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n", encode_credential(&g).unwrap());
        let c = raw_request(addr, get.clone());
        serve_one(&listener, &peer, &auth(&root, &none())).unwrap();
        assert!(c.join().unwrap().starts_with("HTTP/1.1 200"), "control: valid grant served");
        let deny: HashSet<String> = [g.id.clone()].into();
        let c = raw_request(addr, get);
        serve_one(&listener, &peer, &auth(&root, &deny)).unwrap();
        assert!(c.join().unwrap().starts_with("HTTP/1.1 401"), "PBA-L6b-019: deny-listed grant served");
    }

    /// PBA-L6b-019 follow-up: expiry uses the system clock — a grant that expired
    /// in the past is refused (there is no caller clock to freeze any more), and
    /// a wildcard grant is not an audience binding.
    #[test]
    fn pba_l6b_019_system_clock_expiry_and_no_wildcard_audience() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        let root = crate::tests::trust_root();
        let sk = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let mut past = peer_grant(&["citrate-chain"], false);
        past.expires_at_ms = system_now_ms() - 1_000;
        past.sign_with(&sk);
        let mut wild = crate::tests::grant_for(&["*"], false);
        wild.sign_with(&sk);
        for (label, g) in [("expired by wall clock", &past), ("wildcard, no audience", &wild)] {
            let c = raw_request(addr, format!("GET /bundle/citrate-chain HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n", encode_credential(g).unwrap()));
            serve_one(&listener, &peer, &auth(&root, &none())).unwrap();
            assert!(c.join().unwrap().starts_with("HTTP/1.1 401"), "PBA-L6b-019: {label} served");
        }
    }

    /// The audience scope is part of the wire contract issuers mint against.
    #[test]
    fn audience_resource_wire_format() {
        assert_eq!(audience_resource("peer-C"), "peer:peer-C");
        assert_ne!(audience_resource("a"), audience_resource("b"));
    }

    /// Pass-2: a grant that names this peer's audience scope but with
    /// can_read=false is refused (kills the surviving `&& r.can_read` mutant).
    #[test]
    fn pba_l6b_019_p2_audience_scope_without_read_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "private")], &[]).unwrap();
        let root = crate::tests::trust_root();
        let mut g = crate::tests::grant_for(&["citrate-chain"], false);
        g.allowed_resources.push(mem_authz::ResourceScope { resource_id: audience_resource(PEER_ID), can_read: false, can_write: true });
        g.sign_with(&ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]));
        let c = raw_request(addr, format!("GET /bundle/citrate-chain HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n", encode_credential(&g).unwrap()));
        serve_one(&listener, &peer, &auth(&root, &none())).unwrap();
        assert!(c.join().unwrap().starts_with("HTTP/1.1 401"));
    }

    /// Pass-2 (INFO): an empty server audience would match a `peer:` scope — it is
    /// refused both at request time and by the PeerAuth constructor.
    #[test]
    fn pba_l6b_019_p2_empty_audience_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let peer = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        peer.commit(&[derived_node("citrate-chain", "private")], &[]).unwrap();
        let root = crate::tests::trust_root();
        let mut g = crate::tests::grant_for(&["citrate-chain"], false);
        g.allowed_resources.push(mem_authz::ResourceScope { resource_id: "peer:".into(), can_read: true, can_write: false });
        g.sign_with(&ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]));
        let empty = none();
        let bad = PeerAuth { trust_root: &root, audience: "", revoked_ids: &empty };
        let c = raw_request(addr, format!("GET /bundle/citrate-chain HTTP/1.1\r\nAuthorization: Bearer {}\r\n\r\n", encode_credential(&g).unwrap()));
        serve_one(&listener, &peer, &bad).unwrap();
        assert!(c.join().unwrap().starts_with("HTTP/1.1 401"), "PBA-L6b-019 p2: empty audience matched peer:");
    }

    #[test]
    fn pba_l6b_019_p2_peer_auth_constructor_rejects_empty_audience() {
        let root = crate::tests::trust_root();
        let r = none();
        assert!(PeerAuth::new(&root, "", &r).is_err());
        assert!(PeerAuth::new(&root, "  ", &r).is_err());
        let ok = PeerAuth::new(&root, PEER_ID, &r).expect("valid");
        assert_eq!(ok.audience, PEER_ID);
    }
}
