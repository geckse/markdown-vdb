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
                    Error::Io(e.into_io_error().unwrap_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::Other, msg)
                    }))
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
                let relative = path
                    .strip_prefix(&self.project_root)
                    .map_err(|_| Error::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("path {} is not under project root {}", path.display(), self.project_root.display()),
                    )))?;

                results.push(relative.to_path_buf());
            }
        }

        results.sort();
        results.dedup();
        Ok(results)
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
            builder.add(&negated).map_err(|e| {
                Error::Config(format!("invalid ignore pattern '{pattern}': {e}"))
            })?;
        }

        builder.build().map_err(|e| {
            Error::Config(format!("failed to build override rules: {e}"))
        })
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
            assert!(pattern.starts_with('!'), "pattern should start with '!': {pattern}");
        }
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
