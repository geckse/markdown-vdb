use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tiktoken_rs::CoreBPE;
use tracing::debug;

use crate::parser::MarkdownFile;

/// A chunk of markdown content produced by the chunking engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Deterministic identifier: `"relative/path.md#index"`.
    pub id: String,
    /// Relative path to the source markdown file.
    pub source_path: PathBuf,
    /// Heading hierarchy leading to this chunk (e.g. `["H1 Title", "H2 Section"]`).
    pub heading_hierarchy: Vec<String>,
    /// The text content of this chunk.
    pub content: String,
    /// 1-based start line in the source file.
    pub start_line: usize,
    /// 1-based end line in the source file (inclusive).
    pub end_line: usize,
    /// 0-based index of this chunk within the file.
    pub chunk_index: usize,
    /// Whether this chunk was produced by splitting an oversized heading section.
    pub is_sub_split: bool,
}

/// Global cached tokenizer for token counting.
static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();

fn get_tokenizer() -> &'static CoreBPE {
    TOKENIZER
        .get_or_init(|| tiktoken_rs::cl100k_base().expect("failed to load cl100k_base tokenizer"))
}

/// Count the number of tokens in the given text using the cl100k_base tokenizer.
pub fn count_tokens(text: &str) -> usize {
    get_tokenizer().encode_ordinary(text).len()
}

/// A section of content between headings, used internally during chunking.
struct Section {
    /// Heading hierarchy for this section.
    heading_hierarchy: Vec<String>,
    /// Lines of content in this section.
    lines: Vec<String>,
    /// 1-based start line in the body.
    start_line: usize,
    /// 1-based end line in the body (inclusive).
    end_line: usize,
}

/// Split a section that exceeds `max_tokens` into smaller chunks using token-based
/// sliding windows with overlap.
///
/// Tokenizes the full section content, creates windows of `max_tokens` size with
/// `overlap_tokens` overlap between consecutive windows, then detokenizes each window
/// back to text. Each sub-chunk inherits the parent heading hierarchy and has
/// `is_sub_split = true`. Line ranges are approximated from character offset ratios.
fn sub_split_section(
    section: &Section,
    source_path: &str,
    max_tokens: usize,
    overlap_tokens: usize,
    chunk_index: &mut usize,
) -> Vec<Chunk> {
    let tokenizer = get_tokenizer();
    let full_content = section.lines.join("\n");
    let tokens = tokenizer.encode_ordinary(&full_content);
    let total_tokens = tokens.len();

    debug!(
        source_path,
        total_tokens, max_tokens, overlap_tokens, "sub-splitting oversized section"
    );

    if total_tokens == 0 {
        return Vec::new();
    }

    let total_lines = section.lines.len();
    let total_chars = full_content.len();
    let mut chunks = Vec::new();

    // Ensure stride is at least 1 to avoid infinite loop
    let stride = if max_tokens > overlap_tokens {
        max_tokens - overlap_tokens
    } else {
        max_tokens.max(1)
    };

    let mut start = 0usize;
    while start < total_tokens {
        let end = (start + max_tokens).min(total_tokens);
        let window = &tokens[start..end];

        let content = tokenizer.decode(window.to_vec()).unwrap_or_default();

        // Approximate line ranges based on character offset ratios.
        // Find where this chunk's text starts and ends in the full content.
        let chars_before: usize = if start == 0 {
            0
        } else {
            tokenizer
                .decode(tokens[..start].to_vec())
                .map(|s| s.len())
                .unwrap_or(0)
        };

        let approx_start_line = if total_chars > 0 {
            (chars_before as f64 / total_chars as f64 * total_lines as f64).floor() as usize
        } else {
            0
        };
        let chars_end = chars_before + content.len();
        let approx_end_line = if total_chars > 0 {
            (chars_end as f64 / total_chars as f64 * total_lines as f64).ceil() as usize
        } else {
            0
        };

        let start_line = section.start_line + approx_start_line.min(total_lines.saturating_sub(1));
        let end_line = section.start_line
            + approx_end_line
                .min(total_lines)
                .saturating_sub(1)
                .max(approx_start_line);

        let idx = *chunk_index;
        chunks.push(Chunk {
            id: format!("{source_path}#{idx}"),
            source_path: PathBuf::from(source_path),
            heading_hierarchy: section.heading_hierarchy.clone(),
            content,
            start_line,
            end_line: end_line.max(start_line),
            chunk_index: idx,
            is_sub_split: true,
        });
        *chunk_index += 1;

        if end >= total_tokens {
            break;
        }
        start += stride;
    }

    chunks
}

/// Chunk a parsed markdown file into semantically meaningful pieces.
///
/// Splits the document by headings, maintaining a heading hierarchy stack.
/// Sections that exceed `max_tokens` are further split by lines via
/// [`sub_split_section`]. Each chunk receives a deterministic ID of the form
/// `"source_path#chunk_index"`.
pub fn chunk_document(
    file: &MarkdownFile,
    max_tokens: usize,
    overlap_tokens: usize,
) -> crate::Result<Vec<Chunk>> {
    let body_lines: Vec<&str> = file.body.lines().collect();
    let total_lines = body_lines.len();
    let source_path = file.path.to_string_lossy();

    // Build sections from headings.
    // heading.line_number is 1-based relative to body.
    let mut sections: Vec<Section> = Vec::new();
    let mut heading_stack: Vec<(u8, String)> = Vec::new();

    // Collect heading positions (convert to 0-based line index in body)
    let mut heading_positions: Vec<(usize, u8, &str)> = Vec::new();
    for h in &file.headings {
        // line_number is 1-based in body
        if h.line_number >= 1 && h.line_number <= total_lines {
            heading_positions.push((h.line_number - 1, h.level, &h.text));
        }
    }

    // Determine section boundaries
    let mut boundaries: Vec<usize> = heading_positions.iter().map(|(idx, _, _)| *idx).collect();
    boundaries.sort();
    boundaries.dedup();

    // Build sections
    let mut prev_start = 0usize;
    let mut prev_hierarchy: Vec<String> = Vec::new();

    for &(line_idx, level, text) in &heading_positions {
        // Emit section for content before this heading
        if line_idx > prev_start || (line_idx == 0 && prev_start == 0 && sections.is_empty()) {
            // Only emit if there's actual content before first heading
            if line_idx > prev_start {
                let section_lines: Vec<String> = body_lines[prev_start..line_idx]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let content = section_lines.join("\n");
                if !content.trim().is_empty() {
                    sections.push(Section {
                        heading_hierarchy: prev_hierarchy.clone(),
                        lines: section_lines,
                        start_line: prev_start + 1,
                        end_line: line_idx,
                    });
                }
            }
        }

        // Update heading stack: pop all entries with level >= this level
        while heading_stack.last().is_some_and(|(l, _)| *l >= level) {
            heading_stack.pop();
        }
        heading_stack.push((level, text.to_string()));

        prev_hierarchy = heading_stack.iter().map(|(_, t)| t.clone()).collect();
        prev_start = line_idx;
    }

    // Emit final section (from last heading or start to end)
    if prev_start < total_lines {
        let section_lines: Vec<String> = body_lines[prev_start..total_lines]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let content = section_lines.join("\n");
        if !content.trim().is_empty() {
            sections.push(Section {
                heading_hierarchy: prev_hierarchy.clone(),
                lines: section_lines,
                start_line: prev_start + 1,
                end_line: total_lines,
            });
        }
    }

    // Handle no-heading body — all content as single section
    if sections.is_empty() && !file.body.trim().is_empty() {
        let section_lines: Vec<String> = body_lines.iter().map(|s| s.to_string()).collect();
        sections.push(Section {
            heading_hierarchy: Vec::new(),
            lines: section_lines,
            start_line: 1,
            end_line: total_lines,
        });
    }

    // Handle empty body — produce exactly 1 chunk with empty content
    if sections.is_empty() {
        sections.push(Section {
            heading_hierarchy: Vec::new(),
            lines: Vec::new(),
            start_line: 1,
            end_line: 1,
        });
    }

    debug!(sections = sections.len(), "heading-based sections found");

    // Convert sections to chunks, sub-splitting oversized sections
    let mut chunks = Vec::new();
    let mut chunk_index = 0usize;

    for section in &sections {
        let content = section.lines.join("\n");
        let tokens = count_tokens(&content);

        if tokens <= max_tokens {
            let idx = chunk_index;
            chunks.push(Chunk {
                id: format!("{source_path}#{idx}"),
                source_path: file.path.clone(),
                heading_hierarchy: section.heading_hierarchy.clone(),
                content,
                start_line: section.start_line,
                end_line: section.end_line,
                chunk_index: idx,
                is_sub_split: false,
            });
            chunk_index += 1;
        } else {
            let sub_chunks = sub_split_section(
                section,
                &source_path,
                max_tokens,
                overlap_tokens,
                &mut chunk_index,
            );
            chunks.extend(sub_chunks);
        }
    }

    debug!(chunks = chunks.len(), source = %source_path, "chunking complete");

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn count_tokens_hello_world() {
        let count = count_tokens("hello world");
        assert!(count > 0);
    }

    #[test]
    fn chunk_struct_serializes() {
        let chunk = Chunk {
            id: "test.md#0".into(),
            source_path: PathBuf::from("test.md"),
            heading_hierarchy: vec!["Introduction".into()],
            content: "Hello world".into(),
            start_line: 1,
            end_line: 5,
            chunk_index: 0,
            is_sub_split: false,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("test.md#0"));
    }

    #[test]
    fn chunk_struct_deserializes() {
        let json = r#"{"id":"a.md#1","source_path":"a.md","heading_hierarchy":[],"content":"x","start_line":1,"end_line":1,"chunk_index":1,"is_sub_split":true}"#;
        let chunk: Chunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.chunk_index, 1);
        assert!(chunk.is_sub_split);
    }

    fn make_file(body: &str, headings: Vec<crate::parser::Heading>) -> MarkdownFile {
        MarkdownFile {
            path: PathBuf::from("test.md"),
            frontmatter: None,
            headings,
            body: body.to_string(),
            content_hash: "abc".to_string(),
            file_size: body.len() as u64,
        }
    }

    #[test]
    fn chunk_document_no_headings() {
        let file = make_file("Hello world\nSecond line", vec![]);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "test.md#0");
        assert_eq!(chunks[0].chunk_index, 0);
        assert!(chunks[0].heading_hierarchy.is_empty());
        assert!(!chunks[0].is_sub_split);
    }

    #[test]
    fn chunk_document_with_headings() {
        use crate::parser::Heading;
        let body = "# Title\nSome intro\n## Section\nSection content";
        let headings = vec![
            Heading {
                level: 1,
                text: "Title".into(),
                line_number: 1,
            },
            Heading {
                level: 2,
                text: "Section".into(),
                line_number: 3,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading_hierarchy, vec!["Title"]);
        assert_eq!(chunks[1].heading_hierarchy, vec!["Title", "Section"]);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[1].chunk_index, 1);
    }

    #[test]
    fn chunk_document_preamble_before_heading() {
        use crate::parser::Heading;
        let body = "Preamble text\n# Title\nBody";
        let headings = vec![Heading {
            level: 1,
            text: "Title".into(),
            line_number: 2,
        }];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].heading_hierarchy.is_empty()); // preamble
        assert_eq!(chunks[1].heading_hierarchy, vec!["Title"]);
    }

    #[test]
    fn chunk_document_sequential_ids() {
        use crate::parser::Heading;
        let body = "# A\ntext\n# B\ntext\n# C\ntext";
        let headings = vec![
            Heading {
                level: 1,
                text: "A".into(),
                line_number: 1,
            },
            Heading {
                level: 1,
                text: "B".into(),
                line_number: 3,
            },
            Heading {
                level: 1,
                text: "C".into(),
                line_number: 5,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 3);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_index, i);
            assert_eq!(chunk.id, format!("test.md#{i}"));
        }
    }

    #[test]
    fn chunk_document_empty_body() {
        let file = make_file("", vec![]);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.is_empty());
    }

    #[test]
    fn chunk_document_heading_hierarchy_reset() {
        use crate::parser::Heading;
        // H1 > H2, then H1 should reset hierarchy
        let body = "# A\n## B\ntext\n# C\ntext";
        let headings = vec![
            Heading {
                level: 1,
                text: "A".into(),
                line_number: 1,
            },
            Heading {
                level: 2,
                text: "B".into(),
                line_number: 2,
            },
            Heading {
                level: 1,
                text: "C".into(),
                line_number: 4,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        // Find the chunk for C
        let c_chunk = chunks
            .iter()
            .find(|c| c.heading_hierarchy.contains(&"C".to_string()))
            .unwrap();
        assert_eq!(c_chunk.heading_hierarchy, vec!["C"]);
    }

    // --- Required test cases ---

    #[test]
    fn three_headings_three_chunks() {
        use crate::parser::Heading;
        let body = "# One\nContent one\n# Two\nContent two\n# Three\nContent three";
        let headings = vec![
            Heading {
                level: 1,
                text: "One".into(),
                line_number: 1,
            },
            Heading {
                level: 1,
                text: "Two".into(),
                line_number: 3,
            },
            Heading {
                level: 1,
                text: "Three".into(),
                line_number: 5,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn no_headings_single_chunk() {
        let file = make_file("Just some plain text\nwith multiple lines.", vec![]);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].heading_hierarchy.is_empty());
    }

    #[test]
    fn preamble_is_chunk_zero() {
        use crate::parser::Heading;
        let body = "This is preamble content.\n# First Heading\nHeading content";
        let headings = vec![Heading {
            level: 1,
            text: "First Heading".into(),
            line_number: 2,
        }];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].chunk_index, 0);
        assert!(chunks[0].heading_hierarchy.is_empty());
        assert!(chunks[0].content.contains("preamble"));
    }

    #[test]
    fn oversized_section_sub_splits() {
        use crate::parser::Heading;
        // Create content large enough to exceed a small max_tokens
        let long_text = "word ".repeat(200);
        let body = format!("# Big Section\n{long_text}");
        let headings = vec![Heading {
            level: 1,
            text: "Big Section".into(),
            line_number: 1,
        }];
        let file = make_file(&body, headings);
        let chunks = chunk_document(&file, 50, 10).unwrap();
        assert!(
            chunks.len() > 1,
            "oversized section should produce multiple chunks"
        );
    }

    #[test]
    fn sub_splits_marked_correctly() {
        use crate::parser::Heading;
        let long_text = "word ".repeat(200);
        let body = format!("# Big\n{long_text}");
        let headings = vec![Heading {
            level: 1,
            text: "Big".into(),
            line_number: 1,
        }];
        let file = make_file(&body, headings);
        let chunks = chunk_document(&file, 50, 10).unwrap();
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.is_sub_split,
                "sub-split chunks must have is_sub_split = true"
            );
        }
    }

    #[test]
    fn sub_split_overlap_correct() {
        use crate::parser::Heading;
        let long_text = "word ".repeat(200);
        let body = format!("# Big\n{long_text}");
        let headings = vec![Heading {
            level: 1,
            text: "Big".into(),
            line_number: 1,
        }];
        let file = make_file(&body, headings);
        let overlap = 10;
        let chunks = chunk_document(&file, 50, overlap).unwrap();
        assert!(chunks.len() >= 2, "need at least 2 chunks to test overlap");

        let tokenizer = get_tokenizer();
        for i in 0..chunks.len() - 1 {
            let tokens_k = tokenizer.encode_ordinary(&chunks[i].content);
            let tokens_k1 = tokenizer.encode_ordinary(&chunks[i + 1].content);
            // Last `overlap` tokens of chunk K should equal first `overlap` tokens of chunk K+1
            let tail = &tokens_k[tokens_k.len().saturating_sub(overlap)..];
            let head = &tokens_k1[..overlap.min(tokens_k1.len())];
            assert_eq!(
                tail, head,
                "overlap tokens must match between consecutive chunks"
            );
        }
    }

    #[test]
    fn heading_hierarchy_nested() {
        use crate::parser::Heading;
        let body = "# H1\n## H2\n### H3\nContent here";
        let headings = vec![
            Heading {
                level: 1,
                text: "H1".into(),
                line_number: 1,
            },
            Heading {
                level: 2,
                text: "H2".into(),
                line_number: 2,
            },
            Heading {
                level: 3,
                text: "H3".into(),
                line_number: 3,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        let last = chunks.last().unwrap();
        assert_eq!(last.heading_hierarchy, vec!["H1", "H2", "H3"]);
    }

    #[test]
    fn heading_hierarchy_resets() {
        use crate::parser::Heading;
        let body = "# A\n## B\ntext\n## C\ntext";
        let headings = vec![
            Heading {
                level: 1,
                text: "A".into(),
                line_number: 1,
            },
            Heading {
                level: 2,
                text: "B".into(),
                line_number: 2,
            },
            Heading {
                level: 2,
                text: "C".into(),
                line_number: 4,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        let c_chunk = chunks
            .iter()
            .find(|c| c.heading_hierarchy.contains(&"C".to_string()))
            .unwrap();
        // C replaces B at same level, hierarchy should be [A, C]
        assert_eq!(c_chunk.heading_hierarchy, vec!["A", "C"]);
    }

    #[test]
    fn short_file_single_chunk() {
        let file = make_file("Short.", vec![]);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn empty_body_single_chunk() {
        let file = make_file("", vec![]);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.is_empty());
        assert!(chunks[0].heading_hierarchy.is_empty());
    }

    #[test]
    fn count_tokens_accuracy() {
        // "hello world" should tokenize to known cl100k_base token count
        let count = count_tokens("hello world");
        assert_eq!(count, 2, "cl100k_base: 'hello world' = 2 tokens");

        let count2 = count_tokens("The quick brown fox jumps over the lazy dog");
        assert!(count2 > 0);
        // Verify consistency
        assert_eq!(
            count2,
            count_tokens("The quick brown fox jumps over the lazy dog")
        );
    }

    #[test]
    fn chunk_ids_format() {
        use crate::parser::Heading;
        let body = "# A\ntext\n# B\ntext";
        let headings = vec![
            Heading {
                level: 1,
                text: "A".into(),
                line_number: 1,
            },
            Heading {
                level: 1,
                text: "B".into(),
                line_number: 3,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(
                chunk.id,
                format!("test.md#{i}"),
                "ID must follow path#index format"
            );
        }
    }

    #[test]
    fn line_ranges_correct() {
        use crate::parser::Heading;
        let body = "# Title\nLine 2\nLine 3\n# Second\nLine 5\nLine 6";
        let headings = vec![
            Heading {
                level: 1,
                text: "Title".into(),
                line_number: 1,
            },
            Heading {
                level: 1,
                text: "Second".into(),
                line_number: 4,
            },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        assert_eq!(chunks.len(), 2);
        // First chunk starts at line 1 (heading line)
        assert!(chunks[0].start_line >= 1);
        // Second chunk starts at or after line 4
        assert!(chunks[1].start_line >= 4);
        // Lines should not overlap between non-sub-split chunks
        assert!(chunks[0].end_line <= chunks[1].start_line);
    }

    #[test]
    fn deterministic_output() {
        use crate::parser::Heading;
        let body = "# A\nContent\n## B\nMore content\n# C\nFinal";
        let headings = vec![
            Heading {
                level: 1,
                text: "A".into(),
                line_number: 1,
            },
            Heading {
                level: 2,
                text: "B".into(),
                line_number: 3,
            },
            Heading {
                level: 1,
                text: "C".into(),
                line_number: 5,
            },
        ];
        let file = make_file(body, headings.clone());
        let chunks1 = chunk_document(&file, 1000, 0).unwrap();

        let file2 = make_file(body, headings);
        let chunks2 = chunk_document(&file2, 1000, 0).unwrap();

        assert_eq!(chunks1.len(), chunks2.len());
        for (a, b) in chunks1.iter().zip(chunks2.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.content, b.content);
            assert_eq!(a.heading_hierarchy, b.heading_hierarchy);
            assert_eq!(a.start_line, b.start_line);
            assert_eq!(a.end_line, b.end_line);
            assert_eq!(a.chunk_index, b.chunk_index);
            assert_eq!(a.is_sub_split, b.is_sub_split);
        }
    }
}
