use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tiktoken_rs::CoreBPE;

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
    TOKENIZER.get_or_init(|| tiktoken_rs::cl100k_base().expect("failed to load cl100k_base tokenizer"))
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

/// Split a section that exceeds `max_tokens` into smaller chunks by lines.
///
/// Each sub-chunk targets at most `max_tokens` tokens. Lines are never split
/// mid-line — if a single line exceeds `max_tokens`, it becomes its own chunk.
fn sub_split_section(
    section: &Section,
    source_path: &str,
    max_tokens: usize,
    _overlap_tokens: usize,
    chunk_index: &mut usize,
) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_start = section.start_line;
    let mut current_tokens = 0usize;

    for (i, line) in section.lines.iter().enumerate() {
        let line_tokens = count_tokens(line);
        let line_num = section.start_line + i;

        // If adding this line would exceed max and we have content, emit chunk
        if !current_lines.is_empty() && current_tokens + line_tokens > max_tokens {
            let content = current_lines.join("\n");
            let idx = *chunk_index;
            chunks.push(Chunk {
                id: format!("{source_path}#{idx}"),
                source_path: PathBuf::from(source_path),
                heading_hierarchy: section.heading_hierarchy.clone(),
                content,
                start_line: current_start,
                end_line: line_num - 1,
                chunk_index: idx,
                is_sub_split: true,
            });
            *chunk_index += 1;
            current_lines.clear();
            current_tokens = 0;
            current_start = line_num;
        }

        current_lines.push(line);
        current_tokens += line_tokens;
    }

    // Emit remaining lines
    if !current_lines.is_empty() {
        let content = current_lines.join("\n");
        let idx = *chunk_index;
        chunks.push(Chunk {
            id: format!("{source_path}#{idx}"),
            source_path: PathBuf::from(source_path),
            heading_hierarchy: section.heading_hierarchy.clone(),
            content,
            start_line: current_start,
            end_line: section.end_line,
            chunk_index: idx,
            is_sub_split: true,
        });
        *chunk_index += 1;
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
                let section_lines: Vec<String> =
                    body_lines[prev_start..line_idx].iter().map(|s| s.to_string()).collect();
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
        let section_lines: Vec<String> =
            body_lines[prev_start..total_lines].iter().map(|s| s.to_string()).collect();
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

    // Handle empty body — no headings, no content
    if sections.is_empty() && !file.body.trim().is_empty() {
        let section_lines: Vec<String> = body_lines.iter().map(|s| s.to_string()).collect();
        sections.push(Section {
            heading_hierarchy: Vec::new(),
            lines: section_lines,
            start_line: 1,
            end_line: total_lines,
        });
    }

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
            Heading { level: 1, text: "Title".into(), line_number: 1 },
            Heading { level: 2, text: "Section".into(), line_number: 3 },
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
        let headings = vec![
            Heading { level: 1, text: "Title".into(), line_number: 2 },
        ];
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
            Heading { level: 1, text: "A".into(), line_number: 1 },
            Heading { level: 1, text: "B".into(), line_number: 3 },
            Heading { level: 1, text: "C".into(), line_number: 5 },
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
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunk_document_heading_hierarchy_reset() {
        use crate::parser::Heading;
        // H1 > H2, then H1 should reset hierarchy
        let body = "# A\n## B\ntext\n# C\ntext";
        let headings = vec![
            Heading { level: 1, text: "A".into(), line_number: 1 },
            Heading { level: 2, text: "B".into(), line_number: 2 },
            Heading { level: 1, text: "C".into(), line_number: 4 },
        ];
        let file = make_file(body, headings);
        let chunks = chunk_document(&file, 1000, 0).unwrap();
        // Find the chunk for C
        let c_chunk = chunks.iter().find(|c| c.heading_hierarchy.contains(&"C".to_string())).unwrap();
        assert_eq!(c_chunk.heading_hierarchy, vec!["C"]);
    }
}
