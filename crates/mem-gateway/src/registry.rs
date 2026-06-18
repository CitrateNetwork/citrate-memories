//! Org → engine routing. Each Org's memory lives in its **own** store; the registry
//! hands out the right engine handle for an Org and never mixes them. This is the
//! physical half of Org isolation (the authz gate in [`crate::authz`] is the logical
//! half). A query for Org A literally cannot see Org B's nodes because it runs
//! against a different store.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mem_core::MemoryNode;
use mem_store::MemoryDagStore;

use crate::org::OrgId;
use crate::GatewayError;

/// A handle to one Org's engine store, shared across requests/sessions.
pub type Engine = Arc<MemoryDagStore<MemoryNode>>;

/// Routes Orgs to their isolated engine instances. Engines are registered explicitly
/// (tests, or after provisioning) or — with the `rocksdb` feature — opened lazily by
/// the Org's `store_path` and cached.
#[derive(Default)]
pub struct OrgEngines {
    engines: Mutex<HashMap<OrgId, Engine>>,
}

impl OrgEngines {
    pub fn new() -> Self {
        Self { engines: Mutex::new(HashMap::new()) }
    }

    /// Register (or replace) an Org's engine handle. Used after provisioning, and by
    /// tests that inject in-memory stores.
    pub fn register(&self, org: OrgId, engine: Engine) -> Result<(), GatewayError> {
        let mut map = self.engines.lock().map_err(|_| GatewayError::EngineUnavailable("registry lock poisoned".into()))?;
        map.insert(org, engine);
        Ok(())
    }

    /// The engine for an Org, if one is registered/open. Returns `EngineUnavailable`
    /// rather than `Option` so the caller fails closed with a clear reason.
    pub fn get(&self, org: &OrgId) -> Result<Engine, GatewayError> {
        let map = self.engines.lock().map_err(|_| GatewayError::EngineUnavailable("registry lock poisoned".into()))?;
        map.get(org)
            .cloned()
            .ok_or_else(|| GatewayError::EngineUnavailable(format!("no engine for org '{}'", org.as_str())))
    }

    pub fn is_open(&self, org: &OrgId) -> bool {
        self.engines.lock().map(|m| m.contains_key(org)).unwrap_or(false)
    }

    /// Lazily open an Org's isolated RocksDB store by path (auto mode — encrypted iff
    /// its keyring is present) and cache the handle. Idempotent: a second call returns
    /// the cached engine. This is the production opener; per-Org single-writer locks
    /// keep Orgs from contending.
    #[cfg(feature = "rocksdb")]
    pub fn open_org(&self, org: &crate::org::Org) -> Result<Engine, GatewayError> {
        if let Ok(existing) = self.get(&org.id) {
            return Ok(existing);
        }
        let store = MemoryDagStore::<MemoryNode>::open_rocksdb_auto(&org.store_path)
            .map_err(|e| GatewayError::EngineUnavailable(format!("open {} : {e}", org.store_path)))?;
        let engine: Engine = Arc::new(store);
        self.register(org.id.clone(), Arc::clone(&engine))?;
        Ok(engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_core::{NodeKind, Plane, SourceRef, Status, TrustTier, SCHEMA_VERSION};
    use mem_store::kv::InMemoryKv;

    fn node(repo: &str, subject: &str) -> MemoryNode {
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

    fn engine_with(nodes: &[MemoryNode]) -> Engine {
        let store = MemoryDagStore::new(Box::new(InMemoryKv::new()));
        store.commit(nodes, &[]).unwrap();
        Arc::new(store)
    }

    #[test]
    fn unregistered_org_fails_closed() {
        let reg = OrgEngines::new();
        assert!(matches!(reg.get(&OrgId::new("ghost")), Err(GatewayError::EngineUnavailable(_))));
        assert!(!reg.is_open(&OrgId::new("ghost")));
    }

    #[test]
    fn engines_are_physically_isolated_per_org() {
        let reg = OrgEngines::new();
        // Two Orgs, each with the SAME repo name but DIFFERENT content.
        reg.register(OrgId::new("acme"), engine_with(&[node("shared-repo", "acme-secret")])).unwrap();
        reg.register(OrgId::new("globex"), engine_with(&[node("shared-repo", "globex-secret")])).unwrap();

        let acme = reg.get(&OrgId::new("acme")).unwrap();
        let globex = reg.get(&OrgId::new("globex")).unwrap();

        // A recall in acme sees only acme's node — never globex's, even though the
        // repo-tenant name collides. Isolation is by store, not by filter.
        let acme_titles: Vec<String> = mem_query::Recall::new(&acme)
            .storyline("shared-repo", 10)
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(acme_titles, vec!["acme-secret"]);

        let globex_titles: Vec<String> = mem_query::Recall::new(&globex)
            .storyline("shared-repo", 10)
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(globex_titles, vec!["globex-secret"]);
    }

    #[test]
    fn register_is_idempotent_replace() {
        let reg = OrgEngines::new();
        reg.register(OrgId::new("acme"), engine_with(&[node("r", "v1")])).unwrap();
        reg.register(OrgId::new("acme"), engine_with(&[node("r", "v2")])).unwrap();
        let e = reg.get(&OrgId::new("acme")).unwrap();
        let titles: Vec<String> = mem_query::Recall::new(&e).storyline("r", 10).unwrap().items.into_iter().map(|i| i.title).collect();
        assert_eq!(titles, vec!["v2"]);
    }
}
