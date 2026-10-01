//! Heading-aware chunking. Pure and deterministic: the same text always yields
//! the same chunks with the same byte spans.
//!
//! Markdown (`#`..`######`, outside fenced code) and AsciiDoc (`=`..`======`)
//! headings start a new section; each chunk carries a `Title › Section`
//! breadcrumb so a recall hit names where it came from. A section longer than
//! the target is split on line boundaries; a single line longer than the target
//! is split on character boundaries. Text with no headings is split by size.

/// One chunk of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// `Title` or `Title › Section`.
    pub breadcrumb: String,
    /// The chunk text (trimmed of surrounding blank lines).
    pub text: String,
    /// Byte span of `text`'s source lines in the original file.
    pub byte_start: u64,
    pub byte_end: u64,
}

/// A section: its heading (none before the first one) and its `(line, byte offset)` lines.
type Section<'a> = (Option<String>, Vec<(&'a str, usize)>);

fn heading(line: &str) -> Option<String> {
    let t = line.trim_end();
    for marker in ['#', '='] {
        let level = t.chars().take_while(|c| *c == marker).count();
        if (1..=6).contains(&level) {
            let rest = &t[level..];
            if let Some(h) = rest.strip_prefix(' ') {
                let h = h.trim().trim_end_matches(marker).trim();
                if !h.is_empty() {
                    return Some(h.to_string());
                }
            }
        }
    }
    None
}

/// Split `text` into chunks. `body_offset` is the byte offset of `text` inside
/// the original file (e.g. after a stripped frontmatter block), so spans point
/// into the file. `title` is the file's title.
pub fn chunk_text(title: &str, text: &str, body_offset: usize, max_chars: usize) -> Vec<Chunk> {
    // Sections: (heading, [(line, byte_start)]).
    let mut sections: Vec<Section<'_>> = vec![(None, Vec::new())];
    let mut in_fence = false;
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            if let Some(h) = heading(line) {
                sections.push((Some(h), vec![(line, start)]));
                continue;
            }
        }
        if let Some(last) = sections.last_mut() {
            last.1.push((line, start));
        }
    }

    let mut out = Vec::new();
    for (head, lines) in sections {
        let breadcrumb = match &head {
            Some(h) if h != title => format!("{title} \u{203a} {h}"),
            _ => title.to_string(),
        };
        let mut buf = String::new();
        let mut buf_start: Option<usize> = None;
        let mut buf_end = 0usize;
        let flush =
            |buf: &mut String, start: &mut Option<usize>, end: usize, out: &mut Vec<Chunk>| {
                let t = buf.trim_matches(|c: char| c == '\n' || c == '\r');
                if !t.trim().is_empty() {
                    if let Some(s) = *start {
                        out.push(Chunk {
                            breadcrumb: breadcrumb.clone(),
                            text: t.to_string(),
                            byte_start: (body_offset + s) as u64,
                            byte_end: (body_offset + end) as u64,
                        });
                    }
                }
                buf.clear();
                *start = None;
            };
        for (line, start) in lines {
            if line.chars().count() > max_chars {
                flush(&mut buf, &mut buf_start, buf_end, &mut out);
                // Hard-split an over-long line on char boundaries.
                let mut seg_start = 0usize;
                let mut count = 0usize;
                for (i, _) in line.char_indices() {
                    if count == max_chars {
                        buf.push_str(&line[seg_start..i]);
                        buf_start = Some(start + seg_start);
                        flush(&mut buf, &mut buf_start, start + i, &mut out);
                        seg_start = i;
                        count = 0;
                    }
                    count += 1;
                }
                buf.push_str(&line[seg_start..]);
                buf_start = Some(start + seg_start);
                buf_end = start + line.len();
                continue;
            }
            if buf.chars().count() + line.chars().count() > max_chars && !buf.trim().is_empty() {
                flush(&mut buf, &mut buf_start, buf_end, &mut out);
            }
            if buf_start.is_none() {
                buf_start = Some(start);
            }
            buf.push_str(line);
            buf_end = start + line.len();
        }
        flush(&mut buf, &mut buf_start, buf_end, &mut out);
    }
    out
}

/// A file's title: the first Markdown/AsciiDoc H1, else frontmatter `title`,
/// else the file stem.
pub fn title_of(rel_path: &str, fm_title: Option<&str>, body: &str) -> String {
    let mut in_fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for p in ["# ", "= "] {
            if let Some(h) = t.strip_prefix(p) {
                if !h.trim().is_empty() {
                    return h.trim().to_string();
                }
            }
        }
    }
    if let Some(t) = fm_title.map(str::trim).filter(|t| !t.is_empty()) {
        return t.trim_matches('"').to_string();
    }
    let file = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_headings_with_breadcrumbs_and_spans() {
        let text = "# Doc\nintro\n\n## Alpha\nalpha body\n\n## Beta\nbeta body\n";
        let c = chunk_text("Doc", text, 10, 1200);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].breadcrumb, "Doc");
        assert_eq!(c[1].breadcrumb, "Doc \u{203a} Alpha");
        assert!(c[1].text.contains("alpha body"));
        assert_eq!(
            &text[(c[2].byte_start - 10) as usize..(c[2].byte_end - 10) as usize],
            "## Beta\nbeta body\n"
        );
    }

    #[test]
    fn headings_inside_fences_do_not_split() {
        let text = "# T\n```\n# not a heading\n```\nafter\n";
        assert_eq!(chunk_text("T", text, 0, 1200).len(), 1);
    }

    #[test]
    fn long_sections_split_under_the_cap_and_cover_all_text() {
        let line = "word ".repeat(20) + "\n"; // 101 chars
        let text = format!("# T\n{}", line.repeat(30));
        let c = chunk_text("T", &text, 0, 300);
        assert!(c.len() > 5);
        for ch in &c {
            assert!(ch.text.chars().count() <= 300, "{}", ch.text.len());
        }
        let total: usize = c.iter().map(|x| x.text.matches("word").count()).sum();
        assert_eq!(total, 600);
    }

    #[test]
    fn over_long_single_line_is_hard_split() {
        let text = "x".repeat(950);
        let c = chunk_text("T", &text, 0, 400);
        assert_eq!(c.len(), 3);
        assert_eq!(c.iter().map(|x| x.text.len()).sum::<usize>(), 950);
        assert_eq!(c[2].byte_end, 950);
    }

    #[test]
    fn asciidoc_headings_split() {
        let c = chunk_text("Guide", "= Guide\nintro\n== Part\nbody\n", 0, 1200);
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].breadcrumb, "Guide \u{203a} Part");
    }

    #[test]
    fn title_prefers_h1_then_frontmatter_then_stem() {
        assert_eq!(title_of("a/b.md", Some("FM"), "# H\n"), "H");
        assert_eq!(title_of("a/b.md", Some("FM"), "body"), "FM");
        assert_eq!(title_of("a/b.md", None, "body"), "b");
        assert_eq!(title_of("a/x.adoc", None, "= Ascii\n"), "Ascii");
    }
}
