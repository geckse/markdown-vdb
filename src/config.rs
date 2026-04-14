use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::clustering::CustomClusterDef;
use crate::error::Error;
use crate::search::SearchMode;

// ---------------------------------------------------------------------------
// YAML configuration types (intermediate deserialization target)
// ---------------------------------------------------------------------------

/// Top-level YAML configuration structure.
/// Deserialized from `.markdownvdb/config.yaml` or `~/.mdvdb/config.yaml`.
/// Converted to the runtime `Config` via `Config::from_yaml()`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct YamlConfig {
    pub embedding: YamlEmbedding,
    pub search: YamlSearch,
    pub chunking: YamlChunking,
    pub clustering: YamlClustering,
    pub watch: YamlWatch,
    pub index: YamlIndex,
    pub sources: YamlSources,
}

/// Embedding provider settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlEmbedding {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub batch_size: usize,
    pub endpoint: Option<String>,
}

impl Default for YamlEmbedding {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
            batch_size: 100,
            endpoint: None,
        }
    }
}

/// Search engine settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlSearch {
    pub limit: usize,
    pub min_score: f64,
    pub mode: String,
    pub rrf_k: f64,
    pub bm25_norm_k: f64,
    pub boost_links: bool,
    pub boost_hops: usize,
    pub expand_graph: usize,
    pub expand_limit: usize,
    pub decay: YamlDecay,
}

impl Default for YamlSearch {
    fn default() -> Self {
        Self {
            limit: 10,
            min_score: 0.0,
            mode: "hybrid".to_string(),
            rrf_k: 60.0,
            bm25_norm_k: 1.5,
            boost_links: false,
            boost_hops: 1,
            expand_graph: 0,
            expand_limit: 3,
            decay: YamlDecay::default(),
        }
    }
}

/// Time decay settings for search scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlDecay {
    pub enabled: bool,
    pub half_life: f64,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
}

impl Default for YamlDecay {
    fn default() -> Self {
        Self {
            enabled: false,
            half_life: 90.0,
            exclude: Vec::new(),
            include: Vec::new(),
        }
    }
}

/// Chunking engine settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlChunking {
    pub max_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for YamlChunking {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            overlap_tokens: 50,
        }
    }
}

/// Clustering settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlClustering {
    pub enabled: bool,
    pub rebalance_threshold: usize,
    pub granularity: f64,
    pub custom: Vec<YamlCustomCluster>,
}

impl Default for YamlClustering {
    fn default() -> Self {
        Self {
            enabled: true,
            rebalance_threshold: 50,
            granularity: 1.0,
            custom: Vec::new(),
        }
    }
}

/// A single custom cluster definition in YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlCustomCluster {
    pub name: String,
    pub seeds: Vec<String>,
}

/// File watcher settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlWatch {
    pub enabled: bool,
    pub debounce_ms: u64,
}

impl Default for YamlWatch {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 300,
        }
    }
}

/// Index storage settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlIndex {
    pub quantization: String,
    pub compression: bool,
    pub edge_embeddings: bool,
    pub edge_boost_weight: f64,
    pub edge_cluster_rebalance: usize,
}

impl Default for YamlIndex {
    fn default() -> Self {
        Self {
            quantization: "f16".to_string(),
            compression: true,
            edge_embeddings: true,
            edge_boost_weight: 0.15,
            edge_cluster_rebalance: 50,
        }
    }
}

/// Source directory settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlSources {
    pub dirs: Vec<String>,
    pub ignore: Vec<String>,
}

impl Default for YamlSources {
    fn default() -> Self {
        Self {
            dirs: vec![".".to_string()],
            ignore: Vec::new(),
        }
    }
}

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

// ---------------------------------------------------------------------------
// YAML pipeline functions
// ---------------------------------------------------------------------------

/// Recursively merge two YAML values. Mappings merge recursively; scalars and
/// sequences from `overlay` replace `base`.
pub fn merge_yaml_values(base: serde_yaml::Value, overlay: serde_yaml::Value) -> serde_yaml::Value {
    match (base, overlay) {
        (serde_yaml::Value::Mapping(mut base_map), serde_yaml::Value::Mapping(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged = if let Some(base_val) = base_map.remove(&key) {
                    merge_yaml_values(base_val, overlay_val)
                } else {
                    overlay_val
                };
                base_map.insert(key, merged);
            }
            serde_yaml::Value::Mapping(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// Apply environment variable overrides to a `YamlConfig`.
/// Each MDVDB_* env var overrides the corresponding field when set.
pub fn apply_env_overrides(yaml: &mut YamlConfig) {
    // Helper closures to reduce repetition
    fn env_str(key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
    fn env_usize(key: &str) -> Option<usize> {
        env_str(key).and_then(|v| v.parse().ok())
    }
    fn env_f64(key: &str) -> Option<f64> {
        env_str(key).and_then(|v| v.parse().ok())
    }
    fn env_u64(key: &str) -> Option<u64> {
        env_str(key).and_then(|v| v.parse().ok())
    }
    fn env_bool(key: &str) -> Option<bool> {
        env_str(key).and_then(|v| match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        })
    }
    fn env_comma_list(key: &str) -> Option<Vec<String>> {
        env_str(key).map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
    }

    // Embedding
    if let Some(v) = env_str("MDVDB_EMBEDDING_PROVIDER") { yaml.embedding.provider = v; }
    if let Some(v) = env_str("MDVDB_EMBEDDING_MODEL") { yaml.embedding.model = v; }
    if let Some(v) = env_usize("MDVDB_EMBEDDING_DIMENSIONS") { yaml.embedding.dimensions = v; }
    if let Some(v) = env_usize("MDVDB_EMBEDDING_BATCH_SIZE") { yaml.embedding.batch_size = v; }
    if let Some(v) = env_str("MDVDB_EMBEDDING_ENDPOINT") { yaml.embedding.endpoint = Some(v); }

    // Search
    if let Some(v) = env_usize("MDVDB_SEARCH_DEFAULT_LIMIT") { yaml.search.limit = v; }
    if let Some(v) = env_f64("MDVDB_SEARCH_MIN_SCORE") { yaml.search.min_score = v; }
    if let Some(v) = env_str("MDVDB_SEARCH_MODE") { yaml.search.mode = v; }
    if let Some(v) = env_f64("MDVDB_SEARCH_RRF_K") { yaml.search.rrf_k = v; }
    if let Some(v) = env_f64("MDVDB_BM25_NORM_K") { yaml.search.bm25_norm_k = v; }
    if let Some(v) = env_bool("MDVDB_SEARCH_BOOST_LINKS") { yaml.search.boost_links = v; }
    if let Some(v) = env_usize("MDVDB_SEARCH_BOOST_HOPS") { yaml.search.boost_hops = v; }
    if let Some(v) = env_usize("MDVDB_SEARCH_EXPAND_GRAPH") { yaml.search.expand_graph = v; }
    if let Some(v) = env_usize("MDVDB_SEARCH_EXPAND_LIMIT") { yaml.search.expand_limit = v; }

    // Decay
    if let Some(v) = env_bool("MDVDB_SEARCH_DECAY") { yaml.search.decay.enabled = v; }
    if let Some(v) = env_f64("MDVDB_SEARCH_DECAY_HALF_LIFE") { yaml.search.decay.half_life = v; }
    if let Some(v) = env_comma_list("MDVDB_SEARCH_DECAY_EXCLUDE") { yaml.search.decay.exclude = v; }
    if let Some(v) = env_comma_list("MDVDB_SEARCH_DECAY_INCLUDE") { yaml.search.decay.include = v; }

    // Chunking
    if let Some(v) = env_usize("MDVDB_CHUNK_MAX_TOKENS") { yaml.chunking.max_tokens = v; }
    if let Some(v) = env_usize("MDVDB_CHUNK_OVERLAP_TOKENS") { yaml.chunking.overlap_tokens = v; }

    // Clustering
    if let Some(v) = env_bool("MDVDB_CLUSTERING_ENABLED") { yaml.clustering.enabled = v; }
    if let Some(v) = env_usize("MDVDB_CLUSTERING_REBALANCE_THRESHOLD") { yaml.clustering.rebalance_threshold = v; }
    if let Some(v) = env_f64("MDVDB_CLUSTER_GRANULARITY") { yaml.clustering.granularity = v; }
    if let Some(v) = env_str("MDVDB_CUSTOM_CLUSTERS") {
        yaml.clustering.custom = parse_custom_clusters_value(&v)
            .into_iter()
            .map(|d| YamlCustomCluster { name: d.name, seeds: d.seeds })
            .collect();
    }

    // Watch
    if let Some(v) = env_bool("MDVDB_WATCH") { yaml.watch.enabled = v; }
    if let Some(v) = env_u64("MDVDB_WATCH_DEBOUNCE_MS") { yaml.watch.debounce_ms = v; }

    // Index
    if let Some(v) = env_str("MDVDB_VECTOR_QUANTIZATION") { yaml.index.quantization = v; }
    if let Some(v) = env_bool("MDVDB_INDEX_COMPRESSION") { yaml.index.compression = v; }
    if let Some(v) = env_bool("MDVDB_EDGE_EMBEDDINGS") { yaml.index.edge_embeddings = v; }
    if let Some(v) = env_f64("MDVDB_EDGE_BOOST_WEIGHT") { yaml.index.edge_boost_weight = v; }
    if let Some(v) = env_usize("MDVDB_EDGE_CLUSTER_REBALANCE") { yaml.index.edge_cluster_rebalance = v; }

    // Sources
    if let Some(v) = env_comma_list("MDVDB_SOURCE_DIRS") { yaml.sources.dirs = v; }
    if let Some(v) = env_comma_list("MDVDB_IGNORE_PATTERNS") { yaml.sources.ignore = v; }
}

impl Config {
    /// Convert a `YamlConfig` into the runtime `Config`.
    ///
    /// Parses string enums, reads secrets from the environment, converts types,
    /// and validates the result.
    pub fn from_yaml(yaml: YamlConfig, _project_root: &Path) -> Result<Self, Error> {
        let embedding_provider = yaml.embedding.provider.parse::<EmbeddingProviderType>()?;
        let search_default_mode = yaml.search.mode.parse::<SearchMode>()?;
        let vector_quantization = yaml.index.quantization.parse::<VectorQuantization>()?;

        let openai_api_key = std::env::var("OPENAI_API_KEY").ok();
        let ollama_host = std::env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());

        let source_dirs = yaml.sources.dirs.iter().map(PathBuf::from).collect();
        let custom_cluster_defs = yaml
            .clustering
            .custom
            .into_iter()
            .map(|c| CustomClusterDef {
                name: c.name,
                seeds: c.seeds,
            })
            .collect();

        let config = Self {
            embedding_provider,
            embedding_model: yaml.embedding.model,
            embedding_dimensions: yaml.embedding.dimensions,
            embedding_batch_size: yaml.embedding.batch_size,
            openai_api_key,
            ollama_host,
            embedding_endpoint: yaml.embedding.endpoint,
            source_dirs,
            ignore_patterns: yaml.sources.ignore,
            watch_enabled: yaml.watch.enabled,
            watch_debounce_ms: yaml.watch.debounce_ms,
            chunk_max_tokens: yaml.chunking.max_tokens,
            chunk_overlap_tokens: yaml.chunking.overlap_tokens,
            clustering_enabled: yaml.clustering.enabled,
            clustering_rebalance_threshold: yaml.clustering.rebalance_threshold,
            clustering_granularity: yaml.clustering.granularity,
            search_default_limit: yaml.search.limit,
            search_min_score: yaml.search.min_score,
            search_default_mode,
            search_rrf_k: yaml.search.rrf_k,
            bm25_norm_k: yaml.search.bm25_norm_k,
            search_decay_enabled: yaml.search.decay.enabled,
            search_decay_half_life: yaml.search.decay.half_life,
            search_decay_exclude: yaml.search.decay.exclude,
            search_decay_include: yaml.search.decay.include,
            search_boost_links: yaml.search.boost_links,
            search_boost_hops: yaml.search.boost_hops,
            search_expand_graph: yaml.search.expand_graph,
            search_expand_limit: yaml.search.expand_limit,
            vector_quantization,
            index_compression: yaml.index.compression,
            edge_embeddings: yaml.index.edge_embeddings,
            edge_boost_weight: yaml.index.edge_boost_weight,
            edge_cluster_rebalance: yaml.index.edge_cluster_rebalance,
            custom_cluster_defs,
        };

        config.validate()?;
        Ok(config)
    }
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

    #[test]
    fn yaml_config_defaults() {
        let cfg = YamlConfig::default();

        // Embedding defaults
        assert_eq!(cfg.embedding.provider, "openai");
        assert_eq!(cfg.embedding.model, "text-embedding-3-small");
        assert_eq!(cfg.embedding.dimensions, 1536);
        assert_eq!(cfg.embedding.batch_size, 100);
        assert!(cfg.embedding.endpoint.is_none());

        // Search defaults
        assert_eq!(cfg.search.limit, 10);
        assert_eq!(cfg.search.min_score, 0.0);
        assert_eq!(cfg.search.mode, "hybrid");
        assert_eq!(cfg.search.rrf_k, 60.0);
        assert_eq!(cfg.search.bm25_norm_k, 1.5);
        assert!(!cfg.search.boost_links);
        assert_eq!(cfg.search.boost_hops, 1);
        assert_eq!(cfg.search.expand_graph, 0);
        assert_eq!(cfg.search.expand_limit, 3);

        // Decay defaults
        assert!(!cfg.search.decay.enabled);
        assert_eq!(cfg.search.decay.half_life, 90.0);
        assert!(cfg.search.decay.exclude.is_empty());
        assert!(cfg.search.decay.include.is_empty());

        // Chunking defaults
        assert_eq!(cfg.chunking.max_tokens, 512);
        assert_eq!(cfg.chunking.overlap_tokens, 50);

        // Clustering defaults
        assert!(cfg.clustering.enabled);
        assert_eq!(cfg.clustering.rebalance_threshold, 50);
        assert!((cfg.clustering.granularity - 1.0).abs() < f64::EPSILON);
        assert!(cfg.clustering.custom.is_empty());

        // Watch defaults
        assert!(cfg.watch.enabled);
        assert_eq!(cfg.watch.debounce_ms, 300);

        // Index defaults
        assert_eq!(cfg.index.quantization, "f16");
        assert!(cfg.index.compression);
        assert!(cfg.index.edge_embeddings);
        assert_eq!(cfg.index.edge_boost_weight, 0.15);
        assert_eq!(cfg.index.edge_cluster_rebalance, 50);

        // Sources defaults
        assert_eq!(cfg.sources.dirs, vec![".".to_string()]);
        assert!(cfg.sources.ignore.is_empty());
    }

    #[test]
    fn yaml_config_partial_deserialize() {
        let yaml = r#"
embedding:
  provider: ollama
search:
  limit: 25
"#;
        let cfg: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        // Specified fields
        assert_eq!(cfg.embedding.provider, "ollama");
        assert_eq!(cfg.search.limit, 25);

        // Everything else should be defaults
        assert_eq!(cfg.embedding.model, "text-embedding-3-small");
        assert_eq!(cfg.embedding.dimensions, 1536);
        assert_eq!(cfg.search.mode, "hybrid");
        assert_eq!(cfg.search.rrf_k, 60.0);
        assert!(!cfg.search.decay.enabled);
        assert_eq!(cfg.chunking.max_tokens, 512);
        assert!(cfg.clustering.enabled);
        assert!(cfg.watch.enabled);
        assert_eq!(cfg.index.quantization, "f16");
        assert_eq!(cfg.sources.dirs, vec![".".to_string()]);
    }

    #[test]
    fn yaml_custom_cluster_roundtrip() {
        let cluster = YamlCustomCluster {
            name: "TestCluster".to_string(),
            seeds: vec!["seed1".to_string(), "seed2".to_string(), "seed3".to_string()],
        };

        let yaml_str = serde_yaml::to_string(&cluster).unwrap();
        let deserialized: YamlCustomCluster = serde_yaml::from_str(&yaml_str).unwrap();

        assert_eq!(deserialized.name, "TestCluster");
        assert_eq!(deserialized.seeds, vec!["seed1", "seed2", "seed3"]);
    }

    // --- merge_yaml_values tests ---

    #[test]
    fn merge_yaml_values_basic() {
        let base: serde_yaml::Value = serde_yaml::from_str("a: 1\nb: 2").unwrap();
        let overlay: serde_yaml::Value = serde_yaml::from_str("b: 99").unwrap();
        let merged = merge_yaml_values(base, overlay);
        let map = merged.as_mapping().unwrap();
        assert_eq!(map[&serde_yaml::Value::String("a".into())], serde_yaml::Value::Number(1.into()));
        assert_eq!(map[&serde_yaml::Value::String("b".into())], serde_yaml::Value::Number(99.into()));
    }

    #[test]
    fn merge_yaml_values_nested() {
        let base: serde_yaml::Value = serde_yaml::from_str("top:\n  a: 1\n  b: 2\nother: 3").unwrap();
        let overlay: serde_yaml::Value = serde_yaml::from_str("top:\n  b: 99").unwrap();
        let merged = merge_yaml_values(base, overlay);
        let top = merged["top"].as_mapping().unwrap();
        assert_eq!(top[&serde_yaml::Value::String("a".into())], serde_yaml::Value::Number(1.into()));
        assert_eq!(top[&serde_yaml::Value::String("b".into())], serde_yaml::Value::Number(99.into()));
        assert_eq!(merged["other"], serde_yaml::Value::Number(3.into()));
    }

    #[test]
    fn merge_yaml_values_sequence_replace() {
        let base: serde_yaml::Value = serde_yaml::from_str("items:\n  - a\n  - b").unwrap();
        let overlay: serde_yaml::Value = serde_yaml::from_str("items:\n  - x").unwrap();
        let merged = merge_yaml_values(base, overlay);
        let items = merged["items"].as_sequence().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], serde_yaml::Value::String("x".into()));
    }

    // --- apply_env_overrides tests ---

    #[test]
    fn apply_env_overrides_all_fields() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Save and set env vars
        let vars = [
            ("MDVDB_EMBEDDING_PROVIDER", "ollama"),
            ("MDVDB_SEARCH_DEFAULT_LIMIT", "25"),
            ("MDVDB_CHUNK_MAX_TOKENS", "1024"),
            ("MDVDB_CLUSTERING_ENABLED", "false"),
            ("MDVDB_WATCH_DEBOUNCE_MS", "500"),
            ("MDVDB_SOURCE_DIRS", "src,docs"),
        ];
        let saved: Vec<(&str, Option<String>)> = vars.iter().map(|(k, _)| (*k, std::env::var(k).ok())).collect();
        for (k, v) in &vars { std::env::set_var(k, v); }

        let mut yaml = YamlConfig::default();
        apply_env_overrides(&mut yaml);

        // Restore
        for (k, v) in &saved {
            match v { Some(val) => std::env::set_var(k, val), None => std::env::remove_var(k) }
        }

        assert_eq!(yaml.embedding.provider, "ollama");
        assert_eq!(yaml.search.limit, 25);
        assert_eq!(yaml.chunking.max_tokens, 1024);
        assert!(!yaml.clustering.enabled);
        assert_eq!(yaml.watch.debounce_ms, 500);
        assert_eq!(yaml.sources.dirs, vec!["src", "docs"]);
    }

    // --- from_yaml tests ---

    #[test]
    fn from_yaml_conversion() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Ensure no interfering env vars for secrets
        let saved_key = std::env::var("OPENAI_API_KEY").ok();
        let saved_host = std::env::var("OLLAMA_HOST").ok();
        std::env::remove_var("OPENAI_API_KEY");
        std::env::set_var("OLLAMA_HOST", "http://myhost:11434");

        let mut yaml = YamlConfig::default();
        yaml.embedding.provider = "ollama".to_string();
        yaml.search.mode = "semantic".to_string();
        yaml.index.quantization = "f32".to_string();
        yaml.sources.dirs = vec!["src".to_string(), "docs".to_string()];
        yaml.clustering.custom = vec![YamlCustomCluster {
            name: "Test".to_string(),
            seeds: vec!["s1".to_string(), "s2".to_string()],
        }];

        let config = Config::from_yaml(yaml, Path::new("/tmp")).unwrap();

        // Restore
        match saved_key { Some(v) => std::env::set_var("OPENAI_API_KEY", v), None => std::env::remove_var("OPENAI_API_KEY") }
        match saved_host { Some(v) => std::env::set_var("OLLAMA_HOST", v), None => std::env::remove_var("OLLAMA_HOST") }

        assert_eq!(config.embedding_provider, EmbeddingProviderType::Ollama);
        assert_eq!(config.search_default_mode, SearchMode::Semantic);
        assert_eq!(config.vector_quantization, VectorQuantization::F32);
        assert_eq!(config.source_dirs, vec![PathBuf::from("src"), PathBuf::from("docs")]);
        assert_eq!(config.ollama_host, "http://myhost:11434");
        assert_eq!(config.custom_cluster_defs.len(), 1);
        assert_eq!(config.custom_cluster_defs[0].name, "Test");
    }
}
