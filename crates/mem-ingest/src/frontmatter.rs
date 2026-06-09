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

/// The doc's authored timestamp (epoch ms) from the frontmatter `created:` field,
/// if present and parseable. This is the bitemporal `valid_from` — "when the fact
/// became true in the world" — so a doc lands in the storyline at its real date
/// rather than at ingest time (finding F-3).
pub fn created_ms(fm: &BTreeMap<String, String>) -> Option<u64> {
    parse_iso8601_ms(fm.get("created")?)
}

/// Parse an ISO-8601 timestamp to epoch milliseconds. Deterministic and
/// dependency-free (no `chrono`): supports `YYYY-MM-DD`, optionally followed by
/// `THH:MM[:SS]`, an optional fractional part, and an optional `Z`. A numeric
/// timezone offset is ignored (federation frontmatter is UTC / `Z`). Returns
/// `None` on any malformed input so ingestion falls back to ingest time.
pub fn parse_iso8601_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date, time) = match s.split_once('T').or_else(|| s.split_once(' ')) {
        Some((d, t)) => (d, t),
        None => (s, ""),
    };

    let mut dparts = date.split('-');
    let year: i64 = dparts.next()?.parse().ok()?;
    let month: u32 = dparts.next()?.parse().ok()?;
    let day: u32 = dparts.next()?.parse().ok()?;
    if dparts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (mut h, mut min, mut sec) = (0u32, 0u32, 0u32);
    let time = time.trim_end_matches('Z');
    // Drop a trailing numeric timezone offset (+HH:MM / -HH:MM); we treat as UTC.
    let time = match time.find(['+']) {
        Some(i) => &time[..i],
        None => time,
    };
    // Drop fractional seconds.
    let time = time.split('.').next().unwrap_or(time);
    if !time.is_empty() {
        let mut tparts = time.split(':');
        h = tparts.next()?.parse().ok()?;
        min = tparts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
        sec = tparts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
        if tparts.next().is_some() || h >= 24 || min >= 60 || sec >= 60 {
            return None;
        }
    }

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + i64::from(h) * 3_600 + i64::from(min) * 60 + i64::from(sec);
    u64::try_from(secs.checked_mul(1_000)?).ok()
}

/// Days since 1970-01-01 for a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`). Exact integer arithmetic, no leap-year edge cases.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = i64::from((m + 9) % 12); // Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
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

    #[test]
    fn parses_iso8601_against_known_epochs() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601_ms("1970-01-02"), Some(86_400_000));
        // Well-known: 2000-01-01T00:00:00Z = 946_684_800 s.
        assert_eq!(parse_iso8601_ms("2000-01-01T00:00:00Z"), Some(946_684_800_000));
    }

    #[test]
    fn tolerates_offset_fraction_and_space_separator() {
        let z = parse_iso8601_ms("2026-06-08T03:30:00Z").unwrap();
        assert_eq!(parse_iso8601_ms("2026-06-08T03:30:00.123Z"), Some(z));
        assert_eq!(parse_iso8601_ms("2026-06-08T03:30:00+02:00"), Some(z)); // offset ignored
        assert_eq!(parse_iso8601_ms("2026-06-08 03:30:00"), Some(z));
        assert_eq!(parse_iso8601_ms("2026-06-08"), parse_iso8601_ms("2026-06-08T00:00:00Z"));
    }

    #[test]
    fn rejects_malformed_timestamps() {
        assert_eq!(parse_iso8601_ms(""), None);
        assert_eq!(parse_iso8601_ms("not-a-date"), None);
        assert_eq!(parse_iso8601_ms("2026-13-01"), None); // month out of range
        assert_eq!(parse_iso8601_ms("2026-06-08T25:00:00Z"), None); // hour out of range
        assert_eq!(parse_iso8601_ms("2026-06"), None); // missing day
    }

    #[test]
    fn dated_docs_order_before_later_docs() {
        let earlier = parse_iso8601_ms("2026-06-05T22:10:00Z").unwrap();
        let later = parse_iso8601_ms("2026-06-08T03:30:00Z").unwrap();
        assert!(earlier < later, "earlier created: must sort before later");
    }

    #[test]
    fn created_ms_reads_frontmatter_field() {
        let fm: BTreeMap<String, String> =
            [("created".to_string(), "2026-06-08T03:30:00Z".to_string())].into_iter().collect();
        assert_eq!(created_ms(&fm), parse_iso8601_ms("2026-06-08T03:30:00Z"));
        assert_eq!(created_ms(&BTreeMap::new()), None);
    }
}
