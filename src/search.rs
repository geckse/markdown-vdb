use serde::Serialize;
use serde_json::Value;
use tracing::{debug, info};

use crate::embedding::provider::EmbeddingProvider;
use crate::error::Result;
use crate::index::state::Index;

/// Builder-pattern query for semantic search.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Natural-language query string.
    pub query: String,
    /// Maximum number of results to return.
    pub limit: usize,
    /// Minimum cosine similarity score (0.0–1.0).
    pub min_score: f64,
    /// Metadata filters applied with AND logic.
    pub filters: Vec<MetadataFilter>,
}

impl SearchQuery {
    /// Create a new search query with sensible defaults (limit=10, min_score=0.0, no filters).
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: 10,
            min_score: 0.0,
            filters: Vec::new(),
        }
    }

    /// Set the maximum number of results to return.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Set the minimum cosine similarity score threshold.
    pub fn with_min_score(mut self, min_score: f64) -> Self {
        self.min_score = min_score;
        self
    }

    /// Add a metadata filter (multiple filters use AND logic).
    pub fn with_filter(mut self, filter: MetadataFilter) -> Self {
        self.filters.push(filter);
        self
    }
}

/// Metadata filter for narrowing search results by frontmatter fields.
#[derive(Debug, Clone)]
pub enum MetadataFilter {
    /// Exact value equality. If the field is an array, checks if the array contains the value.
    Equals { field: String, value: Value },
    /// Field value is in the provided list. If the field is an array, checks intersection.
    In {
        field: String,
        values: Vec<Value>,
    },
    /// Numeric or lexicographic range comparison. Either bound may be omitted.
    Range {
        field: String,
        min: Option<Value>,
        max: Option<Value>,
    },
    /// Field exists in frontmatter and is not null.
    Exists { field: String },
}

/// A single search result with relevance score, chunk content, and file context.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// Cosine similarity score (0.0–1.0, higher is more relevant).
    pub score: f64,
    /// The matched chunk.
    pub chunk: SearchResultChunk,
    /// File-level metadata for the chunk's source file.
    pub file: SearchResultFile,
}

/// Chunk-level data within a search result.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultChunk {
    /// Chunk identifier (e.g. "path.md#0").
    pub chunk_id: String,
    /// Heading hierarchy leading to this chunk.
    pub heading_hierarchy: Vec<String>,
    /// The text content of this chunk.
    pub content: String,
    /// 1-based start line in the source file.
    pub start_line: usize,
    /// 1-based end line in the source file (inclusive).
    pub end_line: usize,
}

/// File-level metadata within a search result.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultFile {
    /// Relative path to the source markdown file.
    pub path: String,
    /// Parsed frontmatter, if present.
    pub frontmatter: Option<Value>,
    /// File size in bytes.
    pub file_size: u64,
}

/// Execute a semantic search query against the index.
///
/// Pipeline: validate → embed → HNSW search (3x over-fetch) → filter → assemble → sort → truncate.
pub async fn search(
    query: &SearchQuery,
    index: &Index,
    provider: &dyn EmbeddingProvider,
) -> Result<Vec<SearchResult>> {
    // Validate: empty query is a no-op.
    if query.query.trim().is_empty() {
        debug!("empty query, returning no results");
        return Ok(Vec::new());
    }

    // Embed the query text.
    let embeddings = provider.embed_batch(std::slice::from_ref(&query.query)).await?;
    let query_vector = &embeddings[0];

    // Over-fetch 3x to account for filtering.
    let over_fetch = query.limit * 3;
    let candidates = index.search_vectors(query_vector, over_fetch)?;

    debug!(
        candidates = candidates.len(),
        limit = query.limit,
        "HNSW search returned candidates"
    );

    // Filter, assemble results, and apply min_score.
    let mut results = Vec::new();
    for (chunk_id, score) in &candidates {
        // Apply min_score threshold.
        if *score < query.min_score {
            continue;
        }

        // Look up chunk metadata.
        let Some(chunk) = index.get_chunk(chunk_id) else {
            continue;
        };

        // Look up file metadata.
        let Some(file) = index.get_file_metadata(&chunk.source_path) else {
            continue;
        };

        // Parse frontmatter JSON for filter evaluation.
        let frontmatter: Option<Value> = file
            .frontmatter
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());

        // Apply metadata filters.
        if !evaluate_filters(&query.filters, frontmatter.as_ref()) {
            continue;
        }

        results.push(SearchResult {
            score: *score,
            chunk: SearchResultChunk {
                chunk_id: chunk_id.clone(),
                heading_hierarchy: chunk.heading_hierarchy.clone(),
                content: chunk.content.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
            },
            file: SearchResultFile {
                path: chunk.source_path.clone(),
                frontmatter,
                file_size: file.file_size,
            },
        });

        // Stop once we have enough results.
        if results.len() >= query.limit {
            break;
        }
    }

    // Results are already sorted by score descending from search_vectors.
    info!(
        query = %query.query,
        results = results.len(),
        "search complete"
    );

    Ok(results)
}

/// Evaluate all metadata filters against parsed frontmatter. Returns `true` if all pass.
///
/// - Empty filters → always `true`
/// - `None` frontmatter with any filter → `false`
fn evaluate_filters(filters: &[MetadataFilter], frontmatter: Option<&Value>) -> bool {
    if filters.is_empty() {
        return true;
    }
    let Some(fm) = frontmatter else {
        return false;
    };

    filters.iter().all(|filter| evaluate_single_filter(filter, fm))
}

/// Evaluate a single metadata filter against frontmatter.
fn evaluate_single_filter(filter: &MetadataFilter, frontmatter: &Value) -> bool {
    match filter {
        MetadataFilter::Equals { field, value } => {
            let Some(field_value) = frontmatter.get(field) else {
                return false;
            };
            // If field is an array, check if it contains the value
            if let Some(arr) = field_value.as_array() {
                arr.contains(value)
            } else {
                field_value == value
            }
        }
        MetadataFilter::In { field, values } => {
            if values.is_empty() {
                return false;
            }
            let Some(field_value) = frontmatter.get(field) else {
                return false;
            };
            // If field is an array, check intersection
            if let Some(arr) = field_value.as_array() {
                arr.iter().any(|v| values.contains(v))
            } else {
                values.contains(field_value)
            }
        }
        MetadataFilter::Range { field, min, max } => {
            let Some(field_value) = frontmatter.get(field) else {
                return false;
            };
            if let Some(min_val) = min {
                if !compare_values(field_value, min_val, std::cmp::Ordering::Greater) {
                    return false;
                }
            }
            if let Some(max_val) = max {
                if !compare_values(field_value, max_val, std::cmp::Ordering::Less) {
                    return false;
                }
            }
            true
        }
        MetadataFilter::Exists { field } => {
            frontmatter
                .get(field)
                .is_some_and(|v| !v.is_null())
        }
    }
}

/// Compare two JSON values. Returns true if `a` is equal to `b` or has the given ordering
/// relative to `b`. Numeric values are compared as f64; otherwise falls back to string comparison.
fn compare_values(a: &Value, b: &Value, ordering: std::cmp::Ordering) -> bool {
    // Try numeric comparison first
    if let (Some(a_num), Some(b_num)) = (as_f64(a), as_f64(b)) {
        let cmp = a_num.partial_cmp(&b_num);
        return matches!(cmp, Some(std::cmp::Ordering::Equal)) || cmp == Some(ordering);
    }
    // Fall back to string comparison
    let a_str = value_as_string(a);
    let b_str = value_as_string(b);
    let cmp = a_str.cmp(&b_str);
    cmp == std::cmp::Ordering::Equal || cmp == ordering
}

/// Try to extract a numeric f64 from a JSON value.
fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

/// Convert a JSON value to a string for lexicographic comparison.
fn value_as_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- SearchQuery builder tests ---

    #[test]
    fn test_search_query_defaults() {
        let q = SearchQuery::new("hello world");
        assert_eq!(q.query, "hello world");
        assert_eq!(q.limit, 10);
        assert_eq!(q.min_score, 0.0);
        assert!(q.filters.is_empty());
    }

    #[test]
    fn test_search_query_builder_chain() {
        let q = SearchQuery::new("test query")
            .with_limit(5)
            .with_min_score(0.7)
            .with_filter(MetadataFilter::Exists {
                field: "author".into(),
            });
        assert_eq!(q.limit, 5);
        assert_eq!(q.min_score, 0.7);
        assert_eq!(q.filters.len(), 1);
    }

    // --- Filter evaluation tests ---

    #[test]
    fn test_no_filters_always_passes() {
        assert!(evaluate_filters(&[], Some(&json!({"a": 1}))));
        assert!(evaluate_filters(&[], None));
    }

    #[test]
    fn test_filters_no_frontmatter() {
        let filters = vec![MetadataFilter::Exists {
            field: "a".into(),
        }];
        assert!(!evaluate_filters(&filters, None));
    }

    #[test]
    fn test_equals_filter_matches() {
        let fm = json!({"status": "draft"});
        let filters = vec![MetadataFilter::Equals {
            field: "status".into(),
            value: json!("draft"),
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_equals_filter_rejects_mismatch() {
        let fm = json!({"status": "published"});
        let filters = vec![MetadataFilter::Equals {
            field: "status".into(),
            value: json!("draft"),
        }];
        assert!(!evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_equals_filter_array_contains() {
        let fm = json!({"tags": ["rust", "cli"]});
        let filters = vec![MetadataFilter::Equals {
            field: "tags".into(),
            value: json!("rust"),
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_in_filter_matches_value_in_list() {
        let fm = json!({"status": "draft"});
        let filters = vec![MetadataFilter::In {
            field: "status".into(),
            values: vec![json!("draft"), json!("review")],
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_in_filter_array_intersection() {
        let fm = json!({"tags": ["rust", "cli"]});
        let filters = vec![MetadataFilter::In {
            field: "tags".into(),
            values: vec![json!("python"), json!("cli")],
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_in_filter_empty_values() {
        let fm = json!({"status": "draft"});
        let filters = vec![MetadataFilter::In {
            field: "status".into(),
            values: vec![],
        }];
        assert!(!evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_range_filter_numeric_within() {
        let fm = json!({"year": 2024});
        let filters = vec![MetadataFilter::Range {
            field: "year".into(),
            min: Some(json!(2023)),
            max: Some(json!(2025)),
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_range_filter_min_only() {
        let fm = json!({"year": 2024});
        let filters = vec![MetadataFilter::Range {
            field: "year".into(),
            min: Some(json!(2020)),
            max: None,
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_range_filter_max_only() {
        let fm = json!({"year": 2024});
        let filters = vec![MetadataFilter::Range {
            field: "year".into(),
            min: None,
            max: Some(json!(2025)),
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_range_filter_numeric_out_of_range() {
        let fm = json!({"year": 2026});
        let filters = vec![MetadataFilter::Range {
            field: "year".into(),
            min: Some(json!(2023)),
            max: Some(json!(2025)),
        }];
        assert!(!evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_exists_filter_present() {
        let fm = json!({"author": "Alice"});
        let filters = vec![MetadataFilter::Exists {
            field: "author".into(),
        }];
        assert!(evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_exists_filter_missing() {
        let fm = json!({"title": "Foo"});
        let filters = vec![MetadataFilter::Exists {
            field: "author".into(),
        }];
        assert!(!evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_exists_filter_null_value() {
        let fm = json!({"author": null});
        let filters = vec![MetadataFilter::Exists {
            field: "author".into(),
        }];
        assert!(!evaluate_filters(&filters, Some(&fm)));
    }

    #[test]
    fn test_multiple_filters_and_logic() {
        let fm = json!({"status": "draft", "year": 2024});
        let filters = vec![
            MetadataFilter::Equals {
                field: "status".into(),
                value: json!("draft"),
            },
            MetadataFilter::Range {
                field: "year".into(),
                min: Some(json!(2023)),
                max: Some(json!(2025)),
            },
        ];
        assert!(evaluate_filters(&filters, Some(&fm)));

        // One filter fails → all fail
        let filters_fail = vec![
            MetadataFilter::Equals {
                field: "status".into(),
                value: json!("published"),
            },
            MetadataFilter::Range {
                field: "year".into(),
                min: Some(json!(2023)),
                max: Some(json!(2025)),
            },
        ];
        assert!(!evaluate_filters(&filters_fail, Some(&fm)));
    }
}
