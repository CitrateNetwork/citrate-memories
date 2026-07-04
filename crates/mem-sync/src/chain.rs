//! WP-5.3 — bind a tenant merkle root to a live citrate-chain block.
//!
//! [`anchor_tenant`](crate::anchor_tenant) gives a tenant a hash-chained,
//! tamper-evident *local* root. This module gives that root a **global order**:
//! it pins the root to a specific citrate-chain block (number + hash), so two
//! replicas that disagree can be ordered by which chain height their roots were
//! witnessed at — the finality story the sprint's WP-5.3 calls for.
//!
//! **What this is and is not.** This is the read-only half — a *checkpoint
//! binding* fetched over JSON-RPC (`eth_chainId` / `eth_blockNumber` /
//! `eth_getBlockByNumber`). It needs no signer, moves no money, and touches no
//! contract, so it is not gated behind a Rule-8 deploy review. The *durable
//! on-chain write* (submitting the root as calldata to a memory-anchor contract,
//! so a third party can later read it back) is a separate operator step: it
//! needs an anchor contract deployed on 40204, a funded signer, and a Rule-8
//! sign-off because it touches signing — tracked as the WP-5.3 on-chain-write
//! follow-up. The binding here is already verifiable evidence of "root R existed
//! at chain height H".
//!
//! Fail-closed: any RPC error, malformed field, or chain-id mismatch returns
//! `SyncError::Chain` and records nothing — an anchor is never stamped against a
//! checkpoint we could not verify.

use serde::{Deserialize, Serialize};

use mem_core::MemoryNode;
use mem_store::MemoryDagStore;

use crate::{anchor_tenant, AnchorRecord, SyncError};

/// A witnessed citrate-chain checkpoint: where on the chain a root was pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainCheckpoint {
    /// EVM chain id (decimal). For Citrate this is 40204.
    pub chain_id: u64,
    pub block_number: u64,
    /// `0x`-prefixed block hash at `block_number`.
    pub block_hash: String,
    pub fetched_at_ms: u64,
}

/// A tenant root bound to a chain checkpoint — the unit a verifier checks for
/// global order / finality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAnchorRecord {
    pub anchor: AnchorRecord,
    pub checkpoint: ChainCheckpoint,
}

fn anchor_chain_key(repo: &str) -> Vec<u8> {
    format!("chain_anchor:{repo}").into_bytes()
}

/// One JSON-RPC round-trip. Kept tiny + dependency-light (ureq, blocking).
fn rpc_call(rpc_url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, SyncError> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp: serde_json::Value = ureq::post(rpc_url)
        .timeout(std::time::Duration::from_secs(15))
        .send_json(body)
        .map_err(|e| SyncError::Chain(format!("{method} RPC failed: {e}")))?
        .into_json()
        .map_err(|e| SyncError::Chain(format!("{method} response not JSON: {e}")))?;
    if let Some(err) = resp.get("error") {
        return Err(SyncError::Chain(format!("{method} RPC error: {err}")));
    }
    resp.get("result")
        .cloned()
        .ok_or_else(|| SyncError::Chain(format!("{method} response missing 'result'")))
}

fn hex_to_u64(s: &str) -> Result<u64, SyncError> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(stripped, 16).map_err(|e| SyncError::Chain(format!("bad hex quantity '{s}': {e}")))
}

/// Fetch the current chain head as a checkpoint. Verifies the endpoint's chain
/// id matches `expected_chain_id` (fail closed against a wrong/forked RPC).
pub fn fetch_chain_checkpoint(
    rpc_url: &str,
    expected_chain_id: u64,
    now_ms: u64,
) -> Result<ChainCheckpoint, SyncError> {
    let chain_id = hex_to_u64(
        rpc_call(rpc_url, "eth_chainId", serde_json::json!([]))?
            .as_str()
            .ok_or_else(|| SyncError::Chain("eth_chainId result not a string".into()))?,
    )?;
    if chain_id != expected_chain_id {
        return Err(SyncError::Chain(format!(
            "chain id mismatch: RPC is {chain_id}, expected {expected_chain_id}"
        )));
    }

    let block_number = hex_to_u64(
        rpc_call(rpc_url, "eth_blockNumber", serde_json::json!([]))?
            .as_str()
            .ok_or_else(|| SyncError::Chain("eth_blockNumber result not a string".into()))?,
    )?;

    // `false` => don't pull full tx bodies, we only want the header hash.
    let block = rpc_call(
        rpc_url,
        "eth_getBlockByNumber",
        serde_json::json!([format!("0x{block_number:x}"), false]),
    )?;
    let block_hash = block
        .get("hash")
        .and_then(|h| h.as_str())
        .ok_or_else(|| SyncError::Chain("block has no hash".into()))?
        .to_string();

    Ok(ChainCheckpoint { chain_id, block_number, block_hash, fetched_at_ms: now_ms })
}

/// Anchor a tenant locally (hash-chained root) AND bind that root to the live
/// chain head, persisting the bound record under `chain_anchor:<repo>`. The
/// local anchor is written only after the checkpoint is verified (fail closed —
/// a chain-reachability failure leaves no half-written chain anchor).
pub fn anchor_tenant_to_chain(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
    rpc_url: &str,
    expected_chain_id: u64,
    now_ms: u64,
) -> Result<ChainAnchorRecord, SyncError> {
    // Fetch + verify the checkpoint FIRST; if the chain is unreachable we record
    // nothing (neither the local anchor advance nor the chain binding).
    let checkpoint = fetch_chain_checkpoint(rpc_url, expected_chain_id, now_ms)?;
    let anchor = anchor_tenant(store, repo, now_ms)?;
    let record = ChainAnchorRecord { anchor, checkpoint };
    let bytes = serde_json::to_vec(&record).map_err(|e| SyncError::Serde(e.to_string()))?;
    store.put_meta(repo, &anchor_chain_key(repo), &bytes)?;
    Ok(record)
}

/// Read the latest chain-anchor record for a tenant.
pub fn latest_chain_anchor(
    store: &MemoryDagStore<MemoryNode>,
    repo: &str,
) -> Result<Option<ChainAnchorRecord>, SyncError> {
    match store.get_meta(&anchor_chain_key(repo))? {
        None => Ok(None),
        Some(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).map_err(|e| SyncError::Serde(e.to_string()))?))
        }
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    #[test]
    fn hex_to_u64_parses_chain_id_and_height() {
        assert_eq!(hex_to_u64("0x9d0c").unwrap(), 40204);
        assert_eq!(hex_to_u64("0x24887").unwrap(), 149_639);
        assert_eq!(hex_to_u64("0x0").unwrap(), 0);
        assert!(hex_to_u64("0xZZ").is_err());
    }

    #[test]
    fn checkpoint_round_trips_through_json() {
        let cp = ChainCheckpoint {
            chain_id: 40204,
            block_number: 149_127,
            block_hash: "0xabc".into(),
            fetched_at_ms: 1,
        };
        let j = serde_json::to_string(&cp).unwrap();
        let back: ChainCheckpoint = serde_json::from_str(&j).unwrap();
        assert_eq!(cp, back);
    }
}
