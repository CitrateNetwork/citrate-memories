//! Parser for the leading `---`-fenced Rule-12 frontmatter on `.md` docs.
//!
//! Frontmatter is flat `key: value` lines (created/branch/author/status/sprint),
//! so a full YAML parser is overkill. Returns the parsed map and the byte offset
//! where the body begins. Only the FIRST `:` splits a line, so ISO timestamps in
//! values (`created: 2026-06-05T22:00:00Z`) parse correctly.

use std::collections::BTreeMap;

/// Parsed frontmatter (empty if the doc has none) plus the doc body (everything
/// after the closing `---`).
pub fn parse(text: &str) -> (BTreeMap<String, String>, &str) {
    let rest = match text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) {
        Some(r) => r,
        None => return (BTreeMap::new(), text),
    };

    // Find the closing `---` on its own line.
    let mut idx = 0usize;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let fm_str = &rest[..idx];
            let body = &rest[idx + line.len()..];
            return (parse_kv(fm_str), body);
        }
        idx += line.len();
    }

    // No closing fence — treat the whole thing as body (malformed frontmatter).
    (BTreeMap::new(), text)
}

fn parse_kv(s: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in s.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let value = v.trim();
            if !key.is_empty() {
                map.insert(key.to_string(), value.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rule12_frontmatter() {
        let text = "---\ncreated: 2026-06-05T22:00:00Z\nbranch: main\nauthor: Saul + Claude\nstatus: active\nsprint: MEM-S0\n---\n\n# Title\n\nbody here";
        let (fm, body) = parse(text);
        assert_eq!(fm.get("created").map(String::as_str), Some("2026-06-05T22:00:00Z"));
        assert_eq!(fm.get("status").map(String::as_str), Some("active"));
        assert_eq!(fm.get("sprint").map(String::as_str), Some("MEM-S0"));
        assert!(body.starts_with("\n# Title"));
    }

    #[test]
    fn no_frontmatter_returns_whole_body() {
        let text = "# Just a heading\n\nno frontmatter";
        let (fm, body) = parse(text);
        assert!(fm.is_empty());
        assert_eq!(body, text);
    }

    #[test]
    fn unterminated_frontmatter_is_treated_as_body() {
        let text = "---\nkey: value\nno closing fence";
        let (fm, body) = parse(text);
        assert!(fm.is_empty());
        assert_eq!(body, text);
    }
}
