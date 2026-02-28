use std::str::FromStr;

use serde::Serialize;
use serde_json::Value;
use tracing::{debug, info};

use crate::embedding::provider::EmbeddingProvider;
use crate::error::{Error, Result};
use crate::fts::FtsIndex;
use crate::index::state::Index;

/// Search mode controlling which retrieval signals are used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Both semantic (HNSW) and lexical (BM25) search, fused via RRF.
    #[default]
    Hybrid,
    /// Semantic search only (embedding + HNSW).
    Semantic,
    /// Lexical search only (BM25 via Tantivy). No embedding API call needed.
    Lexical,
}

impl FromStr for SearchMode {
    type Err = Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hybrid" => Ok(Self::Hybrid),
            "semantic" => Ok(Self::Semantic),
            "lexical" => Ok(Self::Lexical),
            other => Err(Error::Config(format!(
                "unknown search mode '{other}': expected hybrid, semantic, or lexical"
            ))),
        }
    }
}

impl std::fmt::Display for SearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hybrid => write!(f, "hybrid"),
            Self::Semantic => write!(f, "semantic"),
            Self::Lexical => write!(f, "lexical"),
        }
    }
}

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
    /// Search mode: hybrid, semantic, or lexical.
    pub mode: SearchMode,
}

impl SearchQuery {
    /// Create a new search query with sensible defaults (limit=10, min_score=0.0, no filters).
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: 10,
            min_score: 0.0,
            filters: Vec::new(),
            mode: SearchMode::default(),
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

    /// Set the search mode (hybrid, semantic, or lexical).
    pub fn with_mode(mut self, mode: SearchMode) -> Self {
        self.mode = mode;
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

/// Execute a search query against the index, supporting hybrid, semantic, and lexical modes.
///
/// Pipeline varies by mode:
/// - **Semantic**: embed → HNSW search → filter → assemble → truncate
/// - **Lexical**: BM25 search → filter → assemble → truncate (no embedding API call)
/// - **Hybrid**: semantic + lexical in parallel → RRF fusion → filter → assemble → truncate
///
/// When `fts_index` is `None` and mode is Hybrid or Lexical, falls back to semantic-only.
pub async fn search(
    query: &SearchQuery,
    index: &Index,
    provider: &dyn EmbeddingProvider,
    fts_index: Option<&FtsIndex>,
    rrf_k: f64,
) -> Result<Vec<SearchResult>> {
    // Validate: empty query is a no-op.
    if query.query.trim().is_empty() {
        debug!("empty query, returning no results");
        return Ok(Vec::new());
    }

    // Determine effective mode: fall back to semantic if no FTS index available.
    let effective_mode = match query.mode {
        SearchMode::Hybrid | SearchMode::Lexical if fts_index.is_none() => {
            debug!(
                requested = %query.mode,
                "FTS index not available, falling back to semantic mode"
            );
            SearchMode::Semantic
        }
        mode => mode,
    };

    // Over-fetch 3x to account for filtering.
    let over_fetch = query.limit * 3;

    // Get ranked candidates based on mode.
    let ranked_candidates: Vec<(String, f64)> = match effective_mode {
        SearchMode::Semantic => {
            semantic_search(query, index, provider, over_fetch).await?
        }
        SearchMode::Lexical => {
            let fts = fts_index.unwrap(); // safe: checked above
            lexical_search(query, fts, over_fetch)?
        }
        SearchMode::Hybrid => {
            let fts = fts_index.unwrap(); // safe: checked above
            let (semantic_results, lexical_results) = tokio::join!(
                semantic_search(query, index, provider, over_fetch),
                async { lexical_search(query, fts, over_fetch) }
            );
            let semantic = semantic_results?;
            let lexical = lexical_results?;

            debug!(
                semantic_count = semantic.len(),
                lexical_count = lexical.len(),
                rrf_k = rrf_k,
                "fusing semantic and lexical results via RRF"
            );

            reciprocal_rank_fusion(&semantic, &lexical, rrf_k)
        }
    };

    debug!(
        candidates = ranked_candidates.len(),
        limit = query.limit,
        mode = %effective_mode,
        "search returned candidates"
    );

    // Filter, assemble results, and apply min_score.
    let results = assemble_results(query, index, &ranked_candidates)?;

    info!(
        query = %query.query,
        results = results.len(),
        mode = %effective_mode,
        "search complete"
    );

    Ok(results)
}

/// Run semantic (HNSW) search and return ranked (chunk_id, score) pairs.
async fn semantic_search(
    query: &SearchQuery,
    index: &Index,
    provider: &dyn EmbeddingProvider,
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let embeddings = provider.embed_batch(std::slice::from_ref(&query.query)).await?;
    let query_vector = &embeddings[0];
    let candidates = index.search_vectors(query_vector, limit)?;
    Ok(candidates)
}

/// Run lexical (BM25) search and return ranked (chunk_id, score) pairs.
fn lexical_search(
    query: &SearchQuery,
    fts_index: &FtsIndex,
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let fts_results = fts_index.search(&query.query, limit)?;
    Ok(fts_results
        .into_iter()
        .map(|r| (r.chunk_id, r.score as f64))
        .collect())
}

/// Assemble SearchResult objects from ranked candidates, applying filters and min_score.
fn assemble_results(
    query: &SearchQuery,
    index: &Index,
    candidates: &[(String, f64)],
) -> Result<Vec<SearchResult>> {
    let mut results = Vec::new();
    for (chunk_id, score) in candidates {
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
    Ok(results)
}

/// Reciprocal Rank Fusion: merges two ranked lists into a single scored list.
///
/// Each item's fused score is `Σ 1/(k + rank)` where rank is 1-indexed.
/// Items appearing in both lists accumulate scores from each.
/// Returns a list of `(chunk_id, fused_score)` sorted by score descending.
pub fn reciprocal_rank_fusion(
    list_a: &[(String, f64)],
    list_b: &[(String, f64)],
    k: f64,
) -> Vec<(String, f64)> {
    use std::collections::HashMap;

    let mut scores: HashMap<String, f64> = HashMap::new();

    for (rank, (id, _score)) in list_a.iter().enumerate() {
        let rrf_score = 1.0 / (k + (rank as f64 + 1.0));
        *scores.entry(id.clone()).or_default() += rrf_score;
    }

    for (rank, (id, _score)) in list_b.iter().enumerate() {
        let rrf_score = 1.0 / (k + (rank as f64 + 1.0));
        *scores.entry(id.clone()).or_default() += rrf_score;
    }

    let mut results: Vec<(String, f64)> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results
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

    // --- SearchMode tests ---

    #[test]
    fn test_search_mode_default_is_hybrid() {
        assert_eq!(SearchMode::default(), SearchMode::Hybrid);
    }

    #[test]
    fn test_search_mode_from_str() {
        assert_eq!("hybrid".parse::<SearchMode>().unwrap(), SearchMode::Hybrid);
        assert_eq!("semantic".parse::<SearchMode>().unwrap(), SearchMode::Semantic);
        assert_eq!("lexical".parse::<SearchMode>().unwrap(), SearchMode::Lexical);
        // Case-insensitive
        assert_eq!("HYBRID".parse::<SearchMode>().unwrap(), SearchMode::Hybrid);
        assert_eq!("Semantic".parse::<SearchMode>().unwrap(), SearchMode::Semantic);
    }

    #[test]
    fn test_search_mode_from_str_invalid() {
        let err = "invalid".parse::<SearchMode>().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown search mode"));
    }

    #[test]
    fn test_search_mode_display() {
        assert_eq!(SearchMode::Hybrid.to_string(), "hybrid");
        assert_eq!(SearchMode::Semantic.to_string(), "semantic");
        assert_eq!(SearchMode::Lexical.to_string(), "lexical");
    }

    #[test]
    fn test_search_mode_serialize() {
        assert_eq!(serde_json::to_string(&SearchMode::Hybrid).unwrap(), "\"hybrid\"");
        assert_eq!(serde_json::to_string(&SearchMode::Lexical).unwrap(), "\"lexical\"");
    }

    // --- SearchQuery builder tests ---

    #[test]
    fn test_search_query_defaults() {
        let q = SearchQuery::new("hello world");
        assert_eq!(q.query, "hello world");
        assert_eq!(q.limit, 10);
        assert_eq!(q.min_score, 0.0);
        assert!(q.filters.is_empty());
        assert_eq!(q.mode, SearchMode::Hybrid);
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

    #[test]
    fn test_search_query_with_mode() {
        let q = SearchQuery::new("test").with_mode(SearchMode::Lexical);
        assert_eq!(q.mode, SearchMode::Lexical);

        let q2 = SearchQuery::new("test").with_mode(SearchMode::Semantic);
        assert_eq!(q2.mode, SearchMode::Semantic);
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

    // --- RRF tests ---

    #[test]
    fn test_rrf_overlapping_lists() {
        let list_a = vec![
            ("a".to_string(), 0.9),
            ("b".to_string(), 0.8),
            ("c".to_string(), 0.7),
        ];
        let list_b = vec![
            ("b".to_string(), 5.0),
            ("c".to_string(), 4.0),
            ("d".to_string(), 3.0),
        ];
        let results = reciprocal_rank_fusion(&list_a, &list_b, 60.0);

        // "b" appears in both lists (rank 2 in A, rank 1 in B)
        let b_score: f64 = results.iter().find(|(id, _)| id == "b").unwrap().1;
        let expected_b = 1.0 / (60.0 + 2.0) + 1.0 / (60.0 + 1.0);
        assert!((b_score - expected_b).abs() < 1e-10);

        // "a" only in list_a rank 1
        let a_score = results.iter().find(|(id, _)| id == "a").unwrap().1;
        let expected_a = 1.0 / (60.0 + 1.0);
        assert!((a_score - expected_a).abs() < 1e-10);

        // Results sorted descending by score
        for w in results.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    #[test]
    fn test_rrf_disjoint_lists() {
        let list_a = vec![("a".to_string(), 1.0), ("b".to_string(), 0.5)];
        let list_b = vec![("c".to_string(), 1.0), ("d".to_string(), 0.5)];
        let results = reciprocal_rank_fusion(&list_a, &list_b, 60.0);

        assert_eq!(results.len(), 4);
        // All items have single-list scores: 1/(60+rank)
        let a_score = results.iter().find(|(id, _)| id == "a").unwrap().1;
        assert!((a_score - 1.0 / 61.0).abs() < 1e-10);
        let c_score = results.iter().find(|(id, _)| id == "c").unwrap().1;
        assert!((c_score - 1.0 / 61.0).abs() < 1e-10);
    }

    #[test]
    fn test_rrf_single_list_with_empty() {
        let list_a = vec![("x".to_string(), 1.0), ("y".to_string(), 0.5)];
        let empty: Vec<(String, f64)> = vec![];
        let results = reciprocal_rank_fusion(&list_a, &empty, 60.0);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "x");
        assert!((results[0].1 - 1.0 / 61.0).abs() < 1e-10);
        assert_eq!(results[1].0, "y");
        assert!((results[1].1 - 1.0 / 62.0).abs() < 1e-10);
    }

    #[test]
    fn test_rrf_k_parameter_effect() {
        let list_a = vec![("a".to_string(), 1.0), ("b".to_string(), 0.5)];
        let list_b = vec![("a".to_string(), 1.0), ("b".to_string(), 0.5)];

        // Smaller k → larger scores and bigger spread between ranks
        let results_k1 = reciprocal_rank_fusion(&list_a, &list_b, 1.0);
        let results_k60 = reciprocal_rank_fusion(&list_a, &list_b, 60.0);

        let a_k1 = results_k1.iter().find(|(id, _)| id == "a").unwrap().1;
        let a_k60 = results_k60.iter().find(|(id, _)| id == "a").unwrap().1;
        // k=1: score = 2 * 1/(1+1) = 1.0; k=60: score = 2 * 1/(60+1) ≈ 0.0328
        assert!(a_k1 > a_k60);
    }

    #[test]
    fn test_rrf_empty_inputs() {
        let empty: Vec<(String, f64)> = vec![];
        let results = reciprocal_rank_fusion(&empty, &empty, 60.0);
        assert!(results.is_empty());
    }
}
