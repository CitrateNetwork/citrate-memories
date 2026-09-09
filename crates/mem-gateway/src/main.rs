//! `mem-gateway` binary — the Single-Org production runner.
//!
//! Build: `cargo build -p mem-gateway --bin mem-gateway --release --features server,rocksdb,transformer`
//!
//! See `handoffs/MEMRIZZ_BACKEND_DEPLOY_HANDOFF_2026-06-15.md` for the full
//! deploy contract (env vars, JWKS, Org-owner bootstrap, Caddy, verification).

use std::sync::{Arc, Mutex, RwLock};

use mem_authz::AuditChain;
use mem_core::MemoryNode;
use mem_gateway::auth::signing_key_from_seed;
use mem_gateway::control::{Control, Role};
use mem_gateway::http::{run, AppState};
use mem_gateway::now_ms;
use mem_index::Embedder;
use mem_store::MemoryDagStore;

const DEFAULT_ORG: &str = "citrate-federation";
const DEFAULT_STORE: &str = "./data/federation.bge.enc.memdag";
const DEFAULT_BIND: &str = "127.0.0.1:8799";

/// MEM-B-006: the asserter seed derives the gateway's ed25519 key, which signs
/// every capability grant AND every per-principal assertion sub-key. A constant
/// default is therefore a *published signing key* — anyone reading the (OSS)
/// source could forge signed Asserted-plane nodes attributed to any principal. So
/// the gateway REFUSES to start when `MEM_GATEWAY_ASSERTER_SEED` is unset, exactly
/// as it already does for a rejected audit chain or a missing JWKS — never fail
/// open. There is deliberately no in-tree default value.
fn resolve_asserter_seed(env_val: Option<String>) -> Result<String, &'static str> {
    env_val.ok_or(
        "MEM_GATEWAY_ASSERTER_SEED unset — refusing to start. It derives the key that signs \
         every grant and per-principal assertion; a default would be a published signing key. \
         Set it in the operator EnvironmentFile (/etc/mem-gateway/mem-gateway.env).",
    )
}

/// MEM-B-013: guard the dev-auth cliff. When OIDC is off and
/// `MEM_GATEWAY_ALLOW_DEV_AUTH=1`, the gateway trusts the unauthenticated
/// `x-dev-sub` impersonation header — anyone with network reach can read/write as
/// any member. That is a development-only posture. To make it impossible to reach
/// by casually flipping one env var to "fix" a 401 in production, dev-auth also
/// requires an explicit non-production acknowledgement
/// (`MEM_GATEWAY_DEV_AUTH_ACK=1`). Missing it → refuse to start.
fn dev_auth_permitted(oidc_on: bool, allow_dev_auth: bool, ack: Option<&str>) -> Result<(), String> {
    if oidc_on || !allow_dev_auth {
        return Ok(());
    }
    if ack == Some("1") {
        Ok(())
    } else {
        Err("MEM_GATEWAY_ALLOW_DEV_AUTH=1 trusts the unauthenticated x-dev-sub header \
             (impersonate any member). Refusing to start: this is DEVELOPMENT ONLY. \
             Set MEM_GATEWAY_DEV_AUTH_ACK=1 to acknowledge a non-production deployment, \
             or configure OIDC_* for real authentication."
            .to_string())
    }
}

struct Args {
    org: String,
    store: String,
    bind: String,
    bootstrap_owner: Option<String>,
}

fn parse_args() -> Args {
    // Defaults are env-configurable so a deployment need not hardcode the org or
    // store path on the CLI; an explicit `--org`/`--store` flag still wins below.
    let mut org = std::env::var("MEM_DEFAULT_ORG").unwrap_or_else(|_| DEFAULT_ORG.to_string());
    let mut store =
        std::env::var("MEM_DEFAULT_STORE").unwrap_or_else(|_| DEFAULT_STORE.to_string());
    let mut bind = DEFAULT_BIND.to_string();
    let mut bootstrap_owner = None;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let take = |i: &mut usize| -> Option<String> {
            *i += 1;
            argv.get(*i).cloned()
        };
        match argv[i].as_str() {
            "--org" => org = take(&mut i).unwrap_or(org.clone()),
            "--store" => store = take(&mut i).unwrap_or(store.clone()),
            "--bind" => bind = take(&mut i).unwrap_or(bind.clone()),
            "--bootstrap-owner" => bootstrap_owner = take(&mut i),
            "-h" | "--help" => {
                eprintln!(
                    "mem-gateway --org <id> --store <path> --bind <addr> [--bootstrap-owner <oidc-sub>]"
                );
                std::process::exit(0);
            }
            other => eprintln!("mem-gateway: ignoring unknown arg '{other}'"),
        }
        i += 1;
    }
    Args {
        org,
        store,
        bind,
        bootstrap_owner,
    }
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// Detect the store's embedding model and load the matching query embedder once,
/// shared across requests (mirrors the mcp_serve daemon). A hashing store → None.
#[cfg(feature = "transformer")]
fn load_query_embedder(store: &MemoryDagStore<MemoryNode>) -> Option<Arc<dyn Embedder>> {
    let model = mem_query::detect_store_embedding_model(store).ok().flatten()?;
    if model != mem_index::transformer::DEFAULT_MODEL_ID {
        eprintln!("mem-gateway: store model '{model}' has no matching local embedder; search disabled");
        return None;
    }
    eprintln!("mem-gateway: store embedded with '{model}', loading transformer embedder…");
    match mem_index::TransformerEmbedder::bge_base() {
        Ok(e) => Some(Arc::new(e) as Arc<dyn Embedder>),
        Err(e) => {
            eprintln!("mem-gateway: failed to load embedder: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "transformer"))]
fn load_query_embedder(_store: &MemoryDagStore<MemoryNode>) -> Option<Arc<dyn Embedder>> {
    None
}

fn fail(msg: &str) -> ! {
    eprintln!("mem-gateway: {msg}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = parse_args();

    // --- store (RocksDB single-writer lock taken here) ---
    let store = match MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&args.store) {
        Ok(s) => s,
        Err(e) => fail(&format!("cannot open store {} ({e})", args.store)),
    };
    let node_count = store.node_count().unwrap_or(0);
    let edge_count = store.edge_count().unwrap_or(0);
    let encrypted = store.is_encrypted_at_rest();
    let embedder = load_query_embedder(&store);

    // --- audit chain (fail closed: a chain we can't trust must not be extended) ---
    let audit_path = env_opt("MEM_GATEWAY_AUDIT_PATH")
        .unwrap_or_else(|| format!("{}.gateway-audit.jsonl", args.store));
    let audit = match AuditChain::open(&audit_path) {
        Ok(c) => {
            eprintln!(
                "mem-gateway: audit chain {audit_path} verified ({} records)",
                c.len()
            );
            Arc::new(Mutex::new(c))
        }
        Err(e) => fail(&format!("audit chain {audit_path} REJECTED: {e}")),
    };

    // --- control plane (orgs + memberships) ---
    let control_path =
        env_opt("MEM_GATEWAY_CONTROL_PATH").unwrap_or_else(|| "./data/control.json".to_string());
    let mut control = match Control::load(&control_path) {
        Ok(c) => c,
        Err(e) => fail(&format!("cannot load control {control_path} ({e})")),
    };
    let mut control_dirty = control.ensure_org(&args.org, &args.store, now_ms());
    if let Some(owner) = &args.bootstrap_owner {
        if control.upsert_membership(owner, &args.org, Role::OrgOwner) {
            control_dirty = true;
            eprintln!("mem-gateway: bootstrapped OrgOwner '{owner}' for org '{}'", args.org);
        }
    }
    if control_dirty {
        if let Err(e) = control.save(&control_path) {
            fail(&format!("cannot persist control {control_path} ({e})"));
        }
    }
    let owner_count = control
        .memberships
        .iter()
        .filter(|m| m.org == args.org && matches!(m.role, Role::OrgOwner))
        .count();

    // --- OIDC (production). If issuer+aud are set, a JWKS source is REQUIRED. ---
    let issuer = env_opt("OIDC_ISSUER");
    let audience = env_opt("OIDC_AUDIENCE");
    let oidc = match (issuer.as_ref(), audience.as_ref()) {
        (Some(iss), Some(aud)) => {
            let jwks = match (env_opt("OIDC_JWKS_FILE"), env_opt("OIDC_JWKS_JSON")) {
                (Some(path), _) => std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| fail(&format!("cannot read OIDC_JWKS_FILE {path} ({e})"))),
                (None, Some(inline)) => inline,
                (None, None) => fail(
                    "OIDC_ISSUER+OIDC_AUDIENCE set but no OIDC_JWKS_FILE/OIDC_JWKS_JSON — refusing to start (never fail open)",
                ),
            };
            match mem_gateway::auth::OidcVerifier::from_jwks(iss, aud, &jwks) {
                Ok(v) => Some(Arc::new(v)),
                Err(e) => fail(&format!("OIDC verifier init failed: {e}")),
            }
        }
        _ => None,
    };

    let allow_dev_auth = env_opt("MEM_GATEWAY_ALLOW_DEV_AUTH").as_deref() == Some("1");
    let connect_secret = env_opt("MEM_CONNECT_SECRET").map(Arc::new);
    // MEM-B-006: fail closed when the seed is unset — no insecure default.
    let seed = resolve_asserter_seed(env_opt("MEM_GATEWAY_ASSERTER_SEED")).unwrap_or_else(|m| fail(m));

    // --- startup summary + safety checks ---
    eprintln!("mem-gateway: org={} store={} ({node_count} nodes / {edge_count} edges, encrypted_at_rest={encrypted})", args.org, args.store);
    if oidc.is_some() {
        eprintln!("mem-gateway: AUTH = OIDC (fail-closed); dev-auth ignored");
        if allow_dev_auth {
            eprintln!("mem-gateway: note MEM_GATEWAY_ALLOW_DEV_AUTH=1 is ignored while OIDC is on");
        }
        if owner_count == 0 {
            eprintln!("mem-gateway: WARNING no OrgOwner membership for '{}' — every real user will get 403. Bootstrap with --bootstrap-owner <sub>.", args.org);
        }
    } else if allow_dev_auth {
        // MEM-B-013: refuse to start in a dev-auth posture without explicit ack.
        if let Err(m) = dev_auth_permitted(false, true, env_opt("MEM_GATEWAY_DEV_AUTH_ACK").as_deref()) {
            fail(&m);
        }
        eprintln!("mem-gateway: AUTH = dev-auth (x-dev-sub) — DEVELOPMENT ONLY (acknowledged)");
    } else {
        eprintln!("mem-gateway: AUTH = none configured — all Org routes will 401 (fail closed). Set OIDC_* or MEM_GATEWAY_ALLOW_DEV_AUTH=1.");
    }

    let state = AppState {
        store: Arc::new(store),
        embedder,
        index_cache: Arc::new(mem_query::TenantIndexCache::new()),
        write_gate: Arc::new(Mutex::new(())),
        audit,
        signing_key: signing_key_from_seed(&seed),
        control: Arc::new(RwLock::new(control)),
        org_id: Arc::new(args.org.clone()),
        store_path: Arc::new(args.store.clone()),
        // WP-5.3: bind external audit checkpoints to the live 40204 head. Defaults
        // to the public RPC; set MEM_GATEWAY_CHAIN_RPC to override, or "" to disable.
        chain_rpc: match env_opt("MEM_GATEWAY_CHAIN_RPC") {
            Some(u) => Some(Arc::new(u)),
            None => Some(Arc::new("https://rpc.citrate.ai".to_string())),
        },
        issuer: Arc::new(issuer.unwrap_or_else(|| "mem-gateway".to_string())),
        oidc,
        connect_secret,
        allow_dev_auth,
        layout_cache: Arc::new(Mutex::new(None)),
        ingest_queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
    };

    if let Err(e) = run(state, &args.bind).await {
        fail(&format!("server error: {e}"));
    }
}

#[cfg(test)]
mod seed_tests {
    use super::{dev_auth_permitted, resolve_asserter_seed};

    /// MEM-B-013 tripwire: dev-auth (x-dev-sub impersonation) must not be reachable
    /// by flipping one env var — it also requires an explicit non-prod ack.
    #[test]
    fn dev_auth_requires_explicit_ack() {
        // OIDC on → dev-auth is ignored, always fine.
        assert!(dev_auth_permitted(true, true, None).is_ok());
        // OIDC off, dev-auth off → fine (fail-closed 401 posture).
        assert!(dev_auth_permitted(false, false, None).is_ok());
        // OIDC off, dev-auth on, no ack → refuse to start.
        assert!(dev_auth_permitted(false, true, None).is_err());
        assert!(dev_auth_permitted(false, true, Some("0")).is_err());
        // Explicit acknowledgement → permitted.
        assert!(dev_auth_permitted(false, true, Some("1")).is_ok());
    }

    /// MEM-B-006 tripwire: with the seed unset the gateway must refuse to start
    /// (Err), never silently fall back to a constant key.
    #[test]
    fn unset_seed_refuses_to_start() {
        assert!(resolve_asserter_seed(None).is_err(), "unset seed must refuse to start (MEM-B-006)");
    }

    /// A set seed is used verbatim (operator-provided key material).
    #[test]
    fn set_seed_is_used_verbatim() {
        assert_eq!(
            resolve_asserter_seed(Some("operator-chosen-seed".into())).unwrap(),
            "operator-chosen-seed"
        );
    }

    /// MEM-B-006 tripwire: the old published dev seed must not exist anywhere in
    /// this binary's source. The needle is assembled from fragments so this test's
    /// own text does not contain the literal it forbids.
    #[test]
    fn no_default_seed_constant_in_binary() {
        let needle: String = ["dev", "seed", "CHANGE", "ME"].join("-");
        let src = include_str!("main.rs");
        assert!(
            !src.contains(&needle),
            "a hard-coded default asserter seed must not ship in the binary (MEM-B-006)"
        );
    }
}
