use std::collections::HashSet;
use std::str::FromStr;

use serde::Serialize;
use serde_json::Value;
use tracing::{debug, info};

use crate::embedding::provider::EmbeddingProvider;
use crate::error::{Error, Result};
use crate::fts::FtsIndex;
use crate::index::state::Index;
use crate::links;

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
    /// Whether to boost results that are link neighbors of top results.
    pub boost_links: bool,
    /// Search mode: hybrid, semantic, or lexical.
    pub mode: SearchMode,
    /// Optional path prefix to restrict results to a directory subtree.
    pub path_prefix: Option<String>,
    /// Per-query override for time decay (None = use config default).
    pub decay: Option<bool>,
    /// Per-query override for decay half-life in days (None = use config default).
    pub decay_half_life: Option<f64>,
}

impl SearchQuery {
    /// Create a new search query with sensible defaults (limit=10, min_score=0.0, no filters).
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: 10,
            min_score: 0.0,
            filters: Vec::new(),
            boost_links: false,
            mode: SearchMode::default(),
            path_prefix: None,
            decay: None,
            decay_half_life: None,
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

    /// Restrict results to files under the given path prefix.
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = Some(prefix.into());
        self
    }

    /// Add a metadata filter (multiple filters use AND logic).
    pub fn with_filter(mut self, filter: MetadataFilter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Enable link-graph boosting: results linked to top results get a score boost.
    pub fn with_boost_links(mut self, boost: bool) -> Self {
        self.boost_links = boost;
        self
    }

    /// Set the search mode (hybrid, semantic, or lexical).
    pub fn with_mode(mut self, mode: SearchMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enable or disable time decay for this query.
    pub fn with_decay(mut self, decay: bool) -> Self {
        self.decay = Some(decay);
        self
    }

    /// Set the time decay half-life in days for this query.
    pub fn with_decay_half_life(mut self, days: f64) -> Self {
        self.decay_half_life = Some(days);
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
    /// Relevance score (0.0–1.0, higher is more relevant).
    ///
    /// - **Semantic**: cosine similarity (absolute).
    /// - **Lexical**: BM25 score normalized via saturation `score/(score+k)`.
    /// - **Hybrid**: RRF score normalized by theoretical maximum.
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
    /// Path split into components (e.g., `["docs", "api", "auth.md"]`).
    pub path_components: Vec<String>,
    /// Filesystem modification time as Unix timestamp, if available.
    pub modified_at: Option<u64>,
}

/// Apply exponential time decay to a score based on file age.
///
/// Returns `score * 0.5^(elapsed_days / half_life_days)`.
/// The multiplier is always in (0, 1], so the result is <= the input score.
pub fn apply_time_decay(score: f64, modified_at: u64, half_life_days: f64, now: u64) -> f64 {
    let elapsed_secs = now.saturating_sub(modified_at) as f64;
    let elapsed_days = elapsed_secs / 86400.0;
    let multiplier = 0.5_f64.powf(elapsed_days / half_life_days);
    score * multiplier
}

/// Execute a search query against the index, supporting hybrid, semantic, and lexical modes.
///
/// Pipeline varies by mode:
/// - **Semantic**: embed → HNSW search → filter → assemble → truncate
/// - **Lexical**: BM25 search → normalize → filter → assemble → truncate (no embedding API call)
/// - **Hybrid**: semantic + lexical in parallel → RRF fusion → normalize → filter → assemble → truncate
///
/// When `fts_index` is `None` and mode is Hybrid or Lexical, falls back to semantic-only.
#[allow(clippy::too_many_arguments)]
pub async fn search(
    query: &SearchQuery,
    index: &Index,
    provider: &dyn EmbeddingProvider,
    fts_index: Option<&FtsIndex>,
    rrf_k: f64,
    bm25_norm_k: f64,
    decay_enabled: bool,
    decay_half_life: f64,
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

    // Over-fetch to account for filtering. 5x for hybrid (RRF needs more candidates), 3x otherwise.
    let over_fetch = match effective_mode {
        SearchMode::Hybrid => query.limit * 5,
        _ => query.limit * 3,
    };

    // Get ranked candidates based on mode.
    let mut ranked_candidates: Vec<(String, f64)> = match effective_mode {
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

    // Normalize scores to [0, 1] for non-semantic modes.
    match effective_mode {
        SearchMode::Lexical => normalize_bm25_scores(&mut ranked_candidates, bm25_norm_k),
        SearchMode::Hybrid => normalize_rrf_scores(&mut ranked_candidates, rrf_k, 2),
        SearchMode::Semantic => {} // already [0, 1] cosine similarity
    }

    debug!(
        candidates = ranked_candidates.len(),
        limit = query.limit,
        mode = %effective_mode,
        "search returned candidates"
    );

    // Resolve decay settings: per-query overrides take priority over config.
    let should_decay = query.decay.unwrap_or(decay_enabled);
    let effective_half_life = query.decay_half_life.unwrap_or(decay_half_life);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Filter, assemble results, and apply min_score.
    let results = assemble_results(
        query,
        index,
        &ranked_candidates,
        should_decay,
        effective_half_life,
        now,
    )?;

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

/// Assemble SearchResult objects from ranked candidates, applying decay, filters, and min_score.
fn assemble_results(
    query: &SearchQuery,
    index: &Index,
    candidates: &[(String, f64)],
    decay_enabled: bool,
    decay_half_life: f64,
    now: u64,
) -> Result<Vec<SearchResult>> {
    let file_mtimes = index.get_file_mtimes();

    let mut results = Vec::new();
    for (chunk_id, score) in candidates {
        // Look up chunk metadata.
        let Some(chunk) = index.get_chunk(chunk_id) else {
            continue;
        };

        // Apply path prefix filter (before file metadata lookup for early short-circuit).
        if let Some(ref prefix) = query.path_prefix {
            if !chunk.source_path.starts_with(prefix.as_str()) {
                continue;
            }
        }

        // Look up file metadata.
        let Some(file) = index.get_file_metadata(&chunk.source_path) else {
            continue;
        };

        // Apply time decay if enabled.
        let effective_score = if decay_enabled {
            let modified = file_mtimes.get(&chunk.source_path)
                .copied()
                .unwrap_or(file.indexed_at);
            apply_time_decay(*score, modified, decay_half_life, now)
        } else {
            *score
        };

        // Apply min_score threshold (on potentially decayed score).
        if effective_score < query.min_score {
            continue;
        }

        // Parse frontmatter JSON for filter evaluation.
        let frontmatter: Option<Value> = file
            .frontmatter
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());

        // Apply metadata filters.
        if !evaluate_filters(&query.filters, frontmatter.as_ref()) {
            continue;
        }

        let modified_at = file_mtimes.get(&chunk.source_path).copied();

        results.push(SearchResult {
            score: effective_score,
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
                path_components: chunk.source_path.split('/').map(String::from).collect(),
                modified_at,
            },
        });

        // Stop early if no decay (order preserved) and we have enough results.
        if !decay_enabled && results.len() >= query.limit {
            break;
        }
    }

    // When decay is applied, scores may reorder — re-sort and truncate.
    if decay_enabled && results.len() > 1 {
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(query.limit);
    }

    // Apply link-graph boosting if requested.
    if query.boost_links && results.len() > 1 {
        if let Some(link_graph) = index.get_link_graph() {
            let backlinks = links::compute_backlinks(&link_graph);

            // Collect link neighbors of top 3 results.
            let top_paths: Vec<String> = results
                .iter()
                .take(3)
                .map(|r| r.file.path.clone())
                .collect();

            let mut neighbor_paths: HashSet<String> = HashSet::new();
            for path in &top_paths {
                // Outgoing links from this file.
                if let Some(entries) = link_graph.forward.get(path) {
                    for entry in entries {
                        neighbor_paths.insert(entry.target.clone());
                    }
                }
                // Files linking to this file (backlinks).
                if let Some(entries) = backlinks.get(path) {
                    for entry in entries {
                        neighbor_paths.insert(entry.source.clone());
                    }
                }
            }

            // Remove the top paths themselves from neighbors to avoid self-boost.
            for path in &top_paths {
                neighbor_paths.remove(path);
            }

            // Boost neighbor results by 1.2x.
            if !neighbor_paths.is_empty() {
                for result in &mut results {
                    if neighbor_paths.contains(&result.file.path) {
                        result.score *= 1.2;
                        debug!(path = %result.file.path, "boosted link neighbor score");
                    }
                }
                // Re-sort by score descending.
                results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            }
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

/// Normalize BM25 scores to `[0, 1]` using saturation: `score / (score + k)`.
///
/// This maps the unbounded BM25 range `(0, ∞)` into `(0, 1)` with diminishing returns.
/// The parameter `k` controls the midpoint (a BM25 score equal to `k` maps to 0.5).
/// Monotonically increasing, so ranking order is preserved.
fn normalize_bm25_scores(results: &mut [(String, f64)], k: f64) {
    for r in results.iter_mut() {
        r.1 = r.1 / (r.1 + k);
    }
}

/// Normalize RRF scores by dividing by the theoretical maximum.
///
/// The maximum possible RRF score is `num_lists / (k + 1)`, achieved when a document
/// is ranked #1 in every list. After normalization:
/// - 1.0 = top-ranked by all retrievers
/// - ~0.5 = found by only one retriever at rank 1
///
/// Monotonically increasing, so ranking order is preserved.
fn normalize_rrf_scores(results: &mut [(String, f64)], rrf_k: f64, num_lists: usize) {
    let max_score = num_lists as f64 / (rrf_k + 1.0);
    if max_score > 0.0 {
        for r in results.iter_mut() {
            r.1 /= max_score;
        }
    }
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
    fn test_path_prefix_defaults_to_none() {
        let q = SearchQuery::new("hello");
        assert!(q.path_prefix.is_none());
    }

    #[test]
    fn test_search_query_with_path_prefix() {
        let q = SearchQuery::new("hello").with_path_prefix("docs/");
        assert_eq!(q.path_prefix, Some("docs/".to_string()));
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

    // --- boost_links field and builder tests ---

    #[test]
    fn test_search_query_boost_links_default_false() {
        let q = SearchQuery::new("test");
        assert!(!q.boost_links);
    }

    #[test]
    fn test_search_query_with_boost_links() {
        let q = SearchQuery::new("test").with_boost_links(true);
        assert!(q.boost_links);
    }

    #[test]
    fn test_search_query_with_boost_links_false() {
        let q = SearchQuery::new("test").with_boost_links(true).with_boost_links(false);
        assert!(!q.boost_links);
    }

    #[test]
    fn test_search_query_builder_chain_with_boost_links() {
        let q = SearchQuery::new("test")
            .with_limit(5)
            .with_min_score(0.5)
            .with_boost_links(true);
        assert_eq!(q.limit, 5);
        assert_eq!(q.min_score, 0.5);
        assert!(q.boost_links);
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

    // --- BM25 normalization tests ---

    #[test]
    fn test_bm25_normalization_midpoint() {
        // A BM25 score equal to k should normalize to 0.5
        let mut results = vec![("a".to_string(), 1.5)];
        normalize_bm25_scores(&mut results, 1.5);
        assert!((results[0].1 - 0.5).abs() < 1e-10, "k maps to 0.5");
    }

    #[test]
    fn test_bm25_normalization_known_values() {
        let mut results = vec![
            ("a".to_string(), 0.0),
            ("b".to_string(), 0.5),
            ("c".to_string(), 1.5),
            ("d".to_string(), 3.0),
            ("e".to_string(), 6.0),
            ("f".to_string(), 15.0),
        ];
        normalize_bm25_scores(&mut results, 1.5);

        assert!((results[0].1 - 0.0).abs() < 1e-10, "0 maps to 0");
        assert!((results[1].1 - 0.25).abs() < 1e-10, "0.5 maps to 0.25");
        assert!((results[2].1 - 0.5).abs() < 1e-10, "1.5 maps to 0.5");
        assert!((results[3].1 - 2.0 / 3.0).abs() < 1e-10, "3.0 maps to ~0.67");
        assert!((results[4].1 - 0.8).abs() < 1e-10, "6.0 maps to 0.8");
        assert!((results[5].1 - 15.0 / 16.5).abs() < 1e-10, "15.0 maps to ~0.91");
    }

    #[test]
    fn test_bm25_normalization_preserves_order() {
        let mut results = vec![
            ("a".to_string(), 10.0),
            ("b".to_string(), 5.0),
            ("c".to_string(), 1.0),
        ];
        normalize_bm25_scores(&mut results, 1.5);
        assert!(results[0].1 > results[1].1);
        assert!(results[1].1 > results[2].1);
    }

    #[test]
    fn test_bm25_normalization_stays_below_one() {
        let mut results = vec![("a".to_string(), 1000.0)];
        normalize_bm25_scores(&mut results, 1.5);
        assert!(results[0].1 < 1.0, "score should be < 1.0, got {}", results[0].1);
    }

    #[test]
    fn test_bm25_normalization_empty() {
        let mut results: Vec<(String, f64)> = vec![];
        normalize_bm25_scores(&mut results, 1.5);
        assert!(results.is_empty());
    }

    // --- RRF normalization tests ---

    #[test]
    fn test_rrf_normalization_rank1_both_lists() {
        // #1 in both lists with k=60 → raw score = 2/61, max = 2/61 → normalized = 1.0
        let mut results = vec![("a".to_string(), 2.0 / 61.0)];
        normalize_rrf_scores(&mut results, 60.0, 2);
        assert!((results[0].1 - 1.0).abs() < 1e-10, "expected 1.0, got {}", results[0].1);
    }

    #[test]
    fn test_rrf_normalization_rank1_one_list() {
        // #1 in one list only → raw = 1/61, max = 2/61 → normalized = 0.5
        let mut results = vec![("a".to_string(), 1.0 / 61.0)];
        normalize_rrf_scores(&mut results, 60.0, 2);
        assert!((results[0].1 - 0.5).abs() < 1e-10, "expected 0.5, got {}", results[0].1);
    }

    #[test]
    fn test_rrf_normalization_preserves_order() {
        let mut results = vec![
            ("a".to_string(), 2.0 / 61.0),  // both #1
            ("b".to_string(), 1.0 / 61.0 + 1.0 / 62.0), // #1 + #2
            ("c".to_string(), 1.0 / 61.0),  // single #1
        ];
        normalize_rrf_scores(&mut results, 60.0, 2);
        assert!(results[0].1 > results[1].1);
        assert!(results[1].1 > results[2].1);
    }

    #[test]
    fn test_rrf_normalization_empty() {
        let mut results: Vec<(String, f64)> = vec![];
        normalize_rrf_scores(&mut results, 60.0, 2);
        assert!(results.is_empty());
    }

    // --- Time decay tests ---

    #[test]
    fn test_apply_time_decay_zero_age() {
        // File modified right now → multiplier should be 1.0 (no penalty).
        let now = 1_700_000_000;
        let result = apply_time_decay(0.9, now, 90.0, now);
        assert!((result - 0.9).abs() < 1e-10);
    }

    #[test]
    fn test_apply_time_decay_at_half_life() {
        // File modified exactly 90 days ago with 90-day half-life → score halved.
        let now = 1_700_000_000;
        let modified = now - 90 * 86400;
        let result = apply_time_decay(1.0, modified, 90.0, now);
        assert!((result - 0.5).abs() < 1e-10, "expected 0.5, got {}", result);
    }

    #[test]
    fn test_apply_time_decay_double_half_life() {
        // File modified 180 days ago → score quartered.
        let now = 1_700_000_000;
        let modified = now - 180 * 86400;
        let result = apply_time_decay(1.0, modified, 90.0, now);
        assert!((result - 0.25).abs() < 1e-10, "expected 0.25, got {}", result);
    }

    #[test]
    fn test_apply_time_decay_very_old() {
        // File modified 365 days ago → very low score.
        let now = 1_700_000_000;
        let modified = now - 365 * 86400;
        let result = apply_time_decay(1.0, modified, 90.0, now);
        assert!(result > 0.0, "score should never be zero");
        assert!(result < 0.1, "365 days old should be < 10% of original, got {}", result);
    }

    #[test]
    fn test_apply_time_decay_preserves_zero_score() {
        let now = 1_700_000_000;
        let modified = now - 30 * 86400;
        let result = apply_time_decay(0.0, modified, 90.0, now);
        assert!((result - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_time_decay_short_half_life() {
        // 7-day half-life: more aggressive decay.
        let now = 1_700_000_000;
        let modified = now - 7 * 86400;
        let result = apply_time_decay(1.0, modified, 7.0, now);
        assert!((result - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_apply_time_decay_future_modified() {
        // Edge case: modified_at in the future (clock skew) → saturating_sub gives 0, no penalty.
        let now = 1_700_000_000;
        let modified = now + 100;
        let result = apply_time_decay(0.8, modified, 90.0, now);
        assert!((result - 0.8).abs() < 1e-10, "future mtime should have no penalty, got {}", result);
    }

    #[test]
    fn test_apply_time_decay_never_exceeds_original() {
        // Decay should never increase the score.
        let now = 1_700_000_000;
        for days_ago in [0, 1, 10, 30, 90, 180, 365, 1000] {
            let modified = now - days_ago * 86400;
            let result = apply_time_decay(0.75, modified, 90.0, now);
            assert!(result <= 0.75 + 1e-10, "decay should not exceed original score for age {} days", days_ago);
            assert!(result >= 0.0, "decay should not go negative");
        }
    }

    // --- SearchQuery decay builder tests ---

    #[test]
    fn test_search_query_decay_defaults() {
        let q = SearchQuery::new("test");
        assert!(q.decay.is_none());
        assert!(q.decay_half_life.is_none());
    }

    #[test]
    fn test_search_query_with_decay() {
        let q = SearchQuery::new("test").with_decay(true);
        assert_eq!(q.decay, Some(true));
    }

    #[test]
    fn test_search_query_with_decay_disabled() {
        let q = SearchQuery::new("test").with_decay(false);
        assert_eq!(q.decay, Some(false));
    }

    #[test]
    fn test_search_query_with_decay_half_life() {
        let q = SearchQuery::new("test").with_decay_half_life(30.0);
        assert_eq!(q.decay_half_life, Some(30.0));
    }

    #[test]
    fn test_search_query_decay_chain() {
        let q = SearchQuery::new("test")
            .with_decay(true)
            .with_decay_half_life(45.0)
            .with_limit(5);
        assert_eq!(q.decay, Some(true));
        assert_eq!(q.decay_half_life, Some(45.0));
        assert_eq!(q.limit, 5);
    }
}
