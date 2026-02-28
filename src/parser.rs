use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::Error;

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
    /// File size in bytes.
    pub file_size: u64,
    /// Links extracted from the document body.
    pub links: Vec<RawLink>,
    /// Filesystem modification time as Unix timestamp (seconds since epoch).
    pub modified_at: u64,
}

/// A raw link extracted from a markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct RawLink {
    /// The target path of the link (relative to the markdown file or project root).
    pub target: String,
    /// The display text of the link.
    pub text: String,
    /// 1-based line number where the link appears.
    pub line_number: usize,
    /// Whether this is a wikilink (`[[...]]`) or standard markdown link.
    pub is_wikilink: bool,
}

/// Parse a markdown file from disk into a [`MarkdownFile`].
///
/// Reads the file at `project_root.join(relative_path)`, extracts frontmatter,
/// headings, content hash, and file size. Returns `Error::MarkdownParse` for
/// non-UTF-8 files.
pub fn parse_markdown_file(
    project_root: &Path,
    relative_path: &Path,
) -> Result<MarkdownFile, Error> {
    let full_path = project_root.join(relative_path);
    let raw_bytes = std::fs::read(&full_path)?;
    let file_size = raw_bytes.len() as u64;

    // Capture filesystem modification time.
    let modified_at = std::fs::metadata(&full_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let content = String::from_utf8(raw_bytes).map_err(|_| Error::MarkdownParse {
        path: relative_path.to_path_buf(),
        message: "file is not valid UTF-8".into(),
    })?;

    let content_hash = compute_content_hash(&content);
    let (frontmatter, body) = extract_frontmatter(&content);
    let headings = extract_headings(body);
    let links = extract_links(body);

    Ok(MarkdownFile {
        path: relative_path.to_path_buf(),
        frontmatter,
        headings,
        body: body.to_string(),
        content_hash,
        file_size,
        links,
        modified_at,
    })
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
    let after_closing_start = after_open
        + closing_pos
        + if closing_pos == 0 && rest.starts_with("---") {
            0
        } else {
            1
        };
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

/// Extract headings from markdown content.
///
/// Uses `pulldown_cmark::Parser` to find all headings (h1-h6) and returns them
/// with their text content and 1-based line numbers.
pub fn extract_headings(content: &str) -> Vec<Heading> {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

    let parser = Parser::new_ext(content, Options::all());
    let mut headings = Vec::new();
    let mut in_heading: Option<(u8, usize)> = None; // (level, byte_offset)
    let mut heading_text = String::new();

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let byte_offset = range.start;
                let level_num = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                in_heading = Some((level_num, byte_offset));
                heading_text.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, byte_offset)) = in_heading.take() {
                    let line_number = content[..byte_offset].matches('\n').count() + 1;
                    headings.push(Heading {
                        level,
                        text: heading_text.trim().to_string(),
                        line_number,
                    });
                    heading_text.clear();
                }
            }
            Event::Text(text) | Event::Code(text) if in_heading.is_some() => {
                heading_text.push_str(&text);
            }
            _ => {}
        }
    }

    headings
}

/// Extract internal links from markdown content.
///
/// Finds standard markdown links `[text](target)` using pulldown_cmark and
/// wikilinks `[[target]]` or `[[target|text]]` using regex. Filters out
/// external URLs (http://, https://, mailto:) and anchor-only links (#heading).
pub fn extract_links(content: &str) -> Vec<RawLink> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut links = Vec::new();

    // Pre-scan for wikilinks using regex (pulldown_cmark doesn't parse these)
    let wikilink_re = regex::Regex::new(r"\[\[([^\]]+)\]\]").expect("valid regex");
    for mat in wikilink_re.find_iter(content) {
        let line_number = content[..mat.start()].matches('\n').count() + 1;
        let inner = &content[mat.start() + 2..mat.end() - 2];
        let (target, text) = if let Some(pipe_pos) = inner.find('|') {
            (&inner[..pipe_pos], &inner[pipe_pos + 1..])
        } else {
            (inner, inner)
        };
        let target = target.trim();
        let text = text.trim();
        if !target.is_empty() && !is_external_or_anchor(target) {
            links.push(RawLink {
                target: target.to_string(),
                text: text.to_string(),
                line_number,
                is_wikilink: true,
            });
        }
    }

    // Standard markdown links via pulldown_cmark
    let parser = Parser::new_ext(content, Options::all());
    let mut current_link: Option<(String, usize)> = None; // (target, byte_offset)
    let mut link_text = String::new();

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                let url = dest_url.to_string();
                if !is_external_or_anchor(&url) {
                    current_link = Some((url, range.start));
                    link_text.clear();
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some((target, byte_offset)) = current_link.take() {
                    let line_number = content[..byte_offset].matches('\n').count() + 1;
                    links.push(RawLink {
                        target,
                        text: link_text.trim().to_string(),
                        line_number,
                        is_wikilink: false,
                    });
                    link_text.clear();
                }
            }
            Event::Text(text) | Event::Code(text) if current_link.is_some() => {
                link_text.push_str(&text);
            }
            _ => {}
        }
    }

    links
}

/// Check if a URL is external (http/https/mailto) or anchor-only (#heading).
fn is_external_or_anchor(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || (url.starts_with('#') && !url.contains('/'))
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

    // --- extract_headings tests ---

    #[test]
    fn extract_headings_basic() {
        let content = "# Title\n\nSome text\n\n## Section\n\nMore text";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "Title");
        assert_eq!(headings[0].line_number, 1);
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[1].text, "Section");
        assert_eq!(headings[1].line_number, 5);
    }

    #[test]
    fn extract_headings_all_levels() {
        let content = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 6);
        for (i, h) in headings.iter().enumerate() {
            assert_eq!(h.level, (i + 1) as u8);
            assert_eq!(h.line_number, i + 1);
        }
    }

    #[test]
    fn extract_headings_no_headings() {
        let content = "Just some text\nwithout headings";
        let headings = extract_headings(content);
        assert!(headings.is_empty());
    }

    #[test]
    fn extract_headings_with_inline_code() {
        let content = "# Heading with `code`";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].text, "Heading with code");
    }

    #[test]
    fn extract_headings_empty_content() {
        let headings = extract_headings("");
        assert!(headings.is_empty());
    }

    #[test]
    fn extract_headings_after_frontmatter() {
        let content = "---\ntitle: Test\n---\n# First Heading\n\nBody\n\n## Second";
        let (_fm, body) = extract_frontmatter(content);
        let headings = extract_headings(body);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].text, "First Heading");
        assert_eq!(headings[1].text, "Second");
    }

    // --- content_hash tests ---

    // --- extract_links tests ---

    #[test]
    fn extract_links_standard_markdown() {
        let content = "Check [this doc](other.md) for details.";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "other.md");
        assert_eq!(links[0].text, "this doc");
        assert_eq!(links[0].line_number, 1);
        assert!(!links[0].is_wikilink);
    }

    #[test]
    fn extract_links_wikilink() {
        let content = "See [[other-note]] for more.";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "other-note");
        assert_eq!(links[0].text, "other-note");
        assert!(links[0].is_wikilink);
    }

    #[test]
    fn extract_links_wikilink_with_alias() {
        let content = "See [[path/to/note|display text]] here.";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "path/to/note");
        assert_eq!(links[0].text, "display text");
        assert!(links[0].is_wikilink);
    }

    #[test]
    fn extract_links_filters_external() {
        let content = "[Google](https://google.com) and [local](notes.md) and [mail](mailto:x@y.com)";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "notes.md");
    }

    #[test]
    fn extract_links_filters_anchors() {
        let content = "[section](#heading) and [file](other.md#section)";
        let links = extract_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "other.md#section");
    }

    #[test]
    fn extract_links_line_numbers() {
        let content = "Line 1\n[link1](a.md)\nLine 3\n[[b]]";
        let links = extract_links(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].line_number, 4); // wikilinks come first (pre-scan)
        assert_eq!(links[1].line_number, 2);
    }

    #[test]
    fn extract_links_empty_content() {
        let links = extract_links("");
        assert!(links.is_empty());
    }

    #[test]
    fn extract_links_no_links() {
        let links = extract_links("Just plain text without any links.");
        assert!(links.is_empty());
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
