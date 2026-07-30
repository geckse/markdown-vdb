//! Path string normalization helpers.
//!
//! ALL file paths stored in the index (and surfaced through the JSON API) are
//! project-root-relative, `/`-separated strings. These helpers are the single
//! source of truth for converting OS paths (which use `\` on Windows) and
//! caller-supplied path strings into that canonical form.

use std::borrow::Cow;
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

/// Return whether `path` is exactly `scope` or is a descendant of it.
///
/// Both arguments accept the collection's canonical `/` separators as well as
/// Windows `\` separators. A trailing separator on the scope is ignored, and
/// `""`, `"."`, and `"./"` represent the collection root.
///
/// This is deliberately segment-aware: the scope `docs` contains
/// `docs/guide.md`, but never `docs-old/guide.md`.
pub fn path_is_in_scope(path: &str, scope: &str) -> bool {
    fn slash_normalized(value: &str) -> Cow<'_, str> {
        if value.contains('\\') {
            Cow::Owned(value.replace('\\', "/"))
        } else {
            Cow::Borrowed(value)
        }
    }

    fn trim_relative_root(value: &str) -> &str {
        value.strip_prefix("./").unwrap_or(value)
    }

    let path = slash_normalized(path);
    let scope = slash_normalized(scope);
    let path = trim_relative_root(&path).trim_end_matches('/');
    let scope = trim_relative_root(&scope).trim_end_matches('/');

    if scope.is_empty() || scope == "." {
        return true;
    }

    path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|suffix| suffix.starts_with('/'))
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

    #[test]
    fn path_scope_matches_exact_path_and_descendants() {
        assert!(path_is_in_scope("docs", "docs"));
        assert!(path_is_in_scope("docs/guide.md", "docs"));
        assert!(path_is_in_scope("docs/api/auth.md", "docs/"));
    }

    #[test]
    fn path_scope_is_segment_safe() {
        assert!(!path_is_in_scope("docs-old/guide.md", "docs"));
        assert!(!path_is_in_scope("documentation/guide.md", "docs"));
        assert!(!path_is_in_scope("nested/docs/guide.md", "docs"));
    }

    #[test]
    fn path_scope_accepts_windows_separators() {
        assert!(path_is_in_scope(r"docs\api\auth.md", r"docs\api"));
        assert!(path_is_in_scope("docs/api/auth.md", r"docs\api\\"));
        assert!(!path_is_in_scope(r"docs-old\auth.md", r"docs"));
    }

    #[test]
    fn path_scope_root_forms_match_everything() {
        for root in ["", ".", "./"] {
            assert!(path_is_in_scope("docs/guide.md", root));
        }
    }
}
