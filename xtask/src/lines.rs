//! Source-file line-limit enforcement for `cargo xtask lines:check`.
//!
//! The coding standard caps every source file at 500 lines (ADR-0009). This
//! module walks the workspace's Rust sources (`crates/` and `xtask/src`),
//! counts each file the same way `wc -l` does, and fails CI when a file grows
//! past [`MAX_LINES`]. Files that must exceed the limit are recorded in
//! [`EXEMPTIONS`] with a comment citing the governing ADR; the list starts empty
//! because ADR-0009's exception is a markdown document, not a `.rs` source.
//!
//! `target/`, any `generated/` tree, and `.agents/` are skipped: they hold
//! build artifacts, codegen output, and historical agent records respectively,
//! none of which the standard binds.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::FAILURE;

/// Maximum number of lines permitted in a single source file (ADR-0009).
const MAX_LINES: usize = 500;

/// Repository-relative source roots scanned by the check.
const SCAN_ROOTS: [&str; 2] = ["crates", "xtask/src"];

/// Repository-relative paths exempt from the line limit.
///
/// Empty by design: ADR-0009's documented exception covers
/// `.agents/documents/requirements/fsd.md`, a markdown document outside the
/// scanned `.rs` roots. A `.rs` file may be added here only when it genuinely
/// cannot be split, and each entry must carry a comment citing the reason.
const EXEMPTIONS: &[&str] = &[];

/// A directory name pruned during the walk (build output, codegen, archives).
const SKIPPED_DIRS: [&str; 3] = ["target", "generated", ".agents"];

/// One problem found while scanning: either an over-limit file or one that
/// could not be read.
enum Violation {
    /// The file has more than [`MAX_LINES`] lines.
    OverLimit {
        /// Repository-relative path shown to the user.
        path: String,
        /// Line count, measured like `wc -l`.
        lines: usize,
    },
    /// The file could not be read.
    Unreadable {
        /// Repository-relative path shown to the user.
        path: String,
        /// Underlying I/O error rendered as a string.
        error: String,
    },
}

/// Run the line-limit check across the workspace source roots.
///
/// Prints each offending file as `path:lines` followed by a summary and returns
/// [`FAILURE`] when any file exceeds [`MAX_LINES`]; prints a success line with
/// the number of files checked and returns `0` otherwise.
pub fn run() -> i32 {
    let root = workspace_root();
    let mut files: Vec<PathBuf> = Vec::new();
    for relative in SCAN_ROOTS {
        collect_rust_files(&root.join(relative), &mut files);
    }
    files.sort();

    let violations = check_paths(&root, &files);
    let over_limit = violations
        .iter()
        .filter(|violation| matches!(violation, Violation::OverLimit { .. }))
        .count();

    if violations.is_empty() {
        println!(
            "xtask: all source files within the 500-line limit ({} files checked).",
            files.len()
        );
        return 0;
    }

    for violation in &violations {
        match violation {
            Violation::OverLimit { path, lines } => eprintln!("{path}:{lines}"),
            Violation::Unreadable { path, error } => {
                eprintln!("xtask: cannot read {path}: {error}");
            }
        }
    }
    if over_limit > 0 {
        eprintln!("xtask: {over_limit} file(s) exceed the 500-line limit");
    }
    FAILURE
}

/// Resolve the workspace root from this binary's compile-time manifest path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Recursively collect `*.rs` files under `dir`, pruning [`SKIPPED_DIRS`].
///
/// Directories are visited in sorted order so the result is deterministic.
fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if !is_skipped_dir(&path) {
                collect_rust_files(&path, files);
            }
        } else if is_rust_file(&path) {
            files.push(path);
        }
    }
}

/// Whether `path` is a directory pruned from the scan.
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SKIPPED_DIRS.contains(&name))
}

/// Whether `path` has the `.rs` extension.
fn is_rust_file(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}

/// Check `paths` (absolute) against [`MAX_LINES`] using [`EXEMPTIONS`].
fn check_paths(root: &Path, paths: &[PathBuf]) -> Vec<Violation> {
    check_paths_with_exemptions(root, paths, EXEMPTIONS)
}

/// Check `paths` against [`MAX_LINES`], skipping `exemptions`.
///
/// Each path is rendered repository-relative against `root` for display and
/// exemption matching; a path outside `root` is shown verbatim. Split from
/// [`check_paths`] so the exemption list is injectable in tests.
fn check_paths_with_exemptions(
    root: &Path,
    paths: &[PathBuf],
    exemptions: &[&str],
) -> Vec<Violation> {
    let mut violations = Vec::new();
    for path in paths {
        let display = relative_display(root, path);
        if is_exempt(&display, exemptions) {
            continue;
        }
        match count_lines(path) {
            Ok(lines) if lines > MAX_LINES => {
                violations.push(Violation::OverLimit {
                    path: display,
                    lines,
                });
            }
            Ok(_) => {}
            Err(error) => violations.push(Violation::Unreadable {
                path: display,
                error: error.to_string(),
            }),
        }
    }
    violations
}

/// Render `path` relative to `root` with forward slashes, else verbatim.
fn relative_display(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
}

/// Whether a repository-relative `display` path is covered by `exemptions`.
fn is_exempt(display: &str, exemptions: &[&str]) -> bool {
    exemptions
        .iter()
        .any(|exempt| display == *exempt || display.ends_with(&format!("/{exempt}")))
}

/// Count the lines in `path` the way `wc -l` does: newline bytes.
fn count_lines(path: &Path) -> io::Result<usize> {
    let source = fs::read(path)?;
    Ok(source.iter().filter(|byte| **byte == b'\n').count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Create a unique temp directory for a single test case.
    fn temp_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustasea-lines-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    /// Write `lines` newline-terminated lines to `root/relative`.
    fn write_fixture(root: &Path, relative: &str, lines: usize) -> PathBuf {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        let body = "// fixture\n".repeat(lines);
        fs::write(&path, body).expect("write fixture");
        path
    }

    /// A file at exactly the limit is accepted.
    #[test]
    fn file_at_limit_passes() {
        let root = temp_root();
        let path = write_fixture(&root, "at_limit.rs", MAX_LINES);
        assert!(check_paths(&root, std::slice::from_ref(&path)).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    /// A file one line over the limit is reported with its count.
    #[test]
    fn file_over_limit_fails() {
        let root = temp_root();
        let path = write_fixture(&root, "over_limit.rs", MAX_LINES + 1);
        let violations = check_paths(&root, std::slice::from_ref(&path));
        assert_eq!(violations.len(), 1);
        match &violations[0] {
            Violation::OverLimit { path, lines } => {
                assert_eq!(path, "over_limit.rs");
                assert_eq!(*lines, MAX_LINES + 1);
            }
            Violation::Unreadable { .. } => panic!("expected an over-limit violation"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// An exempt path is skipped even when it exceeds the limit.
    #[test]
    fn exempt_path_is_skipped() {
        let root = temp_root();
        let path = write_fixture(&root, "sub/over_limit.rs", MAX_LINES + 1);
        let exemptions = ["sub/over_limit.rs"];
        let violations =
            check_paths_with_exemptions(&root, std::slice::from_ref(&path), &exemptions);
        assert!(violations.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    /// The walker collects `.rs` files but prunes `target/` and `generated/`.
    #[test]
    fn walker_prunes_skipped_dirs() {
        let root = temp_root();
        write_fixture(&root, "src/keep.rs", 1);
        write_fixture(&root, "target/build.rs", 1);
        write_fixture(&root, "src/generated/skip.rs", 1);
        write_fixture(&root, "src/notes.txt", 1);

        let mut files = Vec::new();
        collect_rust_files(&root, &mut files);
        let found: Vec<String> = files
            .iter()
            .map(|path| relative_display(&root, path))
            .collect();
        assert_eq!(found, vec!["src/keep.rs".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    /// An unreadable path is surfaced rather than silently skipped.
    #[test]
    fn unreadable_path_is_reported() {
        let root = temp_root();
        let missing = root.join("does_not_exist.rs");
        let violations = check_paths(&root, std::slice::from_ref(&missing));
        assert_eq!(violations.len(), 1);
        assert!(matches!(violations[0], Violation::Unreadable { .. }));
        let _ = fs::remove_dir_all(&root);
    }
}
