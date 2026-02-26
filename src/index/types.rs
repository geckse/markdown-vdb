use std::collections::HashMap;
use std::time::SystemTime;

use crate::chunker::Chunk;
use crate::parser::MarkdownFile;
use crate::schema::Schema;

/// A chunk stored in the index, with rkyv derives for zero-copy deserialization.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct StoredChunk {
    /// Relative path to the source markdown file.
    pub source_path: String,
    /// Heading hierarchy leading to this chunk.
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

/// A file entry stored in the index.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct StoredFile {
    /// Relative path to the markdown file.
    pub relative_path: String,
    /// SHA-256 hex digest of the file content.
    pub content_hash: String,
    /// Frontmatter as a JSON string, if present.
    pub frontmatter: Option<String>,
    /// File size in bytes.
    pub file_size: u64,
    /// Chunk IDs belonging to this file.
    pub chunk_ids: Vec<String>,
    /// Unix timestamp (seconds since epoch) when the file was indexed.
    pub indexed_at: u64,
}

/// Embedding configuration stored in the index and used for JSON output.
#[derive(
    Debug, Clone, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, serde::Serialize,
)]
#[rkyv(derive(Debug))]
pub struct EmbeddingConfig {
    /// Provider name (e.g. "OpenAI", "Ollama", "Custom").
    pub provider: String,
    /// Model identifier (e.g. "text-embedding-3-small").
    pub model: String,
    /// Vector dimensionality (e.g. 1536).
    pub dimensions: usize,
}

/// Serialized metadata region of the index file.
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct IndexMetadata {
    /// Map from chunk ID (e.g. "path.md#0") to stored chunk data.
    pub chunks: HashMap<String, StoredChunk>,
    /// Map from relative file path to stored file data.
    pub files: HashMap<String, StoredFile>,
    /// Embedding configuration used to build this index.
    pub embedding_config: EmbeddingConfig,
    /// Unix timestamp (seconds since epoch) of last save.
    pub last_updated: u64,
    /// Inferred metadata schema, if available.
    pub schema: Option<Schema>,
}

/// Status snapshot returned by `Index::status()`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexStatus {
    /// Number of unique files in the index.
    pub document_count: usize,
    /// Total number of chunks across all files.
    pub chunk_count: usize,
    /// Total number of vectors in the HNSW index.
    pub vector_count: usize,
    /// Unix timestamp of last save.
    pub last_updated: u64,
    /// Size of the index file on disk in bytes.
    pub file_size: u64,
    /// Embedding configuration snapshot.
    pub embedding_config: EmbeddingConfig,
}

impl From<&Chunk> for StoredChunk {
    fn from(chunk: &Chunk) -> Self {
        Self {
            source_path: chunk.source_path.to_string_lossy().into_owned(),
            heading_hierarchy: chunk.heading_hierarchy.clone(),
            content: chunk.content.clone(),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            chunk_index: chunk.chunk_index,
            is_sub_split: chunk.is_sub_split,
        }
    }
}

impl From<&MarkdownFile> for StoredFile {
    fn from(file: &MarkdownFile) -> Self {
        let frontmatter = file
            .frontmatter
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());

        let indexed_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            relative_path: file.path.to_string_lossy().into_owned(),
            content_hash: file.content_hash.clone(),
            frontmatter,
            file_size: file.file_size,
            chunk_ids: Vec::new(),
            indexed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn stored_chunk_from_chunk() {
        let chunk = Chunk {
            id: "test.md#0".to_string(),
            source_path: PathBuf::from("docs/test.md"),
            heading_hierarchy: vec!["Title".to_string(), "Section".to_string()],
            content: "Hello world".to_string(),
            start_line: 1,
            end_line: 5,
            chunk_index: 0,
            is_sub_split: false,
        };

        let stored = StoredChunk::from(&chunk);
        assert_eq!(stored.source_path, "docs/test.md");
        assert_eq!(stored.heading_hierarchy, vec!["Title", "Section"]);
        assert_eq!(stored.content, "Hello world");
        assert_eq!(stored.start_line, 1);
        assert_eq!(stored.end_line, 5);
        assert_eq!(stored.chunk_index, 0);
        assert!(!stored.is_sub_split);
    }

    #[test]
    fn stored_file_from_markdown_file() {
        let file = MarkdownFile {
            path: PathBuf::from("notes/readme.md"),
            frontmatter: Some(serde_json::json!({"title": "Hello"})),
            headings: vec![],
            body: "Some body text".to_string(),
            content_hash: "abc123".to_string(),
            file_size: 1024,
        };

        let stored = StoredFile::from(&file);
        assert_eq!(stored.relative_path, "notes/readme.md");
        assert_eq!(stored.content_hash, "abc123");
        assert_eq!(stored.file_size, 1024);
        assert!(stored.chunk_ids.is_empty());
        assert!(stored.indexed_at > 0);
        assert!(stored.frontmatter.is_some());
        let fm = stored.frontmatter.unwrap();
        assert!(fm.contains("Hello"));
    }

    #[test]
    fn stored_file_from_markdown_file_no_frontmatter() {
        let file = MarkdownFile {
            path: PathBuf::from("test.md"),
            frontmatter: None,
            headings: vec![],
            body: String::new(),
            content_hash: "def456".to_string(),
            file_size: 0,
        };

        let stored = StoredFile::from(&file);
        assert!(stored.frontmatter.is_none());
    }

    #[test]
    fn embedding_config_equality() {
        let a = EmbeddingConfig {
            provider: "OpenAI".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
