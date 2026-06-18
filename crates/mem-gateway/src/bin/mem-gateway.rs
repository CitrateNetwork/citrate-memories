//! The Memrizz gateway server (feature `server,rocksdb`).
//!
//! M0 dev runner: serves the HTTP read API + 3D-layout endpoint over one or more
//! Org stores. For local/prototype use it registers a single dev Org pointing at a
//! real store path and turns on dev-auth so the constellation prototype can load the
//! actual graph with an `x-dev-sub` header.
//!
//! ```bash
//! MEM_GATEWAY_ALLOW_DEV_AUTH=1 \
//! cargo run -p mem-gateway --bin mem-gateway --features server,rocksdb,transformer -- \
//!     --org dev-org --store ./data/federation.bge.enc.memdag --bind 127.0.0.1:8799
//!
//! curl -s -H 'x-dev-sub: dev' http://127.0.0.1:8799/api/orgs/dev-org/layout | jq '.node_count'
//! ```
//!
//! Production auth (OIDC bearer verification) + multi-Org provisioning are the M1
//! wiring; with dev-auth off the gateway fails closed (refuses every request).

use std::sync::{Arc, RwLock};

use ed25519_dalek::SigningKey;

use mem_gateway::http::{router, AppState};
use mem_gateway::org::{ControlPlane, Membership, Org, OrgId, OrgStatus, Role};
use mem_gateway::registry::OrgEngines;

fn arg(flag: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(feature = "transformer")]
fn load_embedder(engine: &mem_gateway::registry::Engine) -> Option<Arc<dyn mem_index::Embedder>> {
    let model = mem_query::detect_store_embedding_model(engine).ok().flatten()?;
    if model != mem_index::transformer::DEFAULT_MODEL_ID {
        return None;
    }
    eprintln!("mem-gateway: store embedded with '{model}', loading transformer embedder for search…");
    mem_index::TransformerEmbedder::bge_base().ok().map(|e| Arc::new(e) as Arc<dyn mem_index::Embedder>)
}

#[cfg(not(feature = "transformer"))]
fn load_embedder(_engine: &mem_gateway::registry::Engine) -> Option<Arc<dyn mem_index::Embedder>> {
    None
}

#[tokio::main]
async fn main() {
    let org_id = arg("--org", "dev-org");
    let store_path = arg("--store", "./data/federation.bge.enc.memdag");
    let bind = arg("--bind", "127.0.0.1:8799");
    let allow_dev_auth = std::env::var("MEM_GATEWAY_ALLOW_DEV_AUTH").map(|v| v == "1").unwrap_or(false);

    // Open the Org's isolated store + register it.
    let engines = Arc::new(OrgEngines::new());
    let org = Org {
        id: OrgId::new(&org_id),
        name: org_id.clone(),
        created_at_ms: now_ms(),
        store_path: store_path.clone(),
        status: OrgStatus::Active,
    };
    let engine = match engines.open_org(&org) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("mem-gateway: cannot open store {store_path}: {e}");
            std::process::exit(1);
        }
    };
    let embedder = load_embedder(&engine);

    // Control plane (gap G-3): durable. Load the Org/membership graph from disk so
    // it survives restarts; ensure this Org is present, add the dev Org-Owner when
    // dev-auth is on, and persist atomically. Production memberships come from
    // provisioning/invites (which re-save through the same path).
    let control_path = std::path::PathBuf::from(
        std::env::var("MEM_GATEWAY_CONTROL_PATH").unwrap_or_else(|_| "data/control.json".to_string()),
    );
    let mut control = match ControlPlane::load_or_default(&control_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mem-gateway: FATAL — control plane unreadable at {} ({e}); refusing to start.", control_path.display());
            std::process::exit(1);
        }
    };
    control.upsert_org(org);
    if allow_dev_auth {
        control.upsert_membership(Membership {
            sub: "dev".into(),
            org: OrgId::new(&org_id),
            role: Role::OrgOwner,
            scopes: vec![],
            parent: None,
        });
        eprintln!("mem-gateway: DEV-AUTH ON — 'dev' is Org Owner of '{org_id}'. Do NOT use in production.");
    } else {
        eprintln!("mem-gateway: dev-auth OFF — requests need a verified OIDC bearer (G-2).");
    }
    if let Err(e) = control.save_atomic(&control_path) {
        eprintln!("mem-gateway: WARN — could not persist control plane: {e}");
    } else {
        eprintln!("mem-gateway: control plane persisted at {} ({} orgs).", control_path.display(), control.orgs().len());
    }

    let mut state = AppState::new(
        Arc::new(RwLock::new(control)),
        engines,
        Arc::new(SigningKey::from_bytes(&[42u8; 32])),
        allow_dev_auth,
        3_600_000,
        embedder,
    );

    // Real OIDC bearer verification (gap G-2). Enabled when issuer + audience +
    // a JWKS source are all configured. The JWKS is provided as config (a file or
    // inline JSON) to avoid a startup network fetch; rotate it by redeploying the
    // secret. When set, OIDC takes precedence and dev-auth is ignored.
    if let (Ok(issuer), Ok(audience)) = (std::env::var("OIDC_ISSUER"), std::env::var("OIDC_AUDIENCE")) {
        let jwks = std::env::var("OIDC_JWKS_JSON").ok().or_else(|| {
            std::env::var("OIDC_JWKS_FILE").ok().and_then(|p| std::fs::read_to_string(p).ok())
        });
        match jwks.as_deref().map(|j| mem_gateway::oidc::OidcVerifier::from_jwks_json(&issuer, &audience, j)) {
            Some(Ok(v)) => {
                state = state.with_oidc(Arc::new(v));
                eprintln!("mem-gateway: OIDC ON — issuer={issuer} audience={audience} (dev-auth ignored).");
            }
            Some(Err(e)) => {
                eprintln!("mem-gateway: FATAL — OIDC configured but JWKS is invalid ({e}); refusing to start fail-open.");
                std::process::exit(1);
            }
            None => {
                eprintln!("mem-gateway: FATAL — OIDC_ISSUER/AUDIENCE set but no OIDC_JWKS_JSON/OIDC_JWKS_FILE; refusing to start.");
                std::process::exit(1);
            }
        }
    }

    // Write path (gap G-1): a signing identity for assert/propose, plus the
    // tamper-evident mutation audit chain. The asserter key is derived from
    // MEM_GATEWAY_ASSERTER_SEED (any string → blake3 → 32-byte ed25519 seed); a
    // dev default is used when unset. Without these, write routes 503 / proceed
    // unaudited respectively.
    let asserter_seed = std::env::var("MEM_GATEWAY_ASSERTER_SEED")
        .unwrap_or_else(|_| "mem-gateway-dev-asserter".to_string());
    let asserter = mem_assert::Asserter::new(SigningKey::from_bytes(blake3::hash(asserter_seed.as_bytes()).as_bytes()));
    state = state.with_asserter(Arc::new(asserter));

    let audit_path = std::env::var("MEM_GATEWAY_AUDIT_PATH")
        .unwrap_or_else(|_| "data/gateway-audit.jsonl".to_string());
    match mem_authz::AuditChain::open(&audit_path) {
        Ok(chain) => {
            state = state.with_audit(Arc::new(std::sync::Mutex::new(chain)));
            eprintln!("mem-gateway: write audit chain at {audit_path} (mutations + denials recorded).");
        }
        Err(e) => {
            eprintln!("mem-gateway: WARN — audit chain unavailable ({e}); writes proceed UNAUDITED.");
        }
    }

    // Persist provisioning mutations (gap G-4) through the durable control plane.
    state = state.with_control_path(std::sync::Arc::new(control_path.clone()));

    // Warm the 3D-layout cache before serving (dev-auth only — we have a known
    // subject to build it for). The expensive PCA runs once here at boot, so the
    // first user request to /layout is already cached.
    if allow_dev_auth {
        let t = std::time::Instant::now();
        match mem_gateway::http::build_scene_cached(&state, "dev", &OrgId::new(&org_id)) {
            Ok(scene) => eprintln!(
                "mem-gateway: layout cache warmed in {:.1}s ({} bytes)",
                t.elapsed().as_secs_f64(),
                scene.to_string().len()
            ),
            Err((_, m)) => eprintln!("mem-gateway: WARN layout warm failed (will build on first request): {m}"),
        }
    }

    let app = router(state);
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("mem-gateway: cannot bind {bind}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("mem-gateway: serving org '{org_id}' ({store_path}) on http://{bind}");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("mem-gateway: server error: {e}");
        std::process::exit(1);
    }
}
