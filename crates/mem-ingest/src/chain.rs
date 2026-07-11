//! Chain-state ingest (E-4, `PLANSET/08_CHAIN_STATE_INGEST_ADR.md`).
//!
//! The first non-git Derived-plane source: citrate-chain network params, the
//! deployed-contract catalog (`contracts/addresses/40204.json` shape), and a
//! watched-address event walk, projected into the reserved **`chain-state`**
//! tenant. Everything the git ingestor promises holds here too — the graph is
//! a pure function of the source data, so two machines ingesting the same
//! catalog or the same block range produce **byte-identical node ids**.
//!
//! Vocabulary (see the ADR for the full rules):
//! - [`NodeKind::ChainNetwork`]  — one node per chainId (params summary)
//! - [`NodeKind::ChainContract`] — one node per (group, name, address); a
//!   redeploy mints a NEW node that supersedes the old one
//! - [`NodeKind::ChainCheckpoint`] — one node per witnessed (height, hash); a
//!   re-org supersedes the orphaned checkpoint, never mutates it
//! - [`NodeKind::ChainEvent`]    — one node per watched transaction
//! - edges: `References` (contract → network, "deployed-on"),
//!   `TemporalNext` (checkpoint spine), `Emits` (checkpoint → event),
//!   `Touches` (event → contract), `Supersedes` (re-org / redeploy)
//!
//! Determinism levers: catalog entries are BTree-sorted by (group, name) so
//! source order never leaks in; addresses and hashes are lowercased before
//! they enter identity; timestamps land only on non-identity fields.
//!
//! Tests replay RECORDED fixture transcripts (`tests/fixtures/`) — no test
//! ever performs a network call. The live [`RpcBlockSource`] reuses
//! `mem-sync`'s fail-closed JSON-RPC client and is feature-gated (`chain`) so
//! the default build stays network-free.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use mem_core::{
    BelnapValue, ContentHash, Edge, EdgeKind, EdgeMethod, MemoryNode, NodeKind, Plane, SourceRef,
    Status, TrustTier, SCHEMA_VERSION,
};
use mem_index::Embedder;
use mem_store::MemoryDagStore;

use crate::{IngestError, Watermark};

/// The reserved tenant all chain-state nodes live in.
pub const CHAIN_STATE_TENANT: &str = "chain-state";

/// Catalog groups ingested from a `40204.json`-shaped file, in the fixed
/// order they enter the graph.
const CATALOG_GROUPS: [&str; 4] = ["aaStack", "contracts", "genesis", "precompiles"];

fn chain_err(msg: impl Into<String>) -> IngestError {
    IngestError::Chain(msg.into())
}

fn hex_u64(s: &str) -> Result<u64, IngestError> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(stripped, 16).map_err(|e| chain_err(format!("bad hex quantity '{s}': {e}")))
}

fn hex_u128(s: &str) -> Result<u128, IngestError> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    u128::from_str_radix(stripped, 16).map_err(|e| chain_err(format!("bad hex quantity '{s}': {e}")))
}

/// Canonical address/hash form for identity: trimmed + lowercased. Checksummed
/// and lowercase spellings of the same address MUST hash identically.
fn norm_hex(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

// ─────────────────────────────────────────────────────────────────────────────
// WP-2: static ingest — network params + deployed-contract catalog
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Which catalog table the entry came from (`contracts`, `aaStack`, …).
    pub group: String,
    pub name: String,
    /// Normalised (lowercase) address.
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainCatalog {
    pub chain_id: u64,
    pub chain_name: String,
    pub rpc_url: String,
    pub explorer_url: String,
    /// Normalised (lowercase) deployer address.
    pub deployer: String,
    /// Sorted by (group, name) — parse/source order never leaks into ids.
    pub entries: Vec<CatalogEntry>,
}

/// Parse a pinned contract-address catalog in the shape of
/// `citrate-chain/contracts/addresses/40204.json`. Unknown top-level keys are
/// ignored (the catalog carries operator commentary); a malformed address
/// value is fail-closed.
pub fn parse_catalog(json: &str) -> Result<ChainCatalog, IngestError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| chain_err(format!("catalog is not JSON: {e}")))?;
    let chain_id = v
        .get("chainId")
        .and_then(|c| c.as_u64())
        .ok_or_else(|| chain_err("catalog missing numeric 'chainId'"))?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();

    let mut sorted: BTreeMap<(String, String), String> = BTreeMap::new();
    for group in CATALOG_GROUPS {
        let Some(obj) = v.get(group).and_then(|g| g.as_object()) else { continue };
        for (name, addr) in obj {
            let addr = addr
                .as_str()
                .ok_or_else(|| chain_err(format!("{group}.{name} address is not a string")))?;
            sorted.insert((group.to_string(), name.clone()), norm_hex(addr));
        }
    }
    Ok(ChainCatalog {
        chain_id,
        chain_name: text("chainName"),
        rpc_url: text("rpcUrl"),
        explorer_url: text("explorerUrl"),
        deployer: norm_hex(&text("deployer")),
        entries: sorted
            .into_iter()
            .map(|((group, name), address)| CatalogEntry { group, name, address })
            .collect(),
    })
}

fn chain_node(
    kind: NodeKind,
    key: String,
    content: String,
    valid_from: u64,
    now_ms: u64,
    embedder: &dyn Embedder,
) -> Result<MemoryNode, IngestError> {
    let embedding = Some(embedder.embed(&content)?);
    Ok(MemoryNode {
        schema_version: SCHEMA_VERSION,
        plane: Plane::Derived,
        kind,
        repo: CHAIN_STATE_TENANT.into(),
        author: "ingest".into(),
        source_ref: SourceRef::DagNative { key },
        content: content.into_bytes(),
        valid_from,
        valid_to: None,
        observed_at: now_ms,
        trust_tier: TrustTier::DerivedDeterministic,
        signature: None,
        embedding,
        confidence: vec![BelnapValue::True],
        anchors: vec![],
        status: Status::Active,
    })
}

fn network_key(chain_id: u64) -> String {
    format!("chain:{chain_id}")
}

/// Full identity key: the address is identity-bearing, so a redeploy is a new
/// node (see `contract_logical_key` for the redeploy-detection prefix).
fn contract_key(chain_id: u64, e: &CatalogEntry) -> String {
    format!("chain:{chain_id}/contract/{}/{}@{}", e.group, e.name, e.address)
}

/// The address-free logical key: two nodes sharing it are the SAME contract
/// role at different addresses — i.e. a redeploy → supersession.
fn contract_logical_key(full_key: &str) -> &str {
    full_key.rsplit_once('@').map(|(l, _)| l).unwrap_or(full_key)
}

fn checkpoint_key(chain_id: u64, number: u64, hash: &str) -> String {
    format!("chain:{chain_id}/block/{number}@{hash}")
}

fn event_key(chain_id: u64, tx_hash: &str) -> String {
    format!("chain:{chain_id}/tx/{tx_hash}")
}

fn dag_key(n: &MemoryNode) -> Option<&str> {
    match &n.source_ref {
        SourceRef::DagNative { key } => Some(key),
        _ => None,
    }
}

/// Pure core for WP-2: catalog → (nodes, edges). Deterministic given the same
/// catalog (`now_ms` lands only on non-identity fields). One `ChainNetwork`
/// node, one `ChainContract` node per entry, and a `References` edge
/// (contract → network, evidence "deployed-on") per contract.
pub fn build_catalog_graph(
    cat: &ChainCatalog,
    now_ms: u64,
    embedder: &dyn Embedder,
) -> Result<(Vec<MemoryNode>, Vec<Edge>), IngestError> {
    let net_content = format!(
        "{} — chainId {}, rpc {}, explorer {}, deployer {}",
        cat.chain_name, cat.chain_id, cat.rpc_url, cat.explorer_url, cat.deployer
    );
    let network = chain_node(
        NodeKind::ChainNetwork,
        network_key(cat.chain_id),
        net_content,
        now_ms,
        now_ms,
        embedder,
    )?;
    let net_id = network.compute_id();

    let mut nodes = vec![network];
    let mut edges = Vec::new();
    for e in &cat.entries {
        let content = format!("{} @ {} ({}) on chain {}", e.name, e.address, e.group, cat.chain_id);
        let node = chain_node(
            NodeKind::ChainContract,
            contract_key(cat.chain_id, e),
            content,
            now_ms,
            now_ms,
            embedder,
        )?;
        let id = node.compute_id();
        nodes.push(node);
        edges.push(crate::make_edge(
            id,
            net_id,
            EdgeKind::References,
            EdgeMethod::Ingest,
            now_ms,
            Some("deployed-on".into()),
        ));
    }
    Ok((nodes, edges))
}

#[derive(Debug, Clone)]
pub struct ChainCatalogReport {
    pub chain_id: u64,
    /// Contract nodes in this catalog (excludes the network node).
    pub contracts: usize,
    /// Redeploy supersessions applied (old address → Superseded).
    pub superseded: usize,
    /// Supersessions rejected by the store guards — counted, never fatal.
    pub supersessions_rejected: usize,
    pub watermark: Watermark,
}

fn active_chain_nodes(
    store: &MemoryDagStore<MemoryNode>,
    kind: NodeKind,
) -> Result<Vec<MemoryNode>, IngestError> {
    Ok(store
        .all_nodes()?
        .into_iter()
        .filter(|n| n.repo == CHAIN_STATE_TENANT && n.kind == kind && n.status == Status::Active)
        .collect())
}

/// Ingest a pinned catalog (JSON text) into the `chain-state` tenant.
/// Idempotent (content-addressed dedupe) and atomic (one batch). A contract
/// whose (group, name) already exists Active at a DIFFERENT address is a
/// redeploy: the new node supersedes the old one — append-only, no mutation.
/// The tenant watermark records the catalog content hash as its "head".
pub fn ingest_chain_catalog_str(
    json: &str,
    store: &MemoryDagStore<MemoryNode>,
    embedder: &dyn Embedder,
    now_ms: u64,
) -> Result<ChainCatalogReport, IngestError> {
    let cat = parse_catalog(json)?;
    let (nodes, edges) = build_catalog_graph(&cat, now_ms, embedder)?;

    // Redeploy detection against the CURRENT Active set (logical key → node).
    let mut prior: BTreeMap<String, (ContentHash, String)> = BTreeMap::new();
    for n in active_chain_nodes(store, NodeKind::ChainContract)? {
        if let Some(key) = dag_key(&n) {
            prior.insert(contract_logical_key(key).to_string(), (n.compute_id(), key.to_string()));
        }
    }
    let mut supersessions: Vec<Edge> = Vec::new();
    for n in nodes.iter().filter(|n| n.kind == NodeKind::ChainContract) {
        let Some(key) = dag_key(n) else { continue };
        if let Some((old_id, old_key)) = prior.get(contract_logical_key(key)) {
            if old_key != key {
                supersessions.push(crate::make_edge(
                    n.compute_id(),
                    *old_id,
                    EdgeKind::Supersedes,
                    EdgeMethod::Ingest,
                    now_ms,
                    Some("catalog redeploy: same (group, name), new address".into()),
                ));
            }
        }
    }

    store.commit(&nodes, &edges)?;
    let (superseded, supersessions_rejected) = crate::apply_supersessions(store, &supersessions)?;

    let watermark = Watermark {
        repo: CHAIN_STATE_TENANT.into(),
        head: Some(blake3::hash(json.as_bytes()).to_hex().to_string()),
        head_count: nodes.len(),
        ingested_at_ms: now_ms,
    };
    let bytes = serde_json::to_vec(&watermark).map_err(|e| IngestError::Serde(e.to_string()))?;
    store.put_meta(CHAIN_STATE_TENANT, &crate::watermark_key(CHAIN_STATE_TENANT), &bytes)?;

    Ok(ChainCatalogReport {
        chain_id: cat.chain_id,
        contracts: cat.entries.len(),
        superseded,
        supersessions_rejected,
        watermark,
    })
}

/// File wrapper around [`ingest_chain_catalog_str`].
pub fn ingest_chain_catalog(
    catalog_path: &Path,
    store: &MemoryDagStore<MemoryNode>,
    embedder: &dyn Embedder,
    now_ms: u64,
) -> Result<ChainCatalogReport, IngestError> {
    let json = std::fs::read_to_string(catalog_path)
        .map_err(|e| chain_err(format!("read {}: {e}", catalog_path.display())))?;
    ingest_chain_catalog_str(&json, store, embedder, now_ms)
}

// ─────────────────────────────────────────────────────────────────────────────
// WP-3: event ingest — watched-address walk over a block range
// ─────────────────────────────────────────────────────────────────────────────

/// One transaction as the walk sees it (normalised: lowercase hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxRecord {
    pub hash: String,
    pub from: String,
    /// `None` = contract creation.
    pub to: Option<String>,
    pub value_wei: u128,
}

/// One block as the walk sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRecord {
    pub number: u64,
    pub hash: String,
    pub parent_hash: String,
    pub timestamp_secs: u64,
    pub txs: Vec<TxRecord>,
}

/// Parse one raw `eth_getBlockByNumber(_, true)` **result** object into a
/// [`BlockRecord`]. The SAME parser serves the recorded-fixture source and the
/// live RPC source, so fixture tests exercise the real decoding path.
pub fn parse_rpc_block(result: &serde_json::Value) -> Result<BlockRecord, IngestError> {
    let field = |k: &str| {
        result
            .get(k)
            .and_then(|x| x.as_str())
            .ok_or_else(|| chain_err(format!("block missing string field '{k}'")))
    };
    let number = hex_u64(field("number")?)?;
    let hash = norm_hex(field("hash")?);
    let parent_hash = norm_hex(field("parentHash")?);
    let timestamp_secs = hex_u64(field("timestamp")?)?;

    let list = result
        .get("transactions")
        .and_then(|t| t.as_array())
        .ok_or_else(|| chain_err("block missing 'transactions' array (fetch with full tx bodies)"))?;
    let mut txs = Vec::with_capacity(list.len());
    for t in list {
        let tf = |k: &str| {
            t.get(k)
                .and_then(|x| x.as_str())
                .ok_or_else(|| chain_err(format!("tx missing string field '{k}'")))
        };
        let to = match t.get("to") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(norm_hex(
                v.as_str().ok_or_else(|| chain_err("tx 'to' is neither null nor a string"))?,
            )),
        };
        txs.push(TxRecord {
            hash: norm_hex(tf("hash")?),
            from: norm_hex(tf("from")?),
            to,
            value_wei: hex_u128(tf("value")?)?,
        });
    }
    Ok(BlockRecord { number, hash, parent_hash, timestamp_secs, txs })
}

/// Where blocks come from. `Ok(None)` = the source has no block at that height
/// (head reached) — the walk stops cleanly and the cursor stays there.
pub trait BlockSource {
    fn block_by_number(&self, number: u64) -> Result<Option<BlockRecord>, IngestError>;
}

/// Replays a RECORDED transcript of `eth_getBlockByNumber` results
/// (`{"results": [<result>, …]}`) — the only source tests ever use.
pub struct FixtureBlockSource {
    blocks: BTreeMap<u64, BlockRecord>,
}

impl FixtureBlockSource {
    pub fn from_transcript(json: &str) -> Result<Self, IngestError> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| chain_err(format!("transcript is not JSON: {e}")))?;
        let results = v
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or_else(|| chain_err("transcript missing 'results' array"))?;
        let mut blocks = BTreeMap::new();
        for r in results {
            let b = parse_rpc_block(r)?;
            blocks.insert(b.number, b);
        }
        Ok(Self { blocks })
    }
}

impl BlockSource for FixtureBlockSource {
    fn block_by_number(&self, number: u64) -> Result<Option<BlockRecord>, IngestError> {
        Ok(self.blocks.get(&number).cloned())
    }
}

/// Live block source over `mem-sync`'s fail-closed JSON-RPC client (E-4 WP-3
/// reuse; read-only, no signer). Feature-gated so the default build — and
/// every test — stays network-free.
#[cfg(feature = "chain")]
pub struct RpcBlockSource {
    rpc_url: String,
}

#[cfg(feature = "chain")]
impl RpcBlockSource {
    /// Fail-closed: verifies the endpoint's chain id BEFORE any walk, exactly
    /// like `mem_sync::chain::fetch_chain_checkpoint`.
    pub fn connect(rpc_url: &str, expected_chain_id: u64) -> Result<Self, IngestError> {
        let got = mem_sync::chain::rpc_call(rpc_url, "eth_chainId", serde_json::json!([]))
            .map_err(|e| chain_err(e.to_string()))?;
        let got = hex_u64(got.as_str().ok_or_else(|| chain_err("eth_chainId result not a string"))?)?;
        if got != expected_chain_id {
            return Err(chain_err(format!(
                "chain id mismatch: RPC is {got}, expected {expected_chain_id}"
            )));
        }
        Ok(Self { rpc_url: rpc_url.to_string() })
    }
}

#[cfg(feature = "chain")]
impl BlockSource for RpcBlockSource {
    fn block_by_number(&self, number: u64) -> Result<Option<BlockRecord>, IngestError> {
        // `true` => full tx bodies (events are minted from them).
        let v = mem_sync::chain::rpc_call(
            &self.rpc_url,
            "eth_getBlockByNumber",
            serde_json::json!([format!("0x{number:x}"), true]),
        )
        .map_err(|e| chain_err(e.to_string()))?;
        if v.is_null() {
            return Ok(None);
        }
        parse_rpc_block(&v).map(Some)
    }
}

/// Resumable walk position: the next block the walk will fetch. Advanced only
/// AFTER a block's batch is durably committed, so kill/restart resumes at the
/// first uncommitted block and content-addressed ids make any overlap a no-op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCursor {
    pub chain_id: u64,
    pub next_block: u64,
}

fn cursor_key(chain_id: u64) -> Vec<u8> {
    format!("chain_cursor:{chain_id}").into_bytes()
}

/// Read back the walk cursor (`None` if this chain was never walked).
pub fn read_chain_cursor(
    store: &MemoryDagStore<MemoryNode>,
    chain_id: u64,
) -> Result<Option<EventCursor>, IngestError> {
    match store.get_meta(&cursor_key(chain_id))? {
        None => Ok(None),
        Some(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).map_err(|e| IngestError::Serde(e.to_string()))?))
        }
    }
}

fn write_chain_cursor(
    store: &MemoryDagStore<MemoryNode>,
    cursor: &EventCursor,
) -> Result<(), IngestError> {
    let bytes = serde_json::to_vec(cursor).map_err(|e| IngestError::Serde(e.to_string()))?;
    store.put_meta(CHAIN_STATE_TENANT, &cursor_key(cursor.chain_id), &bytes)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ChainEventReport {
    /// Blocks fetched and committed this call.
    pub blocks: usize,
    /// Watched-transaction event nodes minted this call.
    pub events: usize,
    /// Orphaned checkpoints superseded by a re-org this call.
    pub reorgs_superseded: usize,
    /// Supersessions rejected by the store guards — counted, never fatal.
    pub supersessions_rejected: usize,
    pub cursor: EventCursor,
}

/// Walk blocks `[from, to_block]` (inclusive) minting `ChainCheckpoint` nodes
/// per block and `ChainEvent` nodes for transactions touching an address in
/// `watch` (as sender or recipient; empty `watch` = checkpoints only).
///
/// - `from_block = None` resumes from the stored cursor (0 if never walked).
/// - Commits per block and advances the cursor AFTER each commit → resumable.
/// - A re-org (same height, different hash, prior checkpoint still Active) is
///   handled by supersession, never mutation.
/// - `Touches` edges attach to catalog `ChainContract` nodes already in the
///   store — ingest the catalog first if you want them.
#[allow(clippy::too_many_arguments)]
pub fn ingest_chain_events(
    store: &MemoryDagStore<MemoryNode>,
    source: &dyn BlockSource,
    chain_id: u64,
    watch: &[String],
    from_block: Option<u64>,
    to_block: u64,
    embedder: &dyn Embedder,
    now_ms: u64,
) -> Result<ChainEventReport, IngestError> {
    let watch: BTreeSet<String> = watch.iter().map(|a| norm_hex(a)).collect();
    let start = match from_block {
        Some(b) => b,
        None => read_chain_cursor(store, chain_id)?.map(|c| c.next_block).unwrap_or(0),
    };
    let mut cursor = EventCursor { chain_id, next_block: start };

    // Address → contract-node id, for Touches edges.
    let mut contract_ids: BTreeMap<String, ContentHash> = BTreeMap::new();
    for n in active_chain_nodes(store, NodeKind::ChainContract)? {
        if let Some(key) = dag_key(&n) {
            if let Some((_, addr)) = key.rsplit_once('@') {
                contract_ids.insert(addr.to_string(), n.compute_id());
            }
        }
    }
    // Height → Active checkpoints already witnessed, for re-org supersession.
    let block_prefix = format!("chain:{chain_id}/block/");
    let mut checkpoints_by_height: BTreeMap<u64, Vec<(ContentHash, String)>> = BTreeMap::new();
    for n in active_chain_nodes(store, NodeKind::ChainCheckpoint)? {
        if let Some(key) = dag_key(&n) {
            if let Some(rest) = key.strip_prefix(&block_prefix) {
                if let Some((h, _)) = rest.split_once('@') {
                    if let Ok(height) = h.parse::<u64>() {
                        checkpoints_by_height
                            .entry(height)
                            .or_default()
                            .push((n.compute_id(), key.to_string()));
                    }
                }
            }
        }
    }

    let (mut blocks, mut events, mut reorgs, mut rejected) = (0usize, 0usize, 0usize, 0usize);
    let mut prev: Option<(String, ContentHash)> = None; // (block hash, checkpoint id)

    let mut n = start;
    while n <= to_block {
        let Some(block) = source.block_by_number(n)? else { break };
        let valid_from = block.timestamp_secs.saturating_mul(1000);

        let cp_key = checkpoint_key(chain_id, block.number, &block.hash);
        let cp_content = format!(
            "block {} on chain {} hash {} parent {}",
            block.number, chain_id, block.hash, block.parent_hash
        );
        let cp = chain_node(
            NodeKind::ChainCheckpoint,
            cp_key.clone(),
            cp_content,
            valid_from,
            now_ms,
            embedder,
        )?;
        let cp_id = cp.compute_id();

        let mut nodes = vec![cp];
        let mut edges: Vec<Edge> = Vec::new();
        let mut supersessions: Vec<Edge> = Vec::new();

        // Re-org: a still-Active checkpoint at this height with another hash
        // is orphaned — the new checkpoint supersedes it (append-only).
        if let Some(prior) = checkpoints_by_height.get(&block.number) {
            for (old_id, old_key) in prior {
                if *old_key != cp_key {
                    supersessions.push(crate::make_edge(
                        cp_id,
                        *old_id,
                        EdgeKind::Supersedes,
                        EdgeMethod::Ingest,
                        now_ms,
                        Some("re-org: same height, new hash".into()),
                    ));
                }
            }
        }

        // Chronological spine, only when the hashes actually chain.
        if let Some((prev_hash, prev_id)) = &prev {
            if *prev_hash == block.parent_hash {
                edges.push(crate::make_edge(
                    cp_id,
                    *prev_id,
                    EdgeKind::TemporalNext,
                    EdgeMethod::Ingest,
                    now_ms,
                    Some(prev_hash.clone()),
                ));
            }
        }

        for tx in &block.txs {
            let watched =
                watch.contains(&tx.from) || tx.to.as_ref().is_some_and(|t| watch.contains(t));
            if !watched {
                continue;
            }
            let to_str = tx.to.clone().unwrap_or_else(|| "create".into());
            let ev_content = format!(
                "tx {}: {} -> {} value {} wei in block {} on chain {}",
                tx.hash, tx.from, to_str, tx.value_wei, block.number, chain_id
            );
            let ev = chain_node(
                NodeKind::ChainEvent,
                event_key(chain_id, &tx.hash),
                ev_content,
                valid_from,
                now_ms,
                embedder,
            )?;
            let ev_id = ev.compute_id();
            nodes.push(ev);
            edges.push(crate::make_edge(
                cp_id,
                ev_id,
                EdgeKind::Emits,
                EdgeMethod::Ingest,
                now_ms,
                Some(tx.hash.clone()),
            ));
            // Touch every DISTINCT counterparty that has a catalog node.
            let counterparties: BTreeSet<&String> =
                [Some(&tx.from), tx.to.as_ref()].into_iter().flatten().collect();
            for addr in counterparties {
                if let Some(cid) = contract_ids.get(addr) {
                    edges.push(crate::make_edge(
                        ev_id,
                        *cid,
                        EdgeKind::Touches,
                        EdgeMethod::Ingest,
                        now_ms,
                        Some(addr.clone()),
                    ));
                }
            }
            events += 1;
        }

        // Commit this block's batch, THEN apply supersessions, THEN advance
        // the cursor — a crash resumes at the first uncommitted block.
        store.commit(&nodes, &edges)?;
        let (applied, rej) = crate::apply_supersessions(store, &supersessions)?;
        reorgs += applied;
        rejected += rej;
        checkpoints_by_height.entry(block.number).or_default().push((cp_id, cp_key));

        cursor.next_block = block.number + 1;
        write_chain_cursor(store, &cursor)?;

        prev = Some((block.hash.clone(), cp_id));
        blocks += 1;
        n += 1;
    }

    Ok(ChainEventReport {
        blocks,
        events,
        reorgs_superseded: reorgs,
        supersessions_rejected: rejected,
        cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_index::HashingEmbedder;
    use mem_store::kv::InMemoryKv;

    /// Vendored copy of `citrate-chain/contracts/addresses/40204.json` — the
    /// pinned catalog. Tests never read outside the repo and never network.
    const CATALOG: &str = include_str!("../tests/fixtures/40204.json");
    /// Recorded `eth_getBlockByNumber` transcript for blocks 149700–149703.
    const BLOCKS: &str = include_str!("../tests/fixtures/blocks_40204.json");
    /// Recorded transcript for the SAME heights 149702–149703 after a re-org.
    const BLOCKS_REORG: &str = include_str!("../tests/fixtures/blocks_40204_reorg.json");

    /// The watched member wallet — deliberately CHECKSUMMED to prove the
    /// normaliser makes case irrelevant to identity.
    const MEMBER: &str = "0x4250675F9015E65fC866F3a373F82bb9DFc000c6";
    const LEARNING_POOL: &str = "0xfc514b826daee16c590f86ad83370f4fb8a1d564";
    const MODEL_REGISTRY: &str = "0xf64636d56ec9e0c406149b34ea9c5c5d80b342c0";

    fn store() -> MemoryDagStore<MemoryNode> {
        MemoryDagStore::new(Box::new(InMemoryKv::new()))
    }

    fn emb() -> HashingEmbedder {
        HashingEmbedder::new(64)
    }

    #[test]
    fn parse_catalog_covers_all_groups_and_normalizes() {
        let cat = parse_catalog(CATALOG).unwrap();
        assert_eq!(cat.chain_id, 40204);
        assert_eq!(cat.chain_name, "Citrate Network");
        // 45 contracts + 7 aaStack + 1 genesis + 6 precompiles.
        assert_eq!(cat.entries.len(), 59);
        assert!(cat.entries.iter().all(|e| e.address == e.address.to_ascii_lowercase()));
        // Checksummed source spelling (EntryPoint) is normalised.
        let ep = cat.entries.iter().find(|e| e.name == "EntryPoint").unwrap();
        assert_eq!(ep.address, "0x077fbc3338a9e6bad90a3a041e6b7425689754ef");
        assert_eq!(ep.group, "aaStack");
        // BTree order: sorted by (group, name), independent of source order.
        let mut sorted = cat.entries.clone();
        sorted.sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
        assert_eq!(cat.entries, sorted);
    }

    #[test]
    fn catalog_graph_is_deterministic() {
        // WP-2 property: same catalog ⇒ byte-identical node ids, regardless of
        // when the ingest ran.
        let cat = parse_catalog(CATALOG).unwrap();
        let (n1, e1) = build_catalog_graph(&cat, 1, &emb()).unwrap();
        let (n2, e2) = build_catalog_graph(&cat, 999_999, &emb()).unwrap();
        let ids1: Vec<_> = n1.iter().map(|n| n.compute_id()).collect();
        let ids2: Vec<_> = n2.iter().map(|n| n.compute_id()).collect();
        assert_eq!(ids1, ids2, "node ids are independent of now_ms");
        let k1: Vec<_> = e1.iter().map(|e| e.key()).collect();
        let k2: Vec<_> = e2.iter().map(|e| e.key()).collect();
        assert_eq!(k1, k2, "edge keys are independent of now_ms");
    }

    #[test]
    fn catalog_ingest_twice_yields_identical_ids_and_counts() {
        let s = store();
        let r1 = ingest_chain_catalog_str(CATALOG, &s, &emb(), 100).unwrap();
        assert_eq!(r1.chain_id, 40204);
        assert_eq!(r1.contracts, 59);
        assert_eq!(r1.superseded, 0);
        let (n1, e1) = (s.node_count().unwrap(), s.edge_count().unwrap());
        assert_eq!(n1, 60, "network node + 59 contracts");
        assert_eq!(e1, 59, "one deployed-on edge per contract");

        // Re-ingest at a different time: content-addressed ⇒ pure no-op.
        let r2 = ingest_chain_catalog_str(CATALOG, &s, &emb(), 999).unwrap();
        assert_eq!(s.node_count().unwrap(), n1);
        assert_eq!(s.edge_count().unwrap(), e1);
        assert_eq!(r2.watermark.head, r1.watermark.head, "catalog hash is the head");
        assert_eq!(
            r1.watermark.head.as_deref(),
            Some(blake3::hash(CATALOG.as_bytes()).to_hex().as_str()),
        );
    }

    #[test]
    fn catalog_nodes_are_typed_linked_and_searchable() {
        let s = store();
        ingest_chain_catalog_str(CATALOG, &s, &emb(), 1).unwrap();
        let nodes = s.all_nodes().unwrap();
        assert!(nodes.iter().all(|n| n.repo == CHAIN_STATE_TENANT));
        assert!(nodes.iter().all(|n| n.embedding.is_some()), "catalog is searchable");
        assert!(nodes.iter().all(|n| n.trust_tier == TrustTier::DerivedDeterministic));

        let net = nodes.iter().find(|n| n.kind == NodeKind::ChainNetwork).unwrap();
        assert!(String::from_utf8_lossy(&net.content).contains("chainId 40204"));

        // Every contract references its network ("deployed-on").
        let lp = nodes
            .iter()
            .find(|n| String::from_utf8_lossy(&n.content).starts_with("LearningPool @"))
            .unwrap();
        assert_eq!(lp.kind, NodeKind::ChainContract);
        let out = s.out_edges(&lp.compute_id()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, EdgeKind::References);
        assert_eq!(out[0].to, net.compute_id());
        assert!(!out[0].quarantined, "catalog edges are load-bearing Derived edges");
    }

    #[test]
    fn redeploy_supersedes_old_contract_node() {
        let s = store();
        ingest_chain_catalog_str(CATALOG, &s, &emb(), 1).unwrap();

        // v2 catalog: LearningPool redeployed at a new address.
        let moved = "0x00000000000000000000000000000000000abcde";
        let v2 = CATALOG.replace(LEARNING_POOL, moved);
        assert_ne!(v2, CATALOG, "fixture contains the address exactly once");
        let r2 = ingest_chain_catalog_str(&v2, &s, &emb(), 2).unwrap();
        assert_eq!(r2.superseded, 1, "old-address node transitioned");
        assert_eq!(r2.supersessions_rejected, 0);

        let nodes = s.all_nodes().unwrap();
        let lps: Vec<_> = nodes
            .iter()
            .filter(|n| String::from_utf8_lossy(&n.content).starts_with("LearningPool @"))
            .collect();
        assert_eq!(lps.len(), 2, "append-only: both address generations exist");
        let old = lps
            .iter()
            .find(|n| String::from_utf8_lossy(&n.content).contains(LEARNING_POOL))
            .unwrap();
        let new = lps
            .iter()
            .find(|n| String::from_utf8_lossy(&n.content).contains(moved))
            .unwrap();
        assert_eq!(old.status, Status::Superseded, "re-deploy = supersession, not mutation");
        assert_eq!(new.status, Status::Active);
    }

    #[test]
    fn parse_rpc_block_reads_the_recorded_shape() {
        let v: serde_json::Value = serde_json::from_str(BLOCKS).unwrap();
        let b = parse_rpc_block(&v["results"][0]).unwrap();
        assert_eq!(b.number, 149_700);
        assert_eq!(b.timestamp_secs, 0x686a_0000);
        assert_eq!(b.txs.len(), 1);
        assert_eq!(b.txs[0].from, MEMBER.to_ascii_lowercase(), "checksummed from is normalised");
        assert_eq!(b.txs[0].to.as_deref(), Some(LEARNING_POOL));
        assert_eq!(b.txs[0].value_wei, 1_000_000_000_000_000_000);
        // Contract creation parses to `to: None`.
        let create = parse_rpc_block(&v["results"][2]).unwrap();
        assert_eq!(create.txs[0].to, None);
    }

    #[test]
    fn event_walk_matches_the_fixture_exactly_and_is_deterministic() {
        let s = store();
        ingest_chain_catalog_str(CATALOG, &s, &emb(), 1).unwrap();
        let src = FixtureBlockSource::from_transcript(BLOCKS).unwrap();

        let watch = vec![MEMBER.to_string()]; // checksummed on purpose
        let r = ingest_chain_events(&s, &src, 40204, &watch, Some(149_700), 149_703, &emb(), 5)
            .unwrap();
        assert_eq!(r.blocks, 4);
        assert_eq!(r.events, 3, "txs aaaa (member→pool), cccc (→member), dddd (create)");
        assert_eq!(r.reorgs_superseded, 0);
        assert_eq!(r.cursor, EventCursor { chain_id: 40204, next_block: 149_704 });

        let nodes = s.all_nodes().unwrap();
        let evs: Vec<_> = nodes.iter().filter(|n| n.kind == NodeKind::ChainEvent).collect();
        assert_eq!(evs.len(), 3);
        let texts: Vec<String> =
            evs.iter().map(|n| String::from_utf8_lossy(&n.content).into_owned()).collect();
        assert!(texts.iter().any(|t| t.starts_with("tx 0xaaaa")), "watched outgoing tx");
        assert!(texts.iter().any(|t| t.starts_with("tx 0xcccc")), "watched incoming tx");
        assert!(texts.iter().any(|t| t.contains("-> create")), "contract creation");
        assert!(!texts.iter().any(|t| t.starts_with("tx 0xbbbb")), "unwatched tx minted nothing");

        // Checkpoints chain on the spine; events hang off them via Emits.
        let cps: Vec<_> = nodes.iter().filter(|n| n.kind == NodeKind::ChainCheckpoint).collect();
        assert_eq!(cps.len(), 4);
        let ev_a = evs
            .iter()
            .find(|n| String::from_utf8_lossy(&n.content).starts_with("tx 0xaaaa"))
            .unwrap();
        let touches = s.out_edges(&ev_a.compute_id()).unwrap();
        assert!(
            touches.iter().any(|e| e.kind == EdgeKind::Touches),
            "member→LearningPool touches the catalog contract node"
        );

        // Walking the SAME recorded range again is a pure no-op on the store.
        let (n1, e1) = (s.node_count().unwrap(), s.edge_count().unwrap());
        ingest_chain_events(&s, &src, 40204, &watch, Some(149_700), 149_703, &emb(), 999).unwrap();
        assert_eq!(s.node_count().unwrap(), n1, "same range ⇒ byte-identical ids ⇒ no growth");
        assert_eq!(s.edge_count().unwrap(), e1);
    }

    #[test]
    fn cursor_resumes_without_duplicates() {
        let s = store();
        let src = FixtureBlockSource::from_transcript(BLOCKS).unwrap();
        let watch = vec![MEMBER.to_string()];

        // First leg: two blocks, then "the daemon dies".
        let r1 = ingest_chain_events(&s, &src, 40204, &watch, Some(149_700), 149_701, &emb(), 1)
            .unwrap();
        assert_eq!((r1.blocks, r1.events), (2, 2));
        assert_eq!(
            read_chain_cursor(&s, 40204).unwrap(),
            Some(EventCursor { chain_id: 40204, next_block: 149_702 })
        );
        let n1 = s.node_count().unwrap();

        // Restart: `from_block: None` resumes from the durable cursor.
        let r2 =
            ingest_chain_events(&s, &src, 40204, &watch, None, 149_703, &emb(), 2).unwrap();
        assert_eq!((r2.blocks, r2.events), (2, 1), "only the unwalked tail is fetched");
        assert_eq!(s.node_count().unwrap(), n1 + 3, "2 checkpoints + 1 event, zero duplicates");

        // Resume again with nothing new: cursor is past the range ⇒ no-op.
        let r3 =
            ingest_chain_events(&s, &src, 40204, &watch, None, 149_703, &emb(), 3).unwrap();
        assert_eq!((r3.blocks, r3.events), (0, 0));
        assert_eq!(r3.cursor.next_block, 149_704, "cursor is durable across the no-op");
    }

    #[test]
    fn reorg_is_handled_by_supersession_not_mutation() {
        let s = store();
        ingest_chain_catalog_str(CATALOG, &s, &emb(), 1).unwrap();
        let watch = vec![MEMBER.to_string()];
        let src = FixtureBlockSource::from_transcript(BLOCKS).unwrap();
        ingest_chain_events(&s, &src, 40204, &watch, Some(149_700), 149_703, &emb(), 2).unwrap();

        // The chain re-orgs at 149702: replay the recorded post-re-org blocks.
        let reorg = FixtureBlockSource::from_transcript(BLOCKS_REORG).unwrap();
        let r = ingest_chain_events(&s, &reorg, 40204, &watch, Some(149_702), 149_703, &emb(), 3)
            .unwrap();
        assert_eq!(r.blocks, 2);
        assert_eq!(r.reorgs_superseded, 2, "both orphaned checkpoints transitioned");
        assert_eq!(r.events, 1, "the re-orged block carries the member→ModelRegistry tx");

        let nodes = s.all_nodes().unwrap();
        let at = |height: u64, frag: &str| {
            nodes
                .iter()
                .find(|n| {
                    n.kind == NodeKind::ChainCheckpoint
                        && String::from_utf8_lossy(&n.content).contains(&format!("block {height} "))
                        && String::from_utf8_lossy(&n.content).contains(frag)
                })
                .cloned()
                .unwrap()
        };
        let orphaned = at(149_702, "hash 0xb10c");
        let canonical = at(149_702, "hash 0xe20a");
        assert_eq!(orphaned.status, Status::Superseded, "orphan superseded, never deleted");
        assert_eq!(canonical.status, Status::Active);

        // The new event touches the ModelRegistry catalog node.
        let ev = nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::ChainEvent
                    && String::from_utf8_lossy(&n.content).starts_with("tx 0xeeee")
            })
            .unwrap();
        let out = s.out_edges(&ev.compute_id()).unwrap();
        let touched: Vec<_> = out.iter().filter(|e| e.kind == EdgeKind::Touches).collect();
        assert_eq!(touched.len(), 1);
        let target = s.get_node(&touched[0].to).unwrap().unwrap();
        assert!(String::from_utf8_lossy(&target.content).contains(MODEL_REGISTRY));
    }

    #[test]
    fn bad_catalog_and_bad_block_fail_closed() {
        assert!(parse_catalog("{}").is_err(), "missing chainId is fatal");
        assert!(parse_catalog("not json").is_err());
        let v: serde_json::Value =
            serde_json::json!({ "number": "0x1", "hash": "0xa", "parentHash": "0xb", "timestamp": "0xzz", "transactions": [] });
        assert!(parse_rpc_block(&v).is_err(), "bad hex is fatal, never a silent 0");
        let s = store();
        assert!(ingest_chain_catalog_str("{}", &s, &emb(), 1).is_err());
        assert_eq!(s.node_count().unwrap(), 0, "failed ingest commits nothing");
    }
}
