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

    /// Resolve the user-level config file path (~/.mdvdb/config.yaml).
    pub fn user_config_path() -> Option<PathBuf> {
        Self::user_config_dir().map(|d| d.join("config.yaml"))
    }

    /// Load configuration with priority: shell env > project YAML > user YAML > defaults.
    ///
    /// Pipeline:
    /// 1. Load `.env` for secrets (OPENAI_API_KEY, OLLAMA_HOST) via dotenvy
    /// 2. Detect & auto-migrate project config to YAML
    /// 3. Detect & auto-migrate user config to YAML
    /// 4. Load user YAML as serde_yaml::Value (or empty Mapping)
    /// 5. Load project YAML as serde_yaml::Value (or empty Mapping)
    /// 6. Deep merge project over user via merge_yaml_values()
    /// 7. Deserialize merged Value into YamlConfig
    /// 8. Apply env var overrides via apply_env_overrides()
    /// 9. Convert via Config::from_yaml()
    pub fn load(project_root: &Path) -> Result<Self, Error> {
        use std::fs;

        // (1) Load .env for secrets (does NOT override existing env vars).
        // Capture which MDVDB_* env vars exist BEFORE .env load, so we can
        // remove any MDVDB_* vars that .env introduces (those should come from
        // YAML config, not .env). Only shell-set MDVDB_* vars should persist
        // as overrides.
        let pre_env_mdvdb_vars: std::collections::HashSet<String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("MDVDB_"))
            .map(|(k, _)| k)
            .collect();

        let _ = dotenvy::from_path(project_root.join(".env"));

        // Remove MDVDB_* vars introduced by .env (not present before).
        // These should be configured via YAML, not .env.
        for (k, _) in std::env::vars() {
            if k.starts_with("MDVDB_") && !pre_env_mdvdb_vars.contains(&k) {
                std::env::remove_var(&k);
            }
        }

        // (2) Detect and auto-migrate project config.
        let mdvdb_dir = project_root.join(".markdownvdb");
        let project_yaml_path = mdvdb_dir.join("config.yaml");
        let project_dotenv_path = mdvdb_dir.join(".config");
        let legacy_flat = project_root.join(".markdownvdb");

        if !project_yaml_path.is_file() {
            if project_dotenv_path.is_file() {
                // .markdownvdb/.config exists but no config.yaml — migrate
                let _ = migrate_dotenv_to_yaml(&project_dotenv_path, &project_yaml_path);
            } else if legacy_flat.is_file() {
                // Legacy flat .markdownvdb file — migrate to dir structure
                let tmp_path = project_root.join(".markdownvdb.tmp");
                if fs::rename(&legacy_flat, &tmp_path).is_ok() {
                    if fs::create_dir_all(&mdvdb_dir).is_ok() {
                        let new_dotenv = mdvdb_dir.join(".config");
                        if fs::rename(&tmp_path, &new_dotenv).is_ok() {
                            let _ = migrate_dotenv_to_yaml(&new_dotenv, &project_yaml_path);
                        }
                    } else {
                        // Restore if dir creation failed
                        let _ = fs::rename(&tmp_path, &legacy_flat);
                    }
                }
            }
        }

        // (3) Detect and auto-migrate user config.
        if std::env::var("MDVDB_NO_USER_CONFIG").is_err() {
            if let Some(user_dir) = Self::user_config_dir() {
                let user_yaml = user_dir.join("config.yaml");
                let user_dotenv = user_dir.join("config");
                if !user_yaml.is_file() && user_dotenv.is_file() {
                    let _ = migrate_dotenv_to_yaml(&user_dotenv, &user_yaml);
                }
            }
        }

        // (4) Load user YAML as Value (or empty Mapping).
        let user_value: serde_yaml::Value = if std::env::var("MDVDB_NO_USER_CONFIG").is_err() {
            Self::user_config_path()
                .filter(|p| p.is_file())
                .and_then(|p| fs::read_to_string(&p).ok())
                .and_then(|s| serde_yaml::from_str(&s).ok())
                .unwrap_or_else(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
        } else {
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
        };

        // (5) Load project YAML as Value (or empty Mapping).
        let project_value: serde_yaml::Value = if project_yaml_path.is_file() {
            let content = fs::read_to_string(&project_yaml_path).map_err(|e| {
                Error::Config(format!(
                    "failed to read project config '{}': {e}",
                    project_yaml_path.display()
                ))
            })?;
            serde_yaml::from_str(&content).map_err(|e| {
                Error::Config(format!(
                    "failed to parse project config '{}': {e}",
                    project_yaml_path.display()
                ))
            })?
        } else {
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
        };

        // (6) Deep merge project over user.
        let merged = merge_yaml_values(user_value, project_value);

        // (7) Deserialize merged Value into YamlConfig.
        let mut yaml_config: YamlConfig = serde_yaml::from_value(merged).map_err(|e| {
            Error::Config(format!("failed to deserialize merged config: {e}"))
        })?;

        // (8) Apply env var overrides.
        apply_env_overrides(&mut yaml_config);

        // (9) Convert via Config::from_yaml() (validates inside).
        Self::from_yaml(yaml_config, project_root)
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
#[allow(dead_code)]
fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Parse an env var into a typed value, using a default if not set.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
fn parse_comma_list_path(key: &str, default: Vec<PathBuf>) -> Vec<PathBuf> {
    match std::env::var(key) {
        Ok(val) if !val.trim().is_empty() => {
            val.split(',').map(|s| PathBuf::from(s.trim())).collect()
        }
        _ => default,
    }
}

/// Parse a comma-separated env var into Vec<String>, trimming whitespace.
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Migrate a dotenv-style config file to YAML format.
///
/// Reads the dotenv file line by line, maps `MDVDB_*` keys to `YamlConfig` fields,
/// writes the YAML file atomically, and renames the old file to `.bak`.
pub fn migrate_dotenv_to_yaml(dotenv_path: &Path, yaml_path: &Path) -> Result<(), Error> {
    use std::fs;

    let content = fs::read_to_string(dotenv_path).map_err(|e| {
        Error::Config(format!("failed to read dotenv file '{}': {e}", dotenv_path.display()))
    })?;

    let mut yaml = YamlConfig::default();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = raw_value.trim().trim_matches('"').trim_matches('\'');

        match key {
            "MDVDB_EMBEDDING_PROVIDER" => yaml.embedding.provider = value.to_string(),
            "MDVDB_EMBEDDING_MODEL" => yaml.embedding.model = value.to_string(),
            "MDVDB_EMBEDDING_DIMENSIONS" => {
                if let Ok(v) = value.parse() { yaml.embedding.dimensions = v; }
            }
            "MDVDB_EMBEDDING_BATCH_SIZE" => {
                if let Ok(v) = value.parse() { yaml.embedding.batch_size = v; }
            }
            "MDVDB_EMBEDDING_ENDPOINT" => yaml.embedding.endpoint = Some(value.to_string()),
            "MDVDB_SEARCH_DEFAULT_LIMIT" => {
                if let Ok(v) = value.parse() { yaml.search.limit = v; }
            }
            "MDVDB_SEARCH_MIN_SCORE" => {
                if let Ok(v) = value.parse() { yaml.search.min_score = v; }
            }
            "MDVDB_SEARCH_MODE" => yaml.search.mode = value.to_string(),
            "MDVDB_SEARCH_RRF_K" => {
                if let Ok(v) = value.parse() { yaml.search.rrf_k = v; }
            }
            "MDVDB_BM25_NORM_K" => {
                if let Ok(v) = value.parse() { yaml.search.bm25_norm_k = v; }
            }
            "MDVDB_SEARCH_BOOST_LINKS" => {
                if let Ok(v) = value.parse::<bool>() { yaml.search.boost_links = v; }
                else { yaml.search.boost_links = value == "1" || value == "yes"; }
            }
            "MDVDB_SEARCH_BOOST_HOPS" => {
                if let Ok(v) = value.parse() { yaml.search.boost_hops = v; }
            }
            "MDVDB_SEARCH_EXPAND_GRAPH" => {
                if let Ok(v) = value.parse() { yaml.search.expand_graph = v; }
            }
            "MDVDB_SEARCH_EXPAND_LIMIT" => {
                if let Ok(v) = value.parse() { yaml.search.expand_limit = v; }
            }
            "MDVDB_SEARCH_DECAY" => {
                if let Ok(v) = value.parse::<bool>() { yaml.search.decay.enabled = v; }
                else { yaml.search.decay.enabled = value == "1" || value == "yes"; }
            }
            "MDVDB_SEARCH_DECAY_HALF_LIFE" => {
                if let Ok(v) = value.parse() { yaml.search.decay.half_life = v; }
            }
            "MDVDB_SEARCH_DECAY_EXCLUDE" => {
                yaml.search.decay.exclude = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
            "MDVDB_SEARCH_DECAY_INCLUDE" => {
                yaml.search.decay.include = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
            "MDVDB_CHUNK_MAX_TOKENS" => {
                if let Ok(v) = value.parse() { yaml.chunking.max_tokens = v; }
            }
            "MDVDB_CHUNK_OVERLAP_TOKENS" => {
                if let Ok(v) = value.parse() { yaml.chunking.overlap_tokens = v; }
            }
            "MDVDB_CLUSTERING_ENABLED" => {
                if let Ok(v) = value.parse::<bool>() { yaml.clustering.enabled = v; }
                else { yaml.clustering.enabled = value == "1" || value == "yes"; }
            }
            "MDVDB_CLUSTERING_REBALANCE_THRESHOLD" => {
                if let Ok(v) = value.parse() { yaml.clustering.rebalance_threshold = v; }
            }
            "MDVDB_CLUSTER_GRANULARITY" => {
                if let Ok(v) = value.parse() { yaml.clustering.granularity = v; }
            }
            "MDVDB_CUSTOM_CLUSTERS" => {
                yaml.clustering.custom = parse_custom_clusters_value(value)
                    .into_iter()
                    .map(|d| YamlCustomCluster { name: d.name, seeds: d.seeds })
                    .collect();
            }
            "MDVDB_WATCH" => {
                if let Ok(v) = value.parse::<bool>() { yaml.watch.enabled = v; }
                else { yaml.watch.enabled = value == "1" || value == "yes"; }
            }
            "MDVDB_WATCH_DEBOUNCE_MS" => {
                if let Ok(v) = value.parse() { yaml.watch.debounce_ms = v; }
            }
            "MDVDB_VECTOR_QUANTIZATION" => yaml.index.quantization = value.to_string(),
            "MDVDB_INDEX_COMPRESSION" => {
                if let Ok(v) = value.parse::<bool>() { yaml.index.compression = v; }
                else { yaml.index.compression = value == "1" || value == "yes"; }
            }
            "MDVDB_EDGE_EMBEDDINGS" => {
                if let Ok(v) = value.parse::<bool>() { yaml.index.edge_embeddings = v; }
                else { yaml.index.edge_embeddings = value == "1" || value == "yes"; }
            }
            "MDVDB_EDGE_BOOST_WEIGHT" => {
                if let Ok(v) = value.parse() { yaml.index.edge_boost_weight = v; }
            }
            "MDVDB_EDGE_CLUSTER_REBALANCE" => {
                if let Ok(v) = value.parse() { yaml.index.edge_cluster_rebalance = v; }
            }
            "MDVDB_SOURCE_DIRS" => {
                yaml.sources.dirs = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
            "MDVDB_IGNORE_PATTERNS" => {
                yaml.sources.ignore = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
            _ => {} // Ignore non-MDVDB keys
        }
    }

    write_yaml_config(yaml_path, &yaml)?;

    // Rename old dotenv file to .bak
    let bak_path = dotenv_path.with_extension("bak");
    if dotenv_path.file_name().is_some_and(|n| n.to_str().is_some_and(|s| s.starts_with('.'))) {
        // For dotfiles like .config -> .config.bak
        let mut bak_name = dotenv_path.file_name().unwrap().to_os_string();
        bak_name.push(".bak");
        let bak_path = dotenv_path.with_file_name(bak_name);
        fs::rename(dotenv_path, &bak_path).map_err(|e| {
            Error::Config(format!("failed to rename dotenv to backup: {e}"))
        })?;
    } else {
        fs::rename(dotenv_path, &bak_path).map_err(|e| {
            Error::Config(format!("failed to rename dotenv to backup: {e}"))
        })?;
    }

    tracing::info!(
        "Migrated dotenv config to YAML: {}",
        yaml_path.display()
    );

    Ok(())
}

/// Write a `YamlConfig` to a file atomically (write .tmp, fsync, rename).
///
/// Creates parent directories if needed.
pub fn write_yaml_config(path: &Path, config: &YamlConfig) -> Result<(), Error> {
    use std::fs;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::Config(format!(
                "failed to create directory '{}': {e}",
                parent.display()
            ))
        })?;
    }

    let yaml_str = serde_yaml::to_string(config).map_err(|e| {
        Error::Config(format!("failed to serialize YAML config: {e}"))
    })?;

    let tmp_path = path.with_extension("yaml.tmp");
    let mut file = fs::File::create(&tmp_path).map_err(|e| {
        Error::Config(format!("failed to create temp file '{}': {e}", tmp_path.display()))
    })?;
    file.write_all(yaml_str.as_bytes()).map_err(|e| {
        Error::Config(format!("failed to write YAML config: {e}"))
    })?;
    file.sync_all().map_err(|e| {
        Error::Config(format!("failed to fsync YAML config: {e}"))
    })?;
    drop(file);

    fs::rename(&tmp_path, path).map_err(|e| {
        Error::Config(format!(
            "failed to rename temp file to '{}': {e}",
            path.display()
        ))
    })?;

    Ok(())
}

/// Update a single value in a YAML config file using dot-notation key path.
///
/// For example, `update_yaml_config_value(path, "search.decay.half_life", Value::Number(45.into()))`
/// navigates to `search.decay.half_life` and sets it. Creates the file if it doesn't exist.
/// Writes atomically.
pub fn update_yaml_config_value(
    path: &Path,
    key_path: &str,
    value: serde_yaml::Value,
) -> Result<(), Error> {
    use std::fs;

    let mut root: serde_yaml::Value = if path.exists() {
        let content = fs::read_to_string(path).map_err(|e| {
            Error::Config(format!("failed to read YAML file '{}': {e}", path.display()))
        })?;
        serde_yaml::from_str(&content).map_err(|e| {
            Error::Config(format!("failed to parse YAML file '{}': {e}", path.display()))
        })?
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    let parts: Vec<&str> = key_path.split('.').collect();
    let mut current = &mut root;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Set the value
            if let serde_yaml::Value::Mapping(map) = current {
                map.insert(serde_yaml::Value::String(part.to_string()), value.clone());
            } else {
                return Err(Error::Config(format!(
                    "cannot set key '{}': parent is not a mapping",
                    key_path
                )));
            }
        } else {
            // Navigate or create intermediate mapping
            let key = serde_yaml::Value::String(part.to_string());
            if let serde_yaml::Value::Mapping(map) = current {
                if !map.contains_key(&key) {
                    map.insert(key.clone(), serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
                }
                current = map.get_mut(&key).unwrap();
            } else {
                return Err(Error::Config(format!(
                    "cannot navigate key '{}': intermediate is not a mapping",
                    key_path
                )));
            }
        }
    }

    // Serialize and write atomically
    let yaml_str = serde_yaml::to_string(&root).map_err(|e| {
        Error::Config(format!("failed to serialize YAML: {e}"))
    })?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            Error::Config(format!("failed to create directory '{}': {e}", parent.display()))
        })?;
    }

    let tmp_path = path.with_extension("yaml.tmp");
    {
        use std::io::Write;
        let mut file = fs::File::create(&tmp_path).map_err(|e| {
            Error::Config(format!("failed to create temp file: {e}"))
        })?;
        file.write_all(yaml_str.as_bytes()).map_err(|e| {
            Error::Config(format!("failed to write YAML: {e}"))
        })?;
        file.sync_all().map_err(|e| {
            Error::Config(format!("failed to fsync YAML: {e}"))
        })?;
    }

    fs::rename(&tmp_path, path).map_err(|e| {
        Error::Config(format!("failed to rename temp file to '{}': {e}", path.display()))
    })?;

    Ok(())
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
        // With the YAML pipeline, env var overrides silently ignore unparseable
        // values (using .ok()), so a non-numeric MDVDB_EMBEDDING_DIMENSIONS
        // falls back to the YAML/default value instead of erroring.
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var("MDVDB_EMBEDDING_DIMENSIONS").ok();
        let had_no_user = std::env::var("MDVDB_NO_USER_CONFIG").ok();
        std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "abc");
        std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
        let result = Config::load(Path::new("/nonexistent"));
        match saved { Some(v) => std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", v), None => std::env::remove_var("MDVDB_EMBEDDING_DIMENSIONS") }
        match had_no_user { Some(v) => std::env::set_var("MDVDB_NO_USER_CONFIG", v), None => std::env::remove_var("MDVDB_NO_USER_CONFIG") }
        // Should succeed with default dimensions since "abc" is silently skipped
        let config = result.unwrap();
        assert_eq!(config.embedding_dimensions, 1536);
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
        assert_eq!(path, Some(PathBuf::from("/custom/config.yaml")));
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

        // Create a temp user config that sets a specific model (YAML format).
        let temp = tempfile::TempDir::new().unwrap();
        let config_path = temp.path().join("config.yaml");
        std::fs::write(&config_path, "embedding:\n  model: custom-model\n").unwrap();

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

    // --- migrate / write / update YAML tests ---

    #[test]
    fn migrate_dotenv_to_yaml_basic() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dotenv = tmp.path().join(".config");
        let yaml_path = tmp.path().join("config.yaml");

        std::fs::write(&dotenv, "\
MDVDB_EMBEDDING_PROVIDER=ollama\n\
MDVDB_EMBEDDING_DIMENSIONS=768\n\
MDVDB_SEARCH_DEFAULT_LIMIT=25\n\
MDVDB_CHUNK_MAX_TOKENS=1024\n\
MDVDB_CLUSTERING_ENABLED=false\n\
MDVDB_WATCH_DEBOUNCE_MS=500\n\
").unwrap();

        migrate_dotenv_to_yaml(&dotenv, &yaml_path).unwrap();

        let content = std::fs::read_to_string(&yaml_path).unwrap();
        let cfg: YamlConfig = serde_yaml::from_str(&content).unwrap();
        assert_eq!(cfg.embedding.provider, "ollama");
        assert_eq!(cfg.embedding.dimensions, 768);
        assert_eq!(cfg.search.limit, 25);
        assert_eq!(cfg.chunking.max_tokens, 1024);
        assert!(!cfg.clustering.enabled);
        assert_eq!(cfg.watch.debounce_ms, 500);
    }

    #[test]
    fn migrate_dotenv_backup() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dotenv = tmp.path().join(".config");
        let yaml_path = tmp.path().join("config.yaml");

        std::fs::write(&dotenv, "MDVDB_EMBEDDING_PROVIDER=openai\n").unwrap();
        migrate_dotenv_to_yaml(&dotenv, &yaml_path).unwrap();

        assert!(!dotenv.exists());
        assert!(tmp.path().join(".config.bak").exists());
    }

    #[test]
    fn migrate_dotenv_with_comments() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dotenv = tmp.path().join(".config");
        let yaml_path = tmp.path().join("config.yaml");

        std::fs::write(&dotenv, "\
# This is a comment\n\
\n\
MDVDB_EMBEDDING_PROVIDER=ollama\n\
# Another comment\n\
MDVDB_SEARCH_DEFAULT_LIMIT=5\n\
\n\
").unwrap();

        migrate_dotenv_to_yaml(&dotenv, &yaml_path).unwrap();

        let content = std::fs::read_to_string(&yaml_path).unwrap();
        let cfg: YamlConfig = serde_yaml::from_str(&content).unwrap();
        assert_eq!(cfg.embedding.provider, "ollama");
        assert_eq!(cfg.search.limit, 5);
    }

    #[test]
    fn write_yaml_config_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sub").join("config.yaml");

        let mut cfg = YamlConfig::default();
        cfg.embedding.provider = "ollama".to_string();
        cfg.search.limit = 42;
        cfg.chunking.max_tokens = 2048;

        write_yaml_config(&path, &cfg).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: YamlConfig = serde_yaml::from_str(&content).unwrap();
        assert_eq!(loaded.embedding.provider, "ollama");
        assert_eq!(loaded.search.limit, 42);
        assert_eq!(loaded.chunking.max_tokens, 2048);
    }

    #[test]
    fn update_yaml_config_value_nested() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.yaml");

        // Write initial config
        let cfg = YamlConfig::default();
        write_yaml_config(&path, &cfg).unwrap();

        // Update nested value
        update_yaml_config_value(&path, "search.decay.half_life", serde_yaml::Value::Number(serde_yaml::Number::from(45))).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: YamlConfig = serde_yaml::from_str(&content).unwrap();
        assert_eq!(loaded.search.decay.half_life, 45.0);

        // Update top-level nested value
        update_yaml_config_value(&path, "embedding.provider", serde_yaml::Value::String("ollama".into())).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: YamlConfig = serde_yaml::from_str(&content).unwrap();
        assert_eq!(loaded.embedding.provider, "ollama");
        // Verify previous update is preserved
        assert_eq!(loaded.search.decay.half_life, 45.0);
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
