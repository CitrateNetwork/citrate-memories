//! The Mnemosyne gateway server (feature `server,rocksdb`).
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

    // Control plane: the dev Org + a dev Org-Owner membership (only meaningful with
    // dev-auth; in prod, memberships come from provisioning/invites).
    let mut control = ControlPlane::new();
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
        eprintln!("mem-gateway: dev-auth OFF — all requests fail closed until OIDC is wired (M1).");
    }

    let state = AppState::new(
        Arc::new(RwLock::new(control)),
        engines,
        Arc::new(SigningKey::from_bytes(&[42u8; 32])),
        allow_dev_auth,
        3_600_000,
        embedder,
    );

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
