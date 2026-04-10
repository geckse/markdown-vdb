use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;

use crate::clustering::CustomClusterDef;
use crate::error::Error;
use crate::search::SearchMode;

/// Supported embedding provider backends.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum EmbeddingProviderType {
    OpenAI,
    Ollama,
    Custom,
    Mock,
}

impl FromStr for EmbeddingProviderType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "ollama" => Ok(Self::Ollama),
            "custom" => Ok(Self::Custom),
            "mock" => Ok(Self::Mock),
            other => Err(Error::Config(format!(
                "unknown embedding provider '{other}': expected openai, ollama, or custom"
            ))),
        }
    }
}

/// Supported vector quantization types for the HNSW index.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum VectorQuantization {
    F16,
    F32,
}

impl FromStr for VectorQuantization {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "f16" => Ok(Self::F16),
            "f32" => Ok(Self::F32),
            other => Err(Error::Config(format!(
                "unknown vector quantization '{other}': expected f16 or f32"
            ))),
        }
    }
}

/// Full configuration for mdvdb, loaded from environment / `.markdownvdb` file / defaults.
#[derive(Debug, Clone, Serialize)]
pub struct Config {
    pub embedding_provider: EmbeddingProviderType,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub embedding_batch_size: usize,
    pub openai_api_key: Option<String>,
    pub ollama_host: String,
    pub embedding_endpoint: Option<String>,
    pub source_dirs: Vec<PathBuf>,
    pub ignore_patterns: Vec<String>,
    pub watch_enabled: bool,
    pub watch_debounce_ms: u64,
    pub chunk_max_tokens: usize,
    pub chunk_overlap_tokens: usize,
    pub clustering_enabled: bool,
    pub clustering_rebalance_threshold: usize,
    /// Cluster granularity multiplier. Higher = more clusters. Default: 1.0, range [0.25, 4.0].
    pub clustering_granularity: f64,
    pub search_default_limit: usize,
    pub search_min_score: f64,
    pub search_default_mode: SearchMode,
    pub search_rrf_k: f64,
    /// BM25 saturation normalization constant. A BM25 score equal to this
    /// value maps to 0.5 after normalization. Higher = more compressed scores.
    pub bm25_norm_k: f64,
    /// Whether time decay is applied to search scores by default.
    pub search_decay_enabled: bool,
    /// Half-life in days for time decay. After this many days, a score is halved.
    pub search_decay_half_life: f64,
    /// Path prefixes excluded from time decay (immune to decay even when enabled).
    pub search_decay_exclude: Vec<String>,
    /// Path prefixes where time decay applies (whitelist). Empty = all files eligible.
    pub search_decay_include: Vec<String>,
    /// Whether link boosting is applied to search results by default.
    pub search_boost_links: bool,
    /// Number of link-graph hops used for link-boost scoring. Default: 1, range 1–3.
    pub search_boost_hops: usize,
    /// Number of graph hops to expand results (0 = disabled). Default: 0, range 0–3.
    pub search_expand_graph: usize,
    /// Maximum number of graph-expanded results to add. Default: 3, range 1–10.
    pub search_expand_limit: usize,
    /// Vector quantization type for the HNSW index. Default: F16.
    pub vector_quantization: VectorQuantization,
    /// Whether to compress the metadata region with zstd. Default: true.
    pub index_compression: bool,
    /// Whether to compute and store edge embeddings. Default: true.
    pub edge_embeddings: bool,
    /// Weight for edge-based boost in search scoring. Default: 0.15, range [0.0, 1.0].
    pub edge_boost_weight: f64,
    /// Threshold for rebalancing edge clusters. Default: 50, must be > 0.
    pub edge_cluster_rebalance: usize,
    /// User-defined custom cluster definitions (name + seed phrases).
    pub custom_cluster_defs: Vec<CustomClusterDef>,
}

impl Config {
    /// Resolve the user-level config directory.
    /// Priority: MDVDB_CONFIG_HOME env var > ~/.mdvdb
    pub fn user_config_dir() -> Option<PathBuf> {
        if let Ok(custom) = std::env::var("MDVDB_CONFIG_HOME") {
            if !custom.is_empty() {
                return Some(PathBuf::from(custom));
            }
        }
        dirs::home_dir().map(|h| h.join(".mdvdb"))
    }

    /// Resolve the user-level config file path (~/.mdvdb/config).
    pub fn user_config_path() -> Option<PathBuf> {
        Self::user_config_dir().map(|d| d.join("config"))
    }

    /// Load configuration with priority: shell env > `.markdownvdb/.config` > legacy `.markdownvdb` > `.env` > `~/.mdvdb/config` > defaults.
    pub fn load(project_root: &Path) -> Result<Self, Error> {
        // Load config file (ignore if missing).
        // dotenvy::from_path does NOT override existing env vars,
        // so shell env always takes priority.
        // Try new location first, fall back to legacy flat file.
        let new_config = project_root.join(".markdownvdb").join(".config");
        let legacy_config = project_root.join(".markdownvdb");
        if new_config.is_file() {
            let _ = dotenvy::from_path(new_config);
        } else if legacy_config.is_file() {
            let _ = dotenvy::from_path(legacy_config);
        }

        // Load .env as a fallback for shared secrets (e.g., OPENAI_API_KEY).
        // Since .markdownvdb was loaded first, its values take priority over .env.
        let _ = dotenvy::from_path(project_root.join(".env"));

        // Load user-level config (~/.mdvdb/config) as lowest-priority file source.
        if std::env::var("MDVDB_NO_USER_CONFIG").is_err() {
            if let Some(config_dir) = Self::user_config_dir() {
                let _ = dotenvy::from_path(config_dir.join("config"));
            }
        }

        let embedding_provider = env_or_default("MDVDB_EMBEDDING_PROVIDER", "openai")
            .parse::<EmbeddingProviderType>()?;

        let embedding_model = env_or_default("MDVDB_EMBEDDING_MODEL", "text-embedding-3-small");

        let embedding_dimensions = parse_env::<usize>("MDVDB_EMBEDDING_DIMENSIONS", 1536)?;

        let embedding_batch_size = parse_env::<usize>("MDVDB_EMBEDDING_BATCH_SIZE", 100)?;

        let openai_api_key = std::env::var("OPENAI_API_KEY").ok();

        let ollama_host = env_or_default("OLLAMA_HOST", "http://localhost:11434");

        let embedding_endpoint = std::env::var("MDVDB_EMBEDDING_ENDPOINT").ok();

        let source_dirs = parse_comma_list_path("MDVDB_SOURCE_DIRS", vec![PathBuf::from(".")]);

        let ignore_patterns = parse_comma_list_string("MDVDB_IGNORE_PATTERNS", vec![]);

        let watch_enabled = parse_env_bool("MDVDB_WATCH", true)?;

        let watch_debounce_ms = parse_env::<u64>("MDVDB_WATCH_DEBOUNCE_MS", 300)?;

        let chunk_max_tokens = parse_env::<usize>("MDVDB_CHUNK_MAX_TOKENS", 512)?;

        let chunk_overlap_tokens = parse_env::<usize>("MDVDB_CHUNK_OVERLAP_TOKENS", 50)?;

        let clustering_enabled = parse_env_bool("MDVDB_CLUSTERING_ENABLED", true)?;

        let clustering_rebalance_threshold =
            parse_env::<usize>("MDVDB_CLUSTERING_REBALANCE_THRESHOLD", 50)?;

        let clustering_granularity =
            parse_env::<f64>("MDVDB_CLUSTER_GRANULARITY", 1.0)?;

        let search_default_limit = parse_env::<usize>("MDVDB_SEARCH_DEFAULT_LIMIT", 10)?;

        let search_min_score = parse_env::<f64>("MDVDB_SEARCH_MIN_SCORE", 0.0)?;

        let search_default_mode = env_or_default("MDVDB_SEARCH_MODE", "hybrid")
            .parse::<SearchMode>()?;

        let search_rrf_k = parse_env::<f64>("MDVDB_SEARCH_RRF_K", 60.0)?;

        let bm25_norm_k = parse_env::<f64>("MDVDB_BM25_NORM_K", 1.5)?;

        let search_decay_enabled = parse_env_bool("MDVDB_SEARCH_DECAY", false)?;

        let search_decay_half_life = parse_env::<f64>("MDVDB_SEARCH_DECAY_HALF_LIFE", 90.0)?;

        let search_decay_exclude =
            parse_comma_list_string("MDVDB_SEARCH_DECAY_EXCLUDE", vec![]);

        let search_decay_include =
            parse_comma_list_string("MDVDB_SEARCH_DECAY_INCLUDE", vec![]);

        let search_boost_links = parse_env_bool("MDVDB_SEARCH_BOOST_LINKS", false)?;

        let search_boost_hops = parse_env::<usize>("MDVDB_SEARCH_BOOST_HOPS", 1)?;

        let search_expand_graph = parse_env::<usize>("MDVDB_SEARCH_EXPAND_GRAPH", 0)?;

        let search_expand_limit = parse_env::<usize>("MDVDB_SEARCH_EXPAND_LIMIT", 3)?;

        let vector_quantization = env_or_default("MDVDB_VECTOR_QUANTIZATION", "f16")
            .parse::<VectorQuantization>()?;

        let index_compression = parse_env_bool("MDVDB_INDEX_COMPRESSION", true)?;

        let edge_embeddings = parse_env_bool("MDVDB_EDGE_EMBEDDINGS", true)?;

        let edge_boost_weight = parse_env::<f64>("MDVDB_EDGE_BOOST_WEIGHT", 0.15)?;

        let edge_cluster_rebalance = parse_env::<usize>("MDVDB_EDGE_CLUSTER_REBALANCE", 50)?;

        let custom_cluster_defs = parse_custom_clusters("MDVDB_CUSTOM_CLUSTERS");

        let config = Self {
            embedding_provider,
            embedding_model,
            embedding_dimensions,
            embedding_batch_size,
            openai_api_key,
            ollama_host,
            embedding_endpoint,
            source_dirs,
            ignore_patterns,
            watch_enabled,
            watch_debounce_ms,
            chunk_max_tokens,
            chunk_overlap_tokens,
            clustering_enabled,
            clustering_rebalance_threshold,
            clustering_granularity,
            search_default_limit,
            search_min_score,
            search_default_mode,
            search_rrf_k,
            bm25_norm_k,
            search_decay_enabled,
            search_decay_half_life,
            search_decay_exclude,
            search_decay_include,
            search_boost_links,
            search_boost_hops,
            search_expand_graph,
            search_expand_limit,
            vector_quantization,
            index_compression,
            edge_embeddings,
            edge_boost_weight,
            edge_cluster_rebalance,
            custom_cluster_defs,
        };

        config.validate()?;
        Ok(config)
    }

    /// Validate constraint invariants on the loaded config.
    fn validate(&self) -> Result<(), Error> {
        if self.embedding_dimensions == 0 {
            return Err(Error::Config("embedding_dimensions must be > 0".into()));
        }
        if self.embedding_batch_size == 0 {
            return Err(Error::Config("embedding_batch_size must be > 0".into()));
        }
        if self.chunk_overlap_tokens >= self.chunk_max_tokens {
            return Err(Error::Config(format!(
                "chunk_overlap_tokens ({}) must be less than chunk_max_tokens ({})",
                self.chunk_overlap_tokens, self.chunk_max_tokens
            )));
        }
        if self.search_rrf_k <= 0.0 {
            return Err(Error::Config("search_rrf_k must be > 0".into()));
        }
        if self.bm25_norm_k <= 0.0 {
            return Err(Error::Config("bm25_norm_k must be > 0".into()));
        }
        if !(0.0..=1.0).contains(&self.search_min_score) {
            return Err(Error::Config(format!(
                "search_min_score ({}) must be in [0.0, 1.0]",
                self.search_min_score
            )));
        }
        if self.search_decay_half_life <= 0.0 {
            return Err(Error::Config(
                "search_decay_half_life must be > 0".into(),
            ));
        }
        if !(1..=3).contains(&self.search_boost_hops) {
            return Err(Error::Config(format!(
                "search_boost_hops ({}) must be in [1, 3]",
                self.search_boost_hops
            )));
        }
        if self.search_expand_graph > 3 {
            return Err(Error::Config(format!(
                "search_expand_graph ({}) must be in [0, 3]",
                self.search_expand_graph
            )));
        }
        if !(1..=10).contains(&self.search_expand_limit) {
            return Err(Error::Config(format!(
                "search_expand_limit ({}) must be in [1, 10]",
                self.search_expand_limit
            )));
        }
        if !(0.0..=1.0).contains(&self.edge_boost_weight) {
            return Err(Error::Config(format!(
                "edge_boost_weight ({}) must be in [0.0, 1.0]",
                self.edge_boost_weight
            )));
        }
        if self.edge_cluster_rebalance == 0 {
            return Err(Error::Config(
                "edge_cluster_rebalance must be > 0".into(),
            ));
        }
        if !(0.25..=4.0).contains(&self.clustering_granularity) {
            return Err(Error::Config(format!(
                "clustering_granularity ({}) must be in [0.25, 4.0]",
                self.clustering_granularity
            )));
        }
        // Check for duplicate custom cluster names.
        let mut seen_names = std::collections::HashSet::new();
        for def in &self.custom_cluster_defs {
            if !seen_names.insert(&def.name) {
                return Err(Error::Config(format!(
                    "duplicate custom cluster name: '{}'",
                    def.name
                )));
            }
        }
        Ok(())
    }
}

/// Read an env var or return a default string value.
fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Parse an env var into a typed value, using a default if not set.
fn parse_env<T>(key: &str, default: T) -> Result<T, Error>
where
    T: FromStr + ToString,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(val) => val
            .parse::<T>()
            .map_err(|e| Error::Config(format!("failed to parse {key}='{val}': {e}"))),
        Err(_) => Ok(default),
    }
}

/// Parse a boolean env var (true/false/1/0).
fn parse_env_bool(key: &str, default: bool) -> Result<bool, Error> {
    match std::env::var(key) {
        Ok(val) => match val.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(Error::Config(format!(
                "failed to parse {key}='{val}': expected true/false/1/0/yes/no"
            ))),
        },
        Err(_) => Ok(default),
    }
}

/// Parse a comma-separated env var into Vec<PathBuf>, trimming whitespace.
fn parse_comma_list_path(key: &str, default: Vec<PathBuf>) -> Vec<PathBuf> {
    match std::env::var(key) {
        Ok(val) if !val.trim().is_empty() => {
            val.split(',').map(|s| PathBuf::from(s.trim())).collect()
        }
        _ => default,
    }
}

/// Parse a comma-separated env var into Vec<String>, trimming whitespace.
fn parse_comma_list_string(key: &str, default: Vec<String>) -> Vec<String> {
    match std::env::var(key) {
        Ok(val) if !val.trim().is_empty() => val.split(',').map(|s| s.trim().to_string()).collect(),
        _ => default,
    }
}

/// Parse `MDVDB_CUSTOM_CLUSTERS` env var into custom cluster definitions.
///
/// Format: `Name1:seed1,seed2|Name2:seed3,seed4`
/// - Pipe `|` separates clusters
/// - Colon `:` separates name from seeds
/// - Comma `,` separates seeds within a cluster
fn parse_custom_clusters(key: &str) -> Vec<CustomClusterDef> {
    match std::env::var(key) {
        Ok(val) if !val.trim().is_empty() => val
            .split('|')
            .filter_map(|entry| {
                let (name, seeds_str) = entry.split_once(':')?;
                let name = name.trim().to_string();
                if name.is_empty() {
                    return None;
                }
                let seeds: Vec<String> = seeds_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if seeds.is_empty() {
                    return None;
                }
                Some(CustomClusterDef { name, seeds })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Update a single key=value line in a config file, preserving other lines.
///
/// If `value` is empty, the line is removed. If the key doesn't exist, it's appended.
/// Creates the file and parent directories if they don't exist.
pub fn update_config_value(config_path: &Path, key: &str, value: &str) -> Result<(), Error> {
    use std::fs;
    use std::io::Write;

    // Ensure parent directory exists.
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::Config(format!(
                "failed to create config directory '{}': {e}",
                parent.display()
            ))
        })?;
    }

    // Read existing content (or start fresh).
    let content = fs::read_to_string(config_path).unwrap_or_default();
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    // Quote value if it contains spaces or special characters for dotenvy compatibility.
    let needs_quoting = value.contains(' ') || value.contains('#');
    let formatted_value = if needs_quoting && !value.is_empty() {
        format!("{key}=\"{value}\"")
    } else {
        format!("{key}={value}")
    };

    let prefix = format!("{key}=");
    let quoted_prefix = format!("{key}=\"");
    let mut found = false;

    for line in &mut lines {
        if line.starts_with(&prefix) || line.starts_with(&quoted_prefix) || line.starts_with(&format!("{key} =")) {
            if value.is_empty() {
                // Mark for removal by clearing.
                *line = String::new();
                found = true;
            } else {
                *line = formatted_value.clone();
                found = true;
            }
            break;
        }
    }

    if value.is_empty() {
        // Remove empty lines that were cleared.
        lines.retain(|l| !l.is_empty() || !found);
        // If we didn't find the key, nothing to do.
    } else if !found {
        lines.push(formatted_value);
    }

    let mut file = fs::File::create(config_path).map_err(|e| {
        Error::Config(format!(
            "failed to write config file '{}': {e}",
            config_path.display()
        ))
    })?;
    for line in &lines {
        writeln!(file, "{line}").map_err(|e| {
            Error::Config(format!("failed to write config: {e}"))
        })?;
    }

    Ok(())
}

/// Parse a raw custom clusters string value into definitions.
///
/// This is the public counterpart to `parse_custom_clusters()` for use outside
/// the config loading path (e.g., the CLI `clusters add/remove` commands).
pub fn parse_custom_clusters_value(val: &str) -> Vec<CustomClusterDef> {
    if val.trim().is_empty() {
        return Vec::new();
    }
    val.split('|')
        .filter_map(|entry| {
            let (name, seeds_str) = entry.split_once(':')?;
            let name = name.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let seeds: Vec<String> = seeds_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if seeds.is_empty() {
                return None;
            }
            Some(CustomClusterDef { name, seeds })
        })
        .collect()
}

/// Encode custom cluster definitions back to the dotenv format.
///
/// Format: `Name1:seed1,seed2|Name2:seed3,seed4`
pub fn encode_custom_clusters(defs: &[CustomClusterDef]) -> String {
    defs.iter()
        .map(|d| format!("{}:{}", d.name, d.seeds.join(",")))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that read/write environment variables.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn provider_type_case_insensitive() {
        assert_eq!(
            "openai".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::OpenAI
        );
        assert_eq!(
            "OpenAI".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::OpenAI
        );
        assert_eq!(
            "OPENAI".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::OpenAI
        );
        assert_eq!(
            "ollama".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::Ollama
        );
        assert_eq!(
            "Ollama".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::Ollama
        );
        assert_eq!(
            "custom".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::Custom
        );
        assert_eq!(
            "CUSTOM".parse::<EmbeddingProviderType>().unwrap(),
            EmbeddingProviderType::Custom
        );
    }

    #[test]
    fn provider_type_unknown_rejected() {
        let result = "unknown".parse::<EmbeddingProviderType>();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown"));
    }

    #[test]
    fn default_values_match_spec() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Clear all MDVDB env vars to ensure defaults
        let vars_to_clear = [
            "MDVDB_EMBEDDING_PROVIDER",
            "MDVDB_EMBEDDING_MODEL",
            "MDVDB_EMBEDDING_DIMENSIONS",
            "MDVDB_EMBEDDING_BATCH_SIZE",
            "OPENAI_API_KEY",
            "OLLAMA_HOST",
            "MDVDB_EMBEDDING_ENDPOINT",
            "MDVDB_SOURCE_DIRS",
            "MDVDB_IGNORE_PATTERNS",
            "MDVDB_WATCH",
            "MDVDB_WATCH_DEBOUNCE_MS",
            "MDVDB_CHUNK_MAX_TOKENS",
            "MDVDB_CHUNK_OVERLAP_TOKENS",
            "MDVDB_CLUSTERING_ENABLED",
            "MDVDB_CLUSTERING_REBALANCE_THRESHOLD",
            "MDVDB_CLUSTER_GRANULARITY",
            "MDVDB_SEARCH_DEFAULT_LIMIT",
            "MDVDB_SEARCH_MIN_SCORE",
            "MDVDB_SEARCH_MODE",
            "MDVDB_SEARCH_RRF_K",
            "MDVDB_BM25_NORM_K",
            "MDVDB_SEARCH_DECAY",
            "MDVDB_SEARCH_DECAY_HALF_LIFE",
            "MDVDB_SEARCH_DECAY_EXCLUDE",
            "MDVDB_SEARCH_DECAY_INCLUDE",
            "MDVDB_SEARCH_BOOST_LINKS",
            "MDVDB_SEARCH_BOOST_HOPS",
            "MDVDB_SEARCH_EXPAND_GRAPH",
            "MDVDB_SEARCH_EXPAND_LIMIT",
            "MDVDB_VECTOR_QUANTIZATION",
            "MDVDB_INDEX_COMPRESSION",
            "MDVDB_EDGE_EMBEDDINGS",
            "MDVDB_EDGE_BOOST_WEIGHT",
            "MDVDB_EDGE_CLUSTER_REBALANCE",
        ];
        // Save original values so we can restore them after the test
        let saved: Vec<(&str, Option<String>)> = vars_to_clear
            .iter()
            .map(|v| (*v, std::env::var(v).ok()))
            .collect();
        for var in &vars_to_clear {
            std::env::remove_var(var);
        }
        // Disable user config file (~/.mdvdb/config) so it doesn't inject values
        let had_no_user = std::env::var("MDVDB_NO_USER_CONFIG").ok();
        std::env::set_var("MDVDB_NO_USER_CONFIG", "1");

        // Load from a non-existent dir to get pure defaults
        let config = Config::load(Path::new("/nonexistent")).unwrap();

        // Restore original env vars before assertions (prevents poison on failure)
        match &had_no_user {
            Some(v) => std::env::set_var("MDVDB_NO_USER_CONFIG", v),
            None => std::env::remove_var("MDVDB_NO_USER_CONFIG"),
        }
        for (var, val) in &saved {
            match val {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }

        assert_eq!(config.embedding_provider, EmbeddingProviderType::OpenAI);
        assert_eq!(config.embedding_model, "text-embedding-3-small");
        assert_eq!(config.embedding_dimensions, 1536);
        assert_eq!(config.embedding_batch_size, 100);
        assert_eq!(config.openai_api_key, None);
        assert_eq!(config.ollama_host, "http://localhost:11434");
        assert_eq!(config.embedding_endpoint, None);
        assert_eq!(config.source_dirs, vec![PathBuf::from(".")]);
        assert!(config.ignore_patterns.is_empty());
        assert!(config.watch_enabled);
        assert_eq!(config.watch_debounce_ms, 300);
        assert_eq!(config.chunk_max_tokens, 512);
        assert_eq!(config.chunk_overlap_tokens, 50);
        assert!(config.clustering_enabled);
        assert_eq!(config.clustering_rebalance_threshold, 50);
        assert!((config.clustering_granularity - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.search_default_limit, 10);
        assert_eq!(config.search_min_score, 0.0);
        assert_eq!(config.search_default_mode, SearchMode::Hybrid);
        assert_eq!(config.search_rrf_k, 60.0);
        assert_eq!(config.bm25_norm_k, 1.5);
        assert!(!config.search_decay_enabled);
        assert_eq!(config.search_decay_half_life, 90.0);
        assert!(config.search_decay_exclude.is_empty());
        assert!(config.search_decay_include.is_empty());
        assert!(!config.search_boost_links);
        assert_eq!(config.search_boost_hops, 1);
        assert_eq!(config.search_expand_graph, 0);
        assert_eq!(config.search_expand_limit, 3);
        assert_eq!(config.vector_quantization, VectorQuantization::F16);
        assert!(config.index_compression);
        assert!(config.edge_embeddings);
        assert_eq!(config.edge_boost_weight, 0.15);
        assert_eq!(config.edge_cluster_rebalance, 50);
    }

    #[test]
    fn validation_rejects_zero_dimensions() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_EMBEDDING_DIMENSIONS");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("embedding_dimensions"));
    }

    #[test]
    fn validation_rejects_zero_batch_size() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_EMBEDDING_BATCH_SIZE", "0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_EMBEDDING_BATCH_SIZE");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("embedding_batch_size"));
    }

    #[test]
    fn validation_rejects_overlap_exceeds_max() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_CHUNK_MAX_TOKENS", "10");
        std::env::set_var("MDVDB_CHUNK_OVERLAP_TOKENS", "20");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_CHUNK_MAX_TOKENS");
        std::env::remove_var("MDVDB_CHUNK_OVERLAP_TOKENS");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("chunk_overlap_tokens"));
    }

    #[test]
    fn validation_rejects_score_out_of_range() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_SEARCH_MIN_SCORE", "1.5");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_MIN_SCORE");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("search_min_score"));
    }

    #[test]
    fn validation_rejects_negative_score() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_SEARCH_MIN_SCORE", "-0.1");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_MIN_SCORE");
        assert!(result.is_err());
    }

    #[test]
    fn validation_rejects_zero_rrf_k() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_SEARCH_RRF_K", "0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_RRF_K");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("search_rrf_k"));
    }

    #[test]
    fn validation_rejects_negative_rrf_k() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_SEARCH_RRF_K", "-10.0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_RRF_K");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("search_rrf_k"));
    }

    #[test]
    fn parse_error_on_non_numeric() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "abc");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_EMBEDDING_DIMENSIONS");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("MDVDB_EMBEDDING_DIMENSIONS"));
    }

    #[test]
    fn comma_separated_source_dirs() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_SOURCE_DIRS", " docs , notes ");
        let dirs = parse_comma_list_path("MDVDB_SOURCE_DIRS", vec![]);
        std::env::remove_var("MDVDB_SOURCE_DIRS");
        assert_eq!(dirs, vec![PathBuf::from("docs"), PathBuf::from("notes")]);
    }

    #[test]
    fn comma_separated_ignore_patterns() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_IGNORE_PATTERNS", " *.tmp , .git ");
        let patterns = parse_comma_list_string("MDVDB_IGNORE_PATTERNS", vec![]);
        std::env::remove_var("MDVDB_IGNORE_PATTERNS");
        assert_eq!(patterns, vec!["*.tmp".to_string(), ".git".to_string()]);
    }

    #[test]
    fn user_config_dir_uses_mdvdb_config_home() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_CONFIG_HOME", "/custom/config");
        let dir = Config::user_config_dir();
        std::env::remove_var("MDVDB_CONFIG_HOME");
        assert_eq!(dir, Some(PathBuf::from("/custom/config")));
    }

    #[test]
    fn user_config_dir_empty_env_falls_back() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_CONFIG_HOME", "");
        let dir = Config::user_config_dir();
        std::env::remove_var("MDVDB_CONFIG_HOME");
        // Falls back to ~/.mdvdb (home dir dependent).
        assert!(dir.is_some());
        let path = dir.unwrap();
        assert!(path.ends_with(".mdvdb"));
    }

    #[test]
    fn user_config_dir_unset_falls_back() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MDVDB_CONFIG_HOME");
        let dir = Config::user_config_dir();
        // Falls back to ~/.mdvdb (home dir dependent).
        assert!(dir.is_some());
        let path = dir.unwrap();
        assert!(path.ends_with(".mdvdb"));
    }

    #[test]
    fn user_config_path_appends_config() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_CONFIG_HOME", "/custom");
        let path = Config::user_config_path();
        std::env::remove_var("MDVDB_CONFIG_HOME");
        assert_eq!(path, Some(PathBuf::from("/custom/config")));
    }

    #[test]
    fn no_user_config_env_skips_user_config() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Clear all MDVDB vars first.
        for var in &[
            "MDVDB_EMBEDDING_PROVIDER", "MDVDB_EMBEDDING_MODEL",
            "MDVDB_EMBEDDING_DIMENSIONS", "MDVDB_EMBEDDING_BATCH_SIZE",
            "OPENAI_API_KEY", "OLLAMA_HOST", "MDVDB_EMBEDDING_ENDPOINT",
            "MDVDB_SOURCE_DIRS", "MDVDB_IGNORE_PATTERNS",
            "MDVDB_WATCH", "MDVDB_WATCH_DEBOUNCE_MS",
            "MDVDB_CHUNK_MAX_TOKENS", "MDVDB_CHUNK_OVERLAP_TOKENS",
            "MDVDB_CLUSTERING_ENABLED", "MDVDB_CLUSTERING_REBALANCE_THRESHOLD",
            "MDVDB_CLUSTER_GRANULARITY",
            "MDVDB_SEARCH_DEFAULT_LIMIT", "MDVDB_SEARCH_MIN_SCORE",
            "MDVDB_SEARCH_MODE", "MDVDB_SEARCH_RRF_K", "MDVDB_BM25_NORM_K",
            "MDVDB_SEARCH_DECAY", "MDVDB_SEARCH_DECAY_HALF_LIFE",
            "MDVDB_SEARCH_DECAY_EXCLUDE", "MDVDB_SEARCH_DECAY_INCLUDE",
            "MDVDB_SEARCH_BOOST_LINKS",
            "MDVDB_SEARCH_BOOST_HOPS", "MDVDB_SEARCH_EXPAND_GRAPH",
            "MDVDB_SEARCH_EXPAND_LIMIT",
            "MDVDB_VECTOR_QUANTIZATION", "MDVDB_INDEX_COMPRESSION",
            "MDVDB_EDGE_EMBEDDINGS", "MDVDB_EDGE_BOOST_WEIGHT",
            "MDVDB_EDGE_CLUSTER_REBALANCE",
        ] {
            std::env::remove_var(var);
        }

        // Create a temp user config that sets a specific model.
        let temp = tempfile::TempDir::new().unwrap();
        let config_path = temp.path().join("config");
        std::fs::write(&config_path, "MDVDB_EMBEDDING_MODEL=custom-model\n").unwrap();

        std::env::set_var("MDVDB_CONFIG_HOME", temp.path());
        std::env::set_var("MDVDB_NO_USER_CONFIG", "1");

        let config = Config::load(Path::new("/nonexistent")).unwrap();

        std::env::remove_var("MDVDB_CONFIG_HOME");
        std::env::remove_var("MDVDB_NO_USER_CONFIG");

        // Should get default model, not the user config one.
        assert_eq!(config.embedding_model, "text-embedding-3-small");
    }

    #[test]
    fn validation_rejects_edge_boost_weight_out_of_range() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_EDGE_BOOST_WEIGHT", "1.5");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_EDGE_BOOST_WEIGHT");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("edge_boost_weight"));
    }

    #[test]
    fn validation_rejects_negative_edge_boost_weight() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_EDGE_BOOST_WEIGHT", "-0.1");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_EDGE_BOOST_WEIGHT");
        assert!(result.is_err());
    }

    #[test]
    fn validation_rejects_zero_edge_cluster_rebalance() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MDVDB_EDGE_CLUSTER_REBALANCE", "0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_EDGE_CLUSTER_REBALANCE");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("edge_cluster_rebalance"));
    }
}
