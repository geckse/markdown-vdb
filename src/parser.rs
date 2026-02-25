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

/// Extract YAML frontmatter from markdown content.
///
/// Frontmatter must appear at the very start of the file, delimited by `---` lines.
/// Returns `None` if no frontmatter is present or if it is malformed.
pub fn extract_frontmatter(content: &str) -> (Option<serde_json::Value>, &str) {
    let trimmed = content.trim_start_matches('\u{feff}'); // strip BOM
    if !trimmed.starts_with("---") {
        return (None, content);
    }

    // Find the opening delimiter line end
    let after_open = match trimmed[3..].find('\n') {
        Some(i) => 3 + i + 1,
        None => return (None, content),
    };

    // Only whitespace allowed after opening ---
    if !trimmed[3..after_open].trim().is_empty() {
        return (None, content);
    }

    // Find closing ---
    let rest = &trimmed[after_open..];
    let closing_pos = rest.find("\n---").or_else(|| {
        // Handle case where --- is at the very start of rest (empty frontmatter)
        if rest.starts_with("---") {
            Some(0)
        } else {
            None
        }
    });

    let closing_pos = match closing_pos {
        Some(p) => p,
        None => {
            tracing::warn!("frontmatter missing closing ---");
            return (None, content);
        }
    };

    let yaml_str = if closing_pos == 0 && rest.starts_with("---") {
        ""
    } else {
        &rest[..closing_pos]
    };

    // Find where the body starts (after closing --- line)
    let after_closing_start = after_open + closing_pos + if closing_pos == 0 && rest.starts_with("---") { 0 } else { 1 };
    let after_closing = &trimmed[after_closing_start..];
    let body_start = match after_closing.find('\n') {
        Some(i) => after_closing_start + i + 1,
        None => trimmed.len(),
    };
    let body = &trimmed[body_start..];

    let yaml_trimmed = yaml_str.trim();
    if yaml_trimmed.is_empty() {
        return (None, body);
    }

    match serde_yaml::from_str::<serde_yaml::Value>(yaml_trimmed) {
        Ok(yaml_val) => {
            let json_val = yaml_to_json(yaml_val);
            (Some(json_val), body)
        }
        Err(e) => {
            tracing::warn!("failed to parse frontmatter YAML: {e}");
            (None, body)
        }
    }
}

/// Convert a serde_yaml::Value to serde_json::Value.
fn yaml_to_json(val: serde_yaml::Value) -> serde_json::Value {
    match val {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            }
        }
        serde_yaml::Value::String(s) => serde_json::Value::String(s),
        serde_yaml::Value::Sequence(seq) => {
            serde_json::Value::Array(seq.into_iter().map(yaml_to_json).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let obj = map
                .into_iter()
                .filter_map(|(k, v)| {
                    let key = match k {
                        serde_yaml::Value::String(s) => s,
                        other => serde_yaml::to_string(&other).ok()?.trim().to_string(),
                    };
                    Some((key, yaml_to_json(v)))
                })
                .collect();
            serde_json::Value::Object(obj)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_json(tagged.value),
    }
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

    // --- extract_frontmatter tests ---

    #[test]
    fn extract_frontmatter_basic() {
        let content = "---\ntitle: Hello\ntags:\n  - rust\n---\nBody here";
        let (fm, body) = extract_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm["title"], "Hello");
        assert_eq!(fm["tags"][0], "rust");
        assert_eq!(body, "Body here");
    }

    #[test]
    fn extract_frontmatter_none_when_missing() {
        let content = "# Just a heading\nSome text";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn extract_frontmatter_empty() {
        let content = "---\n---\nBody";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, "Body");
    }

    #[test]
    fn extract_frontmatter_missing_closing() {
        let content = "---\ntitle: Oops\nNo closing delimiter";
        let (fm, _body) = extract_frontmatter(content);
        assert!(fm.is_none());
    }

    #[test]
    fn extract_frontmatter_malformed_yaml() {
        let content = "---\n: :\n  - [invalid\n---\nBody";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, "Body");
    }

    #[test]
    fn extract_frontmatter_with_numbers() {
        let content = "---\ncount: 42\npi: 3.14\n---\n";
        let (fm, _) = extract_frontmatter(content);
        let fm = fm.unwrap();
        assert_eq!(fm["count"], 42);
    }

    #[test]
    fn extract_frontmatter_with_bom() {
        let content = "\u{feff}---\ntitle: BOM\n---\nBody";
        let (fm, body) = extract_frontmatter(content);
        assert_eq!(fm.unwrap()["title"], "BOM");
        assert_eq!(body, "Body");
    }

    // --- content_hash tests ---

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
