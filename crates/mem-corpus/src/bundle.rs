//! The compact tenant-file encoding of corpus format 2.
//!
//! A corpus tenant file carries the same data as a [`mem_sync::SyncBundle`], but
//! the federation wire format serializes node `content` (a `Vec<u8>`) and edge
//! endpoints (`[u8; 32]`) as JSON arrays of numbers, about four bytes of JSON per
//! byte of text. The app ships the corpus, so format 2 writes:
//!
//! - node `content` as a JSON string when it is valid UTF-8 (every chunk the
//!   builder mints is), else as the plain byte array;
//! - edge `from` / `to` as 64-character lowercase hex.
//!
//! Every other field is the `SyncBundle` serialization unchanged, so node
//! identity (computed from the decoded node, never from these bytes) is the same
//! under both encodings. [`decode`] reads one node or edge at a time, so a large
//! tenant never expands into a whole-file `serde_json::Value` of numbers.

use mem_core::{Edge, MemoryNode};
use mem_sync::SyncBundle;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::CorpusError;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactBundle {
    repo: String,
    exported_at_ms: u64,
    nodes: Vec<Map<String, Value>>,
    edges: Vec<Map<String, Value>>,
}

fn serde_err(e: impl std::fmt::Display) -> CorpusError {
    CorpusError::Serde(e.to_string())
}

fn verify_err(e: impl std::fmt::Display) -> CorpusError {
    CorpusError::Verify(e.to_string())
}

fn object(v: Value, what: &str) -> Result<Map<String, Value>, CorpusError> {
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(serde_err(format!("{what} did not serialize to an object"))),
    }
}

fn compact_node(n: &MemoryNode) -> Result<Map<String, Value>, CorpusError> {
    let mut m = object(serde_json::to_value(n).map_err(serde_err)?, "node")?;
    if let Ok(text) = std::str::from_utf8(&n.content) {
        m.insert("content".into(), Value::String(text.to_string()));
    }
    Ok(m)
}

fn compact_edge(e: &Edge) -> Result<Map<String, Value>, CorpusError> {
    let mut m = object(serde_json::to_value(e).map_err(serde_err)?, "edge")?;
    m.insert("from".into(), Value::String(e.from.to_hex()));
    m.insert("to".into(), Value::String(e.to.to_hex()));
    Ok(m)
}

/// Encode a bundle as a format-2 tenant file (no trailing newline).
pub fn encode(b: &SyncBundle) -> Result<String, CorpusError> {
    let compact = CompactBundle {
        repo: b.repo.clone(),
        exported_at_ms: b.exported_at_ms,
        nodes: b.nodes.iter().map(compact_node).collect::<Result<_, _>>()?,
        edges: b.edges.iter().map(compact_edge).collect::<Result<_, _>>()?,
    };
    serde_json::to_string(&compact).map_err(serde_err)
}

fn bytes_value(bytes: &[u8]) -> Value {
    Value::Array(bytes.iter().map(|b| Value::from(*b)).collect())
}

fn hash_value(field: &str, v: &Value) -> Result<Value, CorpusError> {
    let s = v
        .as_str()
        .ok_or_else(|| verify_err(format!("edge {field} is not a hex string")))?;
    if s.len() != 64
        || !s
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(verify_err(format!(
            "edge {field} is not 64 lowercase hex characters"
        )));
    }
    let bytes = hex::decode(s).map_err(verify_err)?;
    Ok(bytes_value(&bytes))
}

/// Decode a format-2 tenant file. Refuses unknown top-level keys, a non-hex
/// edge endpoint, and anything that is not a valid node or edge once expanded.
pub fn decode(text: &str) -> Result<SyncBundle, CorpusError> {
    let c: CompactBundle = serde_json::from_str(text).map_err(verify_err)?;
    let mut nodes = Vec::with_capacity(c.nodes.len());
    for mut m in c.nodes {
        if let Some(Value::String(s)) = m.get("content") {
            let expanded = bytes_value(s.as_bytes());
            m.insert("content".into(), expanded);
        }
        nodes.push(serde_json::from_value::<MemoryNode>(Value::Object(m)).map_err(verify_err)?);
    }
    let mut edges = Vec::with_capacity(c.edges.len());
    for mut m in c.edges {
        for field in ["from", "to"] {
            let v = m
                .get(field)
                .ok_or_else(|| verify_err(format!("edge has no {field}")))?;
            let expanded = hash_value(field, v)?;
            m.insert(field.into(), expanded);
        }
        edges.push(serde_json::from_value::<Edge>(Value::Object(m)).map_err(verify_err)?);
    }
    Ok(SyncBundle {
        repo: c.repo,
        exported_at_ms: c.exported_at_ms,
        nodes,
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_core::{
        BelnapValue, ContentHash, EdgeKind, EdgeMethod, EdgeProvenance, NodeKind, Plane, SourceRef,
        Status, TrustTier, SCHEMA_VERSION,
    };

    fn node(content: Vec<u8>) -> MemoryNode {
        MemoryNode {
            schema_version: SCHEMA_VERSION,
            plane: Plane::Derived,
            kind: NodeKind::Doc,
            repo: "citrate-docs".into(),
            author: "corpus:citrate-docs".into(),
            source_ref: SourceRef::Artifact {
                repo: "citrate-docs".into(),
                path: "content/chain/genesis.md".into(),
                git_sha: "ab".repeat(32),
                byte_start: 3,
                byte_end: 40,
            },
            content,
            valid_from: 1,
            valid_to: None,
            observed_at: 2,
            trust_tier: TrustTier::DerivedDeterministic,
            signature: None,
            embedding: None,
            confidence: vec![BelnapValue::True],
            anchors: vec![],
            status: Status::Active,
        }
    }

    fn edge(from: ContentHash, to: ContentHash) -> Edge {
        Edge {
            from,
            to,
            kind: EdgeKind::DerivedFrom,
            plane: Plane::Derived,
            trust_tier: TrustTier::DerivedDeterministic,
            provenance: EdgeProvenance {
                method: EdgeMethod::Ingest,
                asserter: "corpus".into(),
                at: 2,
                evidence: Some("chain/genesis.md".into()),
            },
            confidence: vec![BelnapValue::True],
            quarantined: false,
            signature: None,
        }
    }

    fn bundle() -> SyncBundle {
        let a = node("Genesis \u{203a} What it is\n\nchain id 40204 (0x9d0c), \"quoted\"".into());
        let b = node(vec![0xff, 0x00, 0x41]);
        let e = edge(a.compute_id(), b.compute_id());
        SyncBundle {
            repo: "citrate-docs".into(),
            exported_at_ms: 2,
            nodes: vec![a, b],
            edges: vec![e],
        }
    }

    #[test]
    fn round_trips_text_and_binary_content_with_the_same_node_ids() {
        let b = bundle();
        let back = decode(&encode(&b).unwrap()).unwrap();
        assert_eq!(back.repo, b.repo);
        assert_eq!(back.exported_at_ms, b.exported_at_ms);
        assert_eq!(back.nodes, b.nodes);
        assert_eq!(back.edges, b.edges);
        let ids = |x: &SyncBundle| x.nodes.iter().map(|n| n.compute_id()).collect::<Vec<_>>();
        assert_eq!(ids(&back), ids(&b));
    }

    #[test]
    fn text_content_is_a_string_and_endpoints_are_hex() {
        let b = bundle();
        let text = encode(&b).unwrap();
        assert!(text.contains("\"content\":\"Genesis \u{203a} What it is"));
        assert!(text.contains(&format!("\"from\":\"{}\"", b.edges[0].from.to_hex())));
        // Invalid UTF-8 stays a byte array rather than being mangled.
        assert!(text.contains("\"content\":[255,0,65]"));
    }

    #[test]
    fn compact_is_much_smaller_than_the_wire_format() {
        let mut b = bundle();
        b.nodes[0].content = "a fairly ordinary sentence of documentation text. "
            .repeat(40)
            .into_bytes();
        let compact = encode(&b).unwrap().len();
        let wire = b.to_json().unwrap().len();
        assert!(compact * 2 < wire, "compact {compact} vs wire {wire}");
    }

    #[test]
    fn decode_refuses_bad_endpoints_and_unknown_keys() {
        let text = encode(&bundle()).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();

        let mut upper = v.clone();
        let from = upper["edges"][0]["from"].as_str().unwrap().to_uppercase();
        upper["edges"][0]["from"] = Value::String(from);
        assert!(decode(&upper.to_string()).is_err(), "uppercase hex refused");

        let mut short = v.clone();
        short["edges"][0]["to"] = Value::String("ab".into());
        assert!(decode(&short.to_string()).is_err(), "short hash refused");

        let mut array = v.clone();
        array["edges"][0]["to"] = bytes_value(&[1u8; 32]);
        assert!(
            decode(&array.to_string()).is_err(),
            "array endpoint refused in format 2"
        );

        let mut extra = v.clone();
        extra["surprise"] = Value::from(1);
        assert!(
            decode(&extra.to_string()).is_err(),
            "unknown top-level key refused"
        );

        let mut no_from = v;
        no_from["edges"][0].as_object_mut().unwrap().remove("from");
        assert!(decode(&no_from.to_string()).is_err());
    }

    #[test]
    fn encoding_is_deterministic() {
        assert_eq!(encode(&bundle()).unwrap(), encode(&bundle()).unwrap());
    }
}
