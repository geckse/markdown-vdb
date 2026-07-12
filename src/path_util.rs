//! Path string normalization helpers.
//!
//! ALL file paths stored in the index (and surfaced through the JSON API) are
//! project-root-relative, `/`-separated strings. These helpers are the single
//! source of truth for converting OS paths (which use `\` on Windows) and
//! caller-supplied path strings into that canonical form.

use std::path::Path;

/// Convert a path to a `/`-separated string (lossy on non-UTF-8).
///
/// Apply at every `PathBuf` → `String` boundary that feeds the index, the FTS
/// index, the link graph, or the JSON API.
pub fn to_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Normalize a path string received from an external caller: `\` → `/`.
///
/// Defense-in-depth for `&str` library API entry points so Windows-style
/// inputs match the `/`-separated keys stored in the index.
pub fn normalize_path_input(s: &str) -> String {
    s.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn to_slash_converts_backslashes() {
        assert_eq!(to_slash(&PathBuf::from(r"docs\note.md")), "docs/note.md");
    }

    #[test]
    fn to_slash_idempotent_on_forward_slashes() {
        assert_eq!(to_slash(&PathBuf::from("docs/note.md")), "docs/note.md");
    }

    #[test]
    fn to_slash_mixed_separators() {
        assert_eq!(
            to_slash(&PathBuf::from(r"docs\sub/deep\note.md")),
            "docs/sub/deep/note.md"
        );
    }

    #[test]
    fn to_slash_empty_path() {
        assert_eq!(to_slash(Path::new("")), "");
    }

    #[test]
    fn normalize_path_input_converts_backslashes() {
        assert_eq!(normalize_path_input(r"docs\note.md"), "docs/note.md");
    }

    #[test]
    fn normalize_path_input_idempotent_on_forward_slashes() {
        assert_eq!(normalize_path_input("docs/note.md"), "docs/note.md");
    }

    #[test]
    fn normalize_path_input_mixed_separators() {
        assert_eq!(
            normalize_path_input(r"docs\sub/deep\note.md"),
            "docs/sub/deep/note.md"
        );
    }

    #[test]
    fn normalize_path_input_empty() {
        assert_eq!(normalize_path_input(""), "");
    }
}
