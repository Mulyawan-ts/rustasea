//! Source scanning for `lang:check` — literal translation-key extraction.
//!
//! The checker flags dictionary keys that are never referenced by a helper
//! call. Rather than pull in a parser (or a regex engine), this module walks
//! the configured source roots, reads every `*.rs` file, and extracts the first
//! string literal of the four translation helpers (`__`, `trans_choice`,
//! `i18n_trans`, `i18n_trans_choice`).
//!
//! The scan is a heuristic: it does not understand comments, macros, or
//! dynamically built keys. That is acceptable because "unused" is an
//! informational finding that never fails the command (see [`super`]).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Helper names whose first string-literal argument is a translation key.
///
/// The bare `.trans(` method is intentionally excluded: it is used for
/// already-resolved strings in tests, not for dictionary lookups.
const CALL_PREFIXES: [&str; 4] = ["__(", "trans_choice(", "i18n_trans(", "i18n_trans_choice("];

/// Scan every `*.rs` file under `roots` and return the referenced keys.
///
/// Non-existent roots are skipped so callers can pass candidate directories
/// (`crates`, `app`, `routes`) without probing each one first.
pub fn referenced_keys(roots: &[PathBuf]) -> HashSet<String> {
    let mut files = Vec::new();
    for root in roots {
        if root.is_dir() {
            collect_rust_files(root, &mut files);
        }
    }

    let mut keys = HashSet::new();
    for file in files {
        if let Ok(source) = std::fs::read_to_string(&file) {
            keys.extend(extract_keys(&source));
        }
    }
    keys
}

/// Recursively collect `*.rs` files under `root`, sorted for determinism.
fn collect_rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();

    for path in paths {
        if path.is_dir() {
            if is_skipped_dir(&path) {
                continue;
            }
            collect_rust_files(&path, out);
        } else if path.extension().and_then(OsStr::to_str) == Some("rs") {
            out.push(path);
        }
    }
}

/// Whether a directory should be pruned from the walk.
///
/// Hidden directories (leading `.`, e.g. `.git`) and build-output directories
/// (`target`) never hold hand-written sources, so descending into them only
/// wastes I/O and can surface stale generated `*.rs` files.
fn is_skipped_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(OsStr::to_str),
        Some(name) if name.starts_with('.') || name == "target"
    )
}

/// Extract every translation key referenced by the helper calls in `source`.
///
/// Only non-empty string literals immediately following one of
/// [`CALL_PREFIXES`] (modulo whitespace) are recorded. Scanning resumes after
/// each literal so a key's own text is never re-matched.
pub fn extract_keys(source: &str) -> HashSet<String> {
    let mut keys = HashSet::new();

    for prefix in CALL_PREFIXES {
        let mut search = 0usize;
        while let Some(offset) = source[search..].find(prefix) {
            let after = search + offset + prefix.len();
            match read_string_literal(source, after) {
                Some((literal, next)) => {
                    if !literal.is_empty() {
                        keys.insert(literal);
                    }
                    search = next;
                }
                None => search = after,
            }
        }
    }
    keys
}

/// Read a `"..."` literal starting at or after `index`, honouring `\"` escapes.
///
/// Returns the decoded literal and the byte index just past its closing quote,
/// or `None` when the next non-whitespace character is not a quote or the
/// literal is unterminated.
fn read_string_literal(source: &str, mut index: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if bytes.get(index) != Some(&b'"') {
        return None;
    }
    index += 1;

    let mut value = String::new();
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                let ch = source[index..].chars().next()?;
                value.push(ch);
                index += ch.len_utf8();
            }
            b'"' => return Some((value, index + 1)),
            _ => {
                let ch = source[index..].chars().next()?;
                value.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Monotonic counter so parallel runs never collide on a temp path.
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    /// Hidden and `target` directories are pruned; normal sources are kept.
    #[test]
    fn skips_hidden_and_target_directories() {
        let unique = SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "rustasea-langcheck-scan-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        let normal = root.join("src").join("nested").join("lib.rs");
        let hidden = root.join(".hidden").join("secret.rs");
        let target = root.join("target").join("debug").join("build.rs");
        for file in [&normal, &hidden, &target] {
            std::fs::create_dir_all(file.parent().expect("fixture parent"))
                .expect("create fixture directory");
            std::fs::write(file, "// fixture").expect("write fixture file");
        }

        let mut found = Vec::new();
        collect_rust_files(&root, &mut found);

        let _ = std::fs::remove_dir_all(&root);

        assert!(found.contains(&normal), "collected: {found:?}");
        assert!(
            !found.contains(&hidden),
            "hidden dir must be skipped: {found:?}"
        );
        assert!(
            !found.contains(&target),
            "target dir must be skipped: {found:?}"
        );
        assert_eq!(found.len(), 1, "collected: {found:?}");
    }

    /// Every supported helper form contributes its literal key.
    #[test]
    fn extracts_plain_and_namespaced_calls() {
        let source = r#"
            let a = __("auth.failed", &[]);
            let b = trans_choice("auth.items", 2, &[]);
            let c = i18n_trans("mail.subject");
            let d = i18n_trans_choice("cart.items", 3);
            let ignored = translator.trans("not.a.call");
        "#;
        let keys = extract_keys(source);
        assert!(keys.contains("auth.failed"), "keys: {keys:?}");
        assert!(keys.contains("auth.items"), "keys: {keys:?}");
        assert!(keys.contains("mail.subject"), "keys: {keys:?}");
        assert!(keys.contains("cart.items"), "keys: {keys:?}");
        assert!(!keys.contains("not.a.call"), "bare .trans must be skipped");
    }

    /// Escaped quotes inside a literal are decoded, not treated as its end.
    #[test]
    fn extracts_escaped_quotes() {
        let source = r#"__("he said \"hi\"", &[]);"#;
        let keys = extract_keys(source);
        assert!(keys.contains(r#"he said "hi""#), "keys: {keys:?}");
    }

    /// A non-literal first argument yields no key.
    #[test]
    fn ignores_non_literal_arguments() {
        let source = r#"__(variable, &[]);"#;
        assert!(extract_keys(source).is_empty());
    }
}
