use std::path::Path;

use mdvdb::chunker::{chunk_document, count_tokens};
use mdvdb::parser::parse_markdown_file;

/// Helper: parse a fixture file relative to the project root.
fn parse_fixture(name: &str) -> mdvdb::parser::MarkdownFile {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let relative = Path::new("tests/fixtures").join(name);
    parse_markdown_file(project_root, &relative)
        .unwrap_or_else(|e| panic!("failed to parse fixture {name}: {e}"))
}

#[test]
fn parse_then_chunk_simple() {
    let file = parse_fixture("simple.md");
    let chunks = chunk_document(&file, 1024, 0).unwrap();

    // simple.md has: h1 "Getting Started", h2 "Installation", h2 "Configuration",
    // h3 "Required Settings", h2 "Usage"
    // Expect chunks for each heading section with content.
    assert!(!chunks.is_empty(), "should produce chunks");

    // Verify sequential chunk indices
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.chunk_index, i);
        assert_eq!(chunk.id, format!("tests/fixtures/simple.md#{i}"));
        assert!(!chunk.is_sub_split, "simple.md is small, no sub-splitting expected");
    }

    // Verify heading hierarchies exist and h2 sections nest under h1
    let installation = chunks.iter().find(|c| c.heading_hierarchy.contains(&"Installation".to_string()));
    assert!(installation.is_some(), "should have an Installation chunk");
    let inst = installation.unwrap();
    assert_eq!(inst.heading_hierarchy, vec!["Getting Started", "Installation"]);

    // h3 Required Settings should nest under Configuration
    let required = chunks.iter().find(|c| c.heading_hierarchy.contains(&"Required Settings".to_string()));
    assert!(required.is_some(), "should have a Required Settings chunk");
    let req = required.unwrap();
    assert_eq!(req.heading_hierarchy, vec!["Getting Started", "Configuration", "Required Settings"]);

    // Usage resets to h2 level
    let usage = chunks.iter().find(|c| c.heading_hierarchy.contains(&"Usage".to_string()));
    assert!(usage.is_some(), "should have a Usage chunk");
    let u = usage.unwrap();
    assert_eq!(u.heading_hierarchy, vec!["Getting Started", "Usage"]);
}

#[test]
fn parse_then_chunk_deep_headings() {
    let file = parse_fixture("deep-headings.md");
    let chunks = chunk_document(&file, 1024, 0).unwrap();

    assert!(!chunks.is_empty(), "should produce chunks");

    // Find the deepest chunk (h6) and verify full hierarchy breadcrumb
    let h6_chunk = chunks.iter().find(|c| {
        c.heading_hierarchy.contains(&"Heading Level 6".to_string())
    });
    assert!(h6_chunk.is_some(), "should have a chunk for h6");
    let h6 = h6_chunk.unwrap();
    assert_eq!(
        h6.heading_hierarchy,
        vec![
            "Heading Level 1",
            "Heading Level 2",
            "Heading Level 3",
            "Heading Level 4",
            "Heading Level 5",
            "Heading Level 6",
        ]
    );

    // Verify all heading levels appear in at least one chunk's hierarchy
    for level in 1..=6 {
        let name = format!("Heading Level {level}");
        let found = chunks.iter().any(|c| c.heading_hierarchy.contains(&name));
        assert!(found, "heading level {level} should appear in hierarchy");
    }
}

#[test]
fn parse_then_chunk_no_frontmatter() {
    let file = parse_fixture("no-frontmatter.md");
    assert!(file.frontmatter.is_none(), "fixture should have no frontmatter");

    let chunks = chunk_document(&file, 1024, 0).unwrap();
    assert!(!chunks.is_empty(), "should produce chunks without frontmatter");

    // Verify chunk IDs use correct path
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.id, format!("tests/fixtures/no-frontmatter.md#{i}"));
    }

    // Should have sections for h1, h2, h2
    let section_one = chunks.iter().find(|c| c.heading_hierarchy.contains(&"Section One".to_string()));
    assert!(section_one.is_some(), "should have Section One chunk");
}

#[test]
fn parse_then_chunk_empty() {
    let file = parse_fixture("empty.md");
    let chunks = chunk_document(&file, 1024, 0).unwrap();

    // Empty file should produce 0 or 1 chunks
    assert!(chunks.len() <= 1, "empty file should produce at most 1 chunk");
}

#[test]
fn various_max_tokens() {
    let file = parse_fixture("simple.md");

    for max_tokens in [64, 128, 512, 1024] {
        let chunks = chunk_document(&file, max_tokens, 0).unwrap();
        assert!(!chunks.is_empty(), "max_tokens={max_tokens} should produce chunks");

        // All chunks should respect token limit (non-sub-split chunks fit within max_tokens)
        for chunk in &chunks {
            let tokens = count_tokens(&chunk.content);
            if !chunk.is_sub_split {
                assert!(
                    tokens <= max_tokens,
                    "non-sub-split chunk exceeds max_tokens={max_tokens}: got {tokens} tokens"
                );
            }
        }

        // Chunk indices should be sequential
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_index, i, "max_tokens={max_tokens}: chunk index mismatch");
        }
    }

    // Smaller max_tokens should produce equal or more chunks
    let chunks_64 = chunk_document(&file, 64, 0).unwrap();
    let chunks_1024 = chunk_document(&file, 1024, 0).unwrap();
    assert!(
        chunks_64.len() >= chunks_1024.len(),
        "smaller max_tokens should produce >= chunks: 64→{}, 1024→{}",
        chunks_64.len(),
        chunks_1024.len()
    );
}
