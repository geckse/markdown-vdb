use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::chunker::Chunk;
use crate::clustering::{ClusterState, CustomClusterState};
use crate::links::LinkGraph;
use crate::parser::MarkdownFile;
use crate::schema::{Schema, ScopedSchema};

/// A stable diagnostic produced while calculating a computed field.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    serde::Serialize,
    serde::Deserialize,
)]
#[rkyv(derive(Debug))]
pub struct ComputedFieldDiagnostic {
    /// Module that produced the diagnostic.
    pub module: String,
    /// Computed field name.
    pub field: String,
    /// Stable machine-readable error code.
    pub code: String,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Optional inclusive start byte offset in the source expression.
    pub span_start: Option<usize>,
    /// Optional exclusive end byte offset in the source expression.
    pub span_end: Option<usize>,
}

/// A module-owned computed field persisted alongside a file.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    serde::Serialize,
    serde::Deserialize,
)]
#[rkyv(derive(Debug))]
pub struct ComputedFieldEntry {
    /// Module that owns this entry.
    pub module: String,
    /// Fingerprint of the definition used to calculate the entry.
    pub definition_fingerprint: String,
    /// Successful result encoded as one complete JSON value.
    pub value_json: Option<String>,
    /// Calculation diagnostic, present when the value could not be produced.
    pub diagnostic: Option<ComputedFieldDiagnostic>,
}

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
    /// SHA-256 digest of the Markdown body represented by the stored vectors.
    ///
    /// Formula write-backs and other frontmatter-only changes update
    /// `content_hash` while leaving this hash unchanged. Incremental ingest can
    /// therefore prove that no embedding call is needed.
    pub embedding_body_hash: String,
    /// Frontmatter as a JSON string, if present.
    pub frontmatter: Option<String>,
    /// File size in bytes.
    pub file_size: u64,
    /// Chunk IDs belonging to this file.
    pub chunk_ids: Vec<String>,
    /// Unix timestamp (seconds since epoch) when the file was indexed.
    pub indexed_at: u64,
    /// Module bookkeeping and diagnostics, keyed by field name.
    ///
    /// Successful Formula values are also materialized into `frontmatter`;
    /// this cache records ownership and the definition fingerprint.
    pub computed_fields: HashMap<String, ComputedFieldEntry>,
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
    /// Cluster state from K-means clustering, if available.
    pub cluster_state: Option<ClusterState>,
    /// Link graph from link extraction, if available.
    pub link_graph: Option<LinkGraph>,
    /// File modification timestamps (path → mtime as Unix seconds).
    /// `None` for indices created before Phase 18.
    pub file_mtimes: Option<HashMap<String, u64>>,
    /// Path-scoped schemas from directory-level inference, if available.
    pub scoped_schemas: Option<Vec<ScopedSchema>>,
    /// User-defined custom cluster state, if available.
    pub custom_cluster_state: Option<CustomClusterState>,
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
    /// Number of edge vectors (semantic link embeddings) in the HNSW index.
    /// Edge vectors live only in HNSW, never in `chunks`, so a healthy index
    /// satisfies `vector_count == chunk_count + edge_count`.
    pub edge_count: usize,
    /// Unix timestamp of last save.
    pub last_updated: u64,
    /// Size of the index file on disk in bytes.
    pub file_size: u64,
    /// Embedding configuration snapshot.
    pub embedding_config: EmbeddingConfig,
}

/// Index-side counts for a path scope, returned by `Index::scoped_counts()`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ScopedCounts {
    /// Indexed files whose path falls within the scope.
    pub files: usize,
    /// Chunks belonging to in-scope files.
    pub chunks: usize,
    /// Edge vectors whose source file falls within the scope.
    pub edges: usize,
}

impl From<&Chunk> for StoredChunk {
    fn from(chunk: &Chunk) -> Self {
        Self {
            source_path: crate::path_util::to_slash(&chunk.source_path),
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
            relative_path: crate::path_util::to_slash(&file.path),
            content_hash: file.content_hash.clone(),
            embedding_body_hash: crate::parser::compute_content_hash(&file.body),
            frontmatter,
            file_size: file.file_size,
            chunk_ids: Vec::new(),
            indexed_at,
            computed_fields: HashMap::new(),
        }
    }
}

impl StoredFile {
    /// Decode all successful computed values, omitting diagnostics and invalid JSON.
    pub fn computed_values_json(&self) -> serde_json::Map<String, serde_json::Value> {
        self.computed_fields
            .iter()
            .filter_map(|(field, entry)| {
                let value = entry.value_json.as_deref()?;
                serde_json::from_str(value)
                    .ok()
                    .map(|value| (field.clone(), value))
            })
            .collect()
    }

    /// Return computed-field diagnostics sorted by field name.
    pub fn computed_errors_json(&self) -> BTreeMap<String, ComputedFieldDiagnostic> {
        self.computed_fields
            .iter()
            .filter_map(|(field, entry)| {
                entry
                    .diagnostic
                    .as_ref()
                    .map(|diagnostic| (field.clone(), diagnostic.clone()))
            })
            .collect()
    }

    /// Return the source frontmatter used by filtering and sorting.
    ///
    /// Formula values are materialized before this snapshot is stored, so the
    /// source value is authoritative and the computed cache is bookkeeping only.
    pub fn effective_frontmatter(&self) -> Option<serde_json::Value> {
        let mut value: serde_json::Value = self
            .frontmatter
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())?;
        if let Some(object) = value.as_object_mut() {
            for (field, entry) in &self.computed_fields {
                if entry.module == "formula" && entry.diagnostic.is_some() {
                    object.remove(field);
                }
            }
        }
        Some(value)
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
            links: Vec::new(),
            modified_at: 1700000000,
            frontmatter_links: Vec::new(),
        };

        let stored = StoredFile::from(&file);
        assert_eq!(stored.relative_path, "notes/readme.md");
        assert_eq!(stored.content_hash, "abc123");
        assert_eq!(
            stored.embedding_body_hash,
            crate::parser::compute_content_hash("Some body text")
        );
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
            links: Vec::new(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
        };

        let stored = StoredFile::from(&file);
        assert!(stored.frontmatter.is_none());
        assert!(stored.computed_fields.is_empty());
    }

    #[test]
    fn computed_field_helpers_preserve_raw_frontmatter() {
        let file = MarkdownFile {
            path: PathBuf::from("invoice.md"),
            frontmatter: Some(serde_json::json!({"price": 10, "total": 999})),
            headings: vec![],
            body: String::new(),
            content_hash: "formula".to_string(),
            file_size: 0,
            links: Vec::new(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
        };
        let mut stored = StoredFile::from(&file);
        stored.computed_fields.insert(
            "tax".to_string(),
            ComputedFieldEntry {
                module: "formula".to_string(),
                definition_fingerprint: "abc".to_string(),
                value_json: Some("1.9".to_string()),
                diagnostic: None,
            },
        );
        stored.computed_fields.insert(
            "total".to_string(),
            ComputedFieldEntry {
                module: "formula".to_string(),
                definition_fingerprint: "def".to_string(),
                value_json: None,
                diagnostic: Some(ComputedFieldDiagnostic {
                    module: "formula".to_string(),
                    field: "total".to_string(),
                    code: "writeback_failed".to_string(),
                    message: "formula source write failed".to_string(),
                    span_start: None,
                    span_end: None,
                }),
            },
        );

        let computed = stored.computed_values_json();
        assert_eq!(computed["tax"], serde_json::json!(1.9));
        assert!(!computed.contains_key("total"));

        let errors = stored.computed_errors_json();
        assert_eq!(errors["total"].code, "writeback_failed");

        let effective = stored.effective_frontmatter().unwrap();
        assert_eq!(effective["price"], 10);
        assert!(effective.get("tax").is_none());
        assert!(effective.get("total").is_none());

        // The persisted raw frontmatter JSON was not rewritten by the merge.
        let raw: serde_json::Value =
            serde_json::from_str(stored.frontmatter.as_deref().unwrap()).unwrap();
        assert_eq!(raw, serde_json::json!({"price": 10, "total": 999}));
    }

    #[test]
    fn computed_cache_is_not_effective_frontmatter_without_source_value() {
        let file = MarkdownFile {
            path: PathBuf::from("computed.md"),
            frontmatter: None,
            headings: vec![],
            body: String::new(),
            content_hash: "formula".to_string(),
            file_size: 0,
            links: Vec::new(),
            modified_at: 0,
            frontmatter_links: Vec::new(),
        };
        let mut stored = StoredFile::from(&file);
        stored.computed_fields.insert(
            "label".to_string(),
            ComputedFieldEntry {
                module: "formula".to_string(),
                definition_fingerprint: "abc".to_string(),
                value_json: Some("\"calculated\"".to_string()),
                diagnostic: None,
            },
        );

        assert!(stored.effective_frontmatter().is_none());
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
