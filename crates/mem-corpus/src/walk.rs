//! Deterministic, symlink-refusing file access for the corpus build.
//!
//! Every path the build reads is (a) relative, with no `..` or absolute
//! component, (b) a regular file reached without following a symlink at any
//! level below the source root, and (c) at most [`MAX_FILE_BYTES`]. Directory
//! listings are sorted byte-wise so two hosts walk the same tree in the same
//! order.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Largest single file the corpus build reads. Larger files are skipped and
/// listed in the manifest, never truncated.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Deepest directory nesting a docs walk descends.
pub const MAX_DEPTH: usize = 12;

/// Refuse anything but a plain relative path (no `..`, no root, no prefix).
pub fn check_relative(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("empty path".into());
    }
    if p.contains('\\') {
        return Err(format!("{p:?} uses a backslash; use `/`"));
    }
    for c in Path::new(p).components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("{p:?} must be relative and stay inside its base")),
        }
    }
    Ok(())
}

/// Why a candidate file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Missing,
    NotRegularFile,
    TooLarge(u64),
    Unreadable(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Missing => write!(f, "missing"),
            Refusal::NotRegularFile => {
                write!(f, "not a regular file (symlink, device or directory)")
            }
            Refusal::TooLarge(n) => write!(f, "too large ({n} bytes > {MAX_FILE_BYTES})"),
            Refusal::Unreadable(e) => write!(f, "unreadable: {e}"),
        }
    }
}

/// Read `root/rel` if every component below `root` is a real directory or a
/// regular file (no symlink is ever followed) and the file fits the cap.
pub fn read_regular(root: &Path, rel: &str) -> Result<Vec<u8>, Refusal> {
    check_relative(rel).map_err(Refusal::Unreadable)?;
    let mut cur = root.to_path_buf();
    let comps: Vec<_> = Path::new(rel).components().collect();
    for (i, c) in comps.iter().enumerate() {
        let Component::Normal(name) = c else { continue };
        cur.push(name);
        let meta = match std::fs::symlink_metadata(&cur) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Refusal::Missing),
            Err(e) => return Err(Refusal::Unreadable(e.to_string())),
        };
        let last = i + 1 == comps.len();
        let ok = if last {
            meta.file_type().is_file()
        } else {
            meta.file_type().is_dir()
        };
        if !ok {
            return Err(Refusal::NotRegularFile);
        }
        if last && meta.len() > MAX_FILE_BYTES {
            return Err(Refusal::TooLarge(meta.len()));
        }
    }
    let mut buf = Vec::new();
    std::fs::File::open(&cur)
        .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_end(&mut buf))
        .map_err(|e| Refusal::Unreadable(e.to_string()))?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return Err(Refusal::TooLarge(buf.len() as u64));
    }
    Ok(buf)
}

/// Whether `root` exists as a real directory (not a symlink to one).
pub fn is_real_dir(root: &Path) -> bool {
    std::fs::symlink_metadata(root)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

/// A walk result: the files to read, and `(path, reason)` for each one left out.
pub type Listing = (Vec<String>, Vec<(String, String)>);

/// Every regular file under `root` whose extension is in `extensions`
/// (case-insensitive), skipping directories named in `exclude_dirs` and any
/// dot-directory. Returned as sorted `/`-separated relative paths. Symlinks
/// (to files or directories) are listed in `skipped` and never followed.
pub fn list_files(
    root: &Path,
    extensions: &[String],
    exclude_dirs: &[String],
) -> Result<Listing, String> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    walk(
        root,
        "",
        0,
        extensions,
        exclude_dirs,
        &mut files,
        &mut skipped,
    )?;
    files.sort();
    skipped.sort();
    Ok((files, skipped))
}

fn walk(
    dir: &Path,
    prefix: &str,
    depth: usize,
    extensions: &[String],
    exclude_dirs: &[String],
    files: &mut Vec<String>,
    skipped: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        skipped.push((
            prefix.to_string(),
            format!("deeper than {MAX_DEPTH} levels"),
        ));
        return Ok(());
    }
    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(|n| (n.to_string(), e.path())))
        .collect();
    entries.sort();
    for (name, path) in entries {
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("stat {rel}: {e}"))?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            if has_ext(&name, extensions) {
                skipped.push((rel, "symlink (not followed)".into()));
            }
            continue;
        }
        if ft.is_dir() {
            if name.starts_with('.') || exclude_dirs.iter().any(|d| d == &name) {
                continue;
            }
            walk(
                &path,
                &rel,
                depth + 1,
                extensions,
                exclude_dirs,
                files,
                skipped,
            )?;
        } else if ft.is_file() && has_ext(&name, extensions) {
            files.push(rel);
        }
    }
    Ok(())
}

fn has_ext(name: &str, extensions: &[String]) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_check() {
        assert!(check_relative("a/b.md").is_ok());
        assert!(check_relative("./a").is_ok());
        for bad in ["", "../a", "/a", "a/../../b", "a\\b"] {
            assert!(check_relative(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn extension_match_is_case_insensitive_and_needs_a_stem() {
        let md = vec!["md".to_string()];
        assert!(has_ext("A.MD", &md));
        assert!(!has_ext(".md", &md));
        assert!(!has_ext("readme", &md));
    }
}
