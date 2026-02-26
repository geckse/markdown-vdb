use std::fs;

use mdvdb::parser::{compute_content_hash, parse_markdown_file};
use tempfile::TempDir;

/// Helper: create a markdown file in a temp dir and parse it.
fn parse_temp_file(content: &str) -> mdvdb::Result<mdvdb::parser::MarkdownFile> {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("test.md");
    fs::write(&file_path, content).unwrap();
    parse_markdown_file(tmp.path(), std::path::Path::new("test.md"))
}

#[test]
fn parse_simple_file() {
    let content = "---\ntitle: Hello\ntags:\n  - rust\n  - markdown\n---\n# Heading 1\n\nSome body text.\n\n## Heading 2\n\nMore text.\n";
    let result = parse_temp_file(content).unwrap();

    // Frontmatter present
    let fm = result.frontmatter.unwrap();
    assert_eq!(fm["title"], "Hello");
    assert_eq!(fm["tags"][0], "rust");
    assert_eq!(fm["tags"][1], "markdown");

    // Headings extracted
    assert_eq!(result.headings.len(), 2);
    assert_eq!(result.headings[0].level, 1);
    assert_eq!(result.headings[0].text, "Heading 1");
    assert_eq!(result.headings[1].level, 2);
    assert_eq!(result.headings[1].text, "Heading 2");

    // Body is everything after frontmatter
    assert!(result.body.contains("# Heading 1"));
    assert!(result.body.contains("Some body text."));

    // Content hash is present
    assert_eq!(result.content_hash.len(), 64);
}

#[test]
fn parse_no_frontmatter() {
    let content = "# Just a heading\n\nNo frontmatter here.\n";
    let result = parse_temp_file(content).unwrap();

    assert!(result.frontmatter.is_none());
    assert_eq!(result.headings.len(), 1);
    assert_eq!(result.headings[0].text, "Just a heading");
    assert!(result.body.contains("Just a heading"));
}

#[test]
fn parse_complex_frontmatter() {
    let content = "---\ntitle: Complex\nauthor:\n  name: Alice\n  email: alice@example.com\nmetadata:\n  nested:\n    deep: value\n---\n# Content\n";
    let result = parse_temp_file(content).unwrap();

    let fm = result.frontmatter.unwrap();
    assert_eq!(fm["title"], "Complex");
    assert_eq!(fm["author"]["name"], "Alice");
    assert_eq!(fm["author"]["email"], "alice@example.com");
    assert_eq!(fm["metadata"]["nested"]["deep"], "value");
}

#[test]
fn parse_deep_headings() {
    let content = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n";
    let result = parse_temp_file(content).unwrap();

    assert_eq!(result.headings.len(), 6);
    for (i, heading) in result.headings.iter().enumerate() {
        let level = (i + 1) as u8;
        assert_eq!(heading.level, level, "heading level mismatch at index {i}");
        assert_eq!(heading.text, format!("H{level}"));
        // Line numbers should be 1-based and sequential (one heading per line)
        assert_eq!(heading.line_number, i + 1);
    }
}

#[test]
fn parse_empty_file() {
    let content = "";
    let result = parse_temp_file(content).unwrap();

    assert!(result.frontmatter.is_none());
    assert!(result.headings.is_empty());
    assert!(result.body.is_empty());
    assert_eq!(result.content_hash.len(), 64);
}

#[test]
fn parse_frontmatter_types() {
    let content = "---\nstring_val: hello\nnumber_int: 42\nnumber_float: 3.14\nbool_val: true\nlist_val:\n  - one\n  - two\n  - three\nnested:\n  key: value\n---\n# Body\n";
    let result = parse_temp_file(content).unwrap();

    let fm = result.frontmatter.unwrap();
    assert_eq!(fm["string_val"], "hello");
    assert_eq!(fm["number_int"], 42);
    #[allow(clippy::approx_constant)]
    {
        assert_eq!(fm["number_float"], 3.14);
    }
    assert_eq!(fm["bool_val"], true);
    assert!(fm["list_val"].is_array());
    assert_eq!(fm["list_val"].as_array().unwrap().len(), 3);
    assert_eq!(fm["list_val"][0], "one");
    assert_eq!(fm["nested"]["key"], "value");
}

#[test]
fn content_hash_deterministic() {
    let content = "# Hello World\n\nSome content here.\n";
    let hash1 = compute_content_hash(content);
    let hash2 = compute_content_hash(content);

    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 64);

    // Different content produces different hash
    let hash3 = compute_content_hash("Different content");
    assert_ne!(hash1, hash3);
}
