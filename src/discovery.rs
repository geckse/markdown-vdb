use std::path::{Path, PathBuf};

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use tracing::debug;

use crate::config::Config;
use crate::error::{Error, Result};

/// Directories that are always excluded from file discovery.
pub const BUILTIN_IGNORE_PATTERNS: &[&str] = &[
    "!.claude/",
    "!.cursor/",
    "!.vscode/",
    "!.idea/",
    "!.git/",
    "!node_modules/",
    "!.obsidian/",
    "!__pycache__/",
    "!.next/",
    "!.nuxt/",
    "!.svelte-kit/",
    "!target/",
    "!dist/",
    "!build/",
    "!out/",
];

/// Discovers markdown files in configured source directories, applying
/// gitignore rules, built-in ignore patterns, and user-configured patterns.
#[derive(Debug)]
pub struct FileDiscovery {
    source_dirs: Vec<PathBuf>,
    ignore_patterns: Vec<String>,
    project_root: PathBuf,
}

impl FileDiscovery {
    /// Create a new `FileDiscovery` from a project root and config.
    pub fn new(project_root: &Path, config: &Config) -> Self {
        Self {
            source_dirs: config.source_dirs.clone(),
            ignore_patterns: config.ignore_patterns.clone(),
            project_root: project_root.to_path_buf(),
        }
    }

    /// Discover all `.md` files in the configured source directories.
    ///
    /// Returns a sorted `Vec<PathBuf>` of paths relative to the project root.
    pub fn discover(&self) -> Result<Vec<PathBuf>> {
        let mut results = Vec::new();

        for source_dir in &self.source_dirs {
            let abs_dir = self.project_root.join(source_dir);
            if !abs_dir.is_dir() {
                debug!("skipping non-existent source dir: {}", abs_dir.display());
                continue;
            }

            let overrides = self.build_overrides(&abs_dir)?;

            let walker = WalkBuilder::new(&abs_dir)
                .standard_filters(true)
                .overrides(overrides)
                .build();

            for entry in walker {
                let entry = entry.map_err(|e| {
                    let msg = e.to_string();
                    Error::Io(
                        e.into_io_error()
                            .unwrap_or_else(|| std::io::Error::other(msg)),
                    )
                })?;

                let path = entry.path();

                // Only include .md files
                if !path.is_file() {
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }

                // Convert to relative path from project root
                let relative = path.strip_prefix(&self.project_root).map_err(|_| {
                    Error::Io(std::io::Error::other(format!(
                        "path {} is not under project root {}",
                        path.display(),
                        self.project_root.display()
                    )))
                })?;

                results.push(relative.to_path_buf());
            }
        }

        results.sort();
        results.dedup();
        Ok(results)
    }

    /// Check whether a relative path should be indexed.
    ///
    /// Returns `true` if the path has a `.md` extension, is not under any
    /// built-in ignored directory, and does not match any custom ignore pattern.
    /// Used by the file watcher to filter filesystem events.
    pub fn should_index(&self, relative_path: &Path) -> bool {
        // Must have .md extension
        if relative_path.extension().and_then(|e| e.to_str()) != Some("md") {
            return false;
        }

        // Check against built-in ignored directories
        for pattern in BUILTIN_IGNORE_PATTERNS {
            // Patterns are like "!.git/" — strip the "!" and trailing "/"
            let dir_name = pattern.trim_start_matches('!').trim_end_matches('/');
            for component in relative_path.components() {
                if let std::path::Component::Normal(c) = component {
                    if c == dir_name {
                        return false;
                    }
                }
            }
        }

        // Check against user-configured ignore patterns
        let path_str = relative_path.to_string_lossy();
        for pattern in &self.ignore_patterns {
            let pat = if let Some(stripped) = pattern.strip_prefix('!') {
                stripped
            } else {
                pattern.as_str()
            };

            // Check if pattern matches the full path or any component
            if path_str.contains(pat.trim_end_matches('/')) {
                return false;
            }
        }

        true
    }

    /// Build override rules combining built-in patterns and user-configured patterns.
    fn build_overrides(&self, dir: &Path) -> Result<ignore::overrides::Override> {
        let mut builder = OverrideBuilder::new(dir);

        // Add built-in ignore patterns (negation patterns exclude directories)
        for pattern in BUILTIN_IGNORE_PATTERNS {
            builder.add(pattern).map_err(|e| {
                Error::Config(format!("invalid built-in ignore pattern '{pattern}': {e}"))
            })?;
        }

        // Add user-configured ignore patterns as negations
        for pattern in &self.ignore_patterns {
            let negated = if pattern.starts_with('!') {
                pattern.clone()
            } else {
                format!("!{pattern}")
            };
            builder
                .add(&negated)
                .map_err(|e| Error::Config(format!("invalid ignore pattern '{pattern}': {e}")))?;
        }

        builder
            .build()
            .map_err(|e| Error::Config(format!("failed to build override rules: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_patterns_count() {
        assert_eq!(BUILTIN_IGNORE_PATTERNS.len(), 15);
    }

    #[test]
    fn builtin_patterns_are_negations() {
        for pattern in BUILTIN_IGNORE_PATTERNS {
            assert!(
                pattern.starts_with('!'),
                "pattern should start with '!': {pattern}"
            );
        }
    }

    fn make_discovery(ignore_patterns: Vec<String>) -> FileDiscovery {
        FileDiscovery {
            source_dirs: vec![PathBuf::from(".")],
            ignore_patterns,
            project_root: PathBuf::from("/tmp/test"),
        }
    }

    #[test]
    fn should_index_accepts_md_files() {
        let fd = make_discovery(vec![]);
        assert!(fd.should_index(Path::new("docs/readme.md")));
        assert!(fd.should_index(Path::new("notes.md")));
    }

    #[test]
    fn should_index_rejects_non_md_files() {
        let fd = make_discovery(vec![]);
        assert!(!fd.should_index(Path::new("readme.txt")));
        assert!(!fd.should_index(Path::new("src/main.rs")));
        assert!(!fd.should_index(Path::new("file")));
    }

    #[test]
    fn should_index_rejects_builtin_ignored_dirs() {
        let fd = make_discovery(vec![]);
        assert!(!fd.should_index(Path::new(".git/hooks/readme.md")));
        assert!(!fd.should_index(Path::new("node_modules/pkg/readme.md")));
        assert!(!fd.should_index(Path::new("target/debug/notes.md")));
        assert!(!fd.should_index(Path::new(".claude/docs/notes.md")));
        assert!(!fd.should_index(Path::new("dist/readme.md")));
    }

    #[test]
    fn should_index_rejects_custom_ignore_patterns() {
        let fd = make_discovery(vec!["drafts/".to_string()]);
        assert!(!fd.should_index(Path::new("drafts/wip.md")));
        assert!(fd.should_index(Path::new("docs/readme.md")));
    }

    #[test]
    fn should_index_handles_custom_pattern_with_bang() {
        let fd = make_discovery(vec!["!private/".to_string()]);
        assert!(!fd.should_index(Path::new("private/secret.md")));
    }

    #[test]
    fn builtin_patterns_contain_expected_dirs() {
        let expected = [
            "!.claude/",
            "!.cursor/",
            "!.vscode/",
            "!.idea/",
            "!.git/",
            "!node_modules/",
            "!.obsidian/",
            "!__pycache__/",
            "!.next/",
            "!.nuxt/",
            "!.svelte-kit/",
            "!target/",
            "!dist/",
            "!build/",
            "!out/",
        ];
        for dir in &expected {
            assert!(
                BUILTIN_IGNORE_PATTERNS.contains(dir),
                "missing expected pattern: {dir}"
            );
        }
    }
}
