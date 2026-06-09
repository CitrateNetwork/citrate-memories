//! Deterministic parser for the structured-communication "struct" (decision #9):
//! git commit trailers and fenced ```agentile blocks. These produce **load-bearing**
//! Derived edges (trust tier `derived-deterministic`). LLM-proposed edges are a
//! separate, advisory path (MEM-S4) and never come through here.

use mem_core::EdgeKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    pub key: String,
    pub value: String,
}

/// Is this a trailer-shaped line? `Key: value` where Key is a single token of
/// `[A-Za-z][A-Za-z0-9_-]*`.
fn parse_line(line: &str) -> Option<Trailer> {
    let (k, v) = line.split_once(':')?;
    let key = k.trim();
    let value = v.trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    let mut chars = key.chars();
    let first_ok = chars.next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false);
    let rest_ok = key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if first_ok && rest_ok {
        Some(Trailer {
            key: key.to_string(),
            value: value.to_string(),
        })
    } else {
        None
    }
}

/// Parse trailers from a commit body: the trailer block is the **last paragraph**
/// (git's own convention), and within it only trailer-shaped lines count.
pub fn parse_trailers(body: &str) -> Vec<Trailer> {
    let blocks: Vec<&str> = body.split("\n\n").filter(|b| !b.trim().is_empty()).collect();
    let Some(last) = blocks.last() else {
        return Vec::new();
    };
    let lines: Vec<&str> = last.lines().collect();
    // Only treat the block as trailers if every non-empty line is trailer-shaped;
    // this avoids misreading prose like "note: see above" as a trailer.
    let parsed: Vec<Trailer> = lines.iter().filter_map(|l| parse_line(l)).collect();
    if !lines.is_empty() && parsed.len() == lines.iter().filter(|l| !l.trim().is_empty()).count() {
        parsed
    } else {
        Vec::new()
    }
}

/// Parse `key: value` pairs from every fenced ```agentile block in `text`.
pub fn parse_agentile_blocks(text: &str) -> Vec<Trailer> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let t = line.trim();
        if !in_block && (t == "```agentile" || t == "~~~agentile") {
            in_block = true;
            continue;
        }
        if in_block && (t == "```" || t == "~~~") {
            in_block = false;
            continue;
        }
        if in_block {
            if let Some(tr) = parse_line(line) {
                out.push(tr);
            }
        }
    }
    out
}

/// Map an `Agentile-*` trailer key to the edge it asserts. Returns `None` for
/// keys that are not edge directives (e.g. `Agentile-Data-Source`, which is a
/// node attribute, handled elsewhere).
pub fn map_trailer(key: &str) -> Option<EdgeKind> {
    match key.to_ascii_lowercase().as_str() {
        "agentile-implements" => Some(EdgeKind::Implements),
        "agentile-decides" => Some(EdgeKind::Decides),
        "agentile-supersedes" => Some(EdgeKind::Supersedes),
        "agentile-refutes" => Some(EdgeKind::Refutes),
        "agentile-depends-on" => Some(EdgeKind::DependsOn),
        "agentile-caused-by" => Some(EdgeKind::CausedBy),
        "agentile-references" | "agentile-ref" | "agentile-see-also" => Some(EdgeKind::References),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trailer_block() {
        let body = "Did the thing.\n\nMore detail about it.\n\nAgentile-Implements: SELL-S2#step-3\nAgentile-Decides: ADR-2026-06-04-x402";
        let t = parse_trailers(body);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].key, "Agentile-Implements");
        assert_eq!(t[0].value, "SELL-S2#step-3");
    }

    #[test]
    fn prose_last_paragraph_is_not_trailers() {
        let body = "Subject body.\n\nThis is prose: it has a colon but is not a trailer.";
        assert!(parse_trailers(body).is_empty());
    }

    #[test]
    fn no_trailers_when_none() {
        assert!(parse_trailers("just a subject, no body").is_empty());
        assert!(parse_trailers("").is_empty());
    }

    #[test]
    fn parses_fenced_agentile_block() {
        let md = "intro\n\n```agentile\nnode: claim\nclaim-status: corrected\nsupersedes: old-node\n```\n\noutro";
        let pairs = parse_agentile_blocks(md);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1].key, "claim-status");
        assert_eq!(pairs[1].value, "corrected");
    }

    #[test]
    fn maps_known_directives_only() {
        assert_eq!(map_trailer("Agentile-Implements"), Some(EdgeKind::Implements));
        assert_eq!(map_trailer("agentile-decides"), Some(EdgeKind::Decides));
        assert_eq!(map_trailer("Agentile-Data-Source"), None);
        assert_eq!(map_trailer("Signed-off-by"), None);
    }
}
