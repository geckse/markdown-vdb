use std::path::PathBuf;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// A parsed markdown file with extracted metadata.
#[derive(Debug, Clone, Serialize)]
pub struct MarkdownFile {
    /// Relative path to the markdown file.
    pub path: PathBuf,
    /// YAML frontmatter parsed as dynamic JSON value, if present.
    pub frontmatter: Option<serde_json::Value>,
    /// Headings extracted from the document.
    pub headings: Vec<Heading>,
    /// Raw body content (everything after frontmatter).
    pub body: String,
    /// SHA-256 hex digest of the full file content.
    pub content_hash: String,
}

/// A heading extracted from a markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct Heading {
    /// Heading level (1-6).
    pub level: u8,
    /// The text content of the heading.
    pub text: String,
    /// 1-based line number where the heading appears.
    pub line_number: usize,
}

/// Compute a SHA-256 hex digest of the given content.
pub fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_deterministic() {
        let hash1 = compute_content_hash("hello world");
        let hash2 = compute_content_hash("hello world");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn content_hash_length() {
        let hash = compute_content_hash("test content");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn content_hash_hex_chars() {
        let hash = compute_content_hash("test");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn content_hash_content_sensitive() {
        let hash1 = compute_content_hash("content a");
        let hash2 = compute_content_hash("content b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn content_hash_empty_string() {
        let hash = compute_content_hash("");
        assert_eq!(hash.len(), 64);
    }
}
