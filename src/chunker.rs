use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tiktoken_rs::CoreBPE;

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
}
