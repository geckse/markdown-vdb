use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;

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
    pub index_file: PathBuf,
    pub ignore_patterns: Vec<String>,
    pub watch_enabled: bool,
    pub watch_debounce_ms: u64,
    pub chunk_max_tokens: usize,
    pub chunk_overlap_tokens: usize,
    pub clustering_enabled: bool,
    pub clustering_rebalance_threshold: usize,
    pub search_default_limit: usize,
    pub search_min_score: f64,
    pub fts_index_dir: PathBuf,
    pub search_default_mode: SearchMode,
    pub search_rrf_k: f64,
}

impl Config {
    /// Load configuration with priority: shell env > `.markdownvdb` file > `.env` file > built-in defaults.
    pub fn load(project_root: &Path) -> Result<Self, Error> {
        // Load .markdownvdb file first (ignore if missing).
        // dotenvy::from_path does NOT override existing env vars,
        // so shell env always takes priority.
        let _ = dotenvy::from_path(project_root.join(".markdownvdb"));

        // Load .env as a fallback for shared secrets (e.g., OPENAI_API_KEY).
        // Since .markdownvdb was loaded first, its values take priority over .env.
        let _ = dotenvy::from_path(project_root.join(".env"));

        let embedding_provider = env_or_default("MDVDB_EMBEDDING_PROVIDER", "openai")
            .parse::<EmbeddingProviderType>()?;

        let embedding_model = env_or_default("MDVDB_EMBEDDING_MODEL", "text-embedding-3-small");

        let embedding_dimensions = parse_env::<usize>("MDVDB_EMBEDDING_DIMENSIONS", 1536)?;

        let embedding_batch_size = parse_env::<usize>("MDVDB_EMBEDDING_BATCH_SIZE", 100)?;

        let openai_api_key = std::env::var("OPENAI_API_KEY").ok();

        let ollama_host = env_or_default("OLLAMA_HOST", "http://localhost:11434");

        let embedding_endpoint = std::env::var("MDVDB_EMBEDDING_ENDPOINT").ok();

        let source_dirs = parse_comma_list_path("MDVDB_SOURCE_DIRS", vec![PathBuf::from(".")]);

        let index_file = PathBuf::from(env_or_default("MDVDB_INDEX_FILE", ".markdownvdb.index"));

        let ignore_patterns = parse_comma_list_string("MDVDB_IGNORE_PATTERNS", vec![]);

        let watch_enabled = parse_env_bool("MDVDB_WATCH", true)?;

        let watch_debounce_ms = parse_env::<u64>("MDVDB_WATCH_DEBOUNCE_MS", 300)?;

        let chunk_max_tokens = parse_env::<usize>("MDVDB_CHUNK_MAX_TOKENS", 512)?;

        let chunk_overlap_tokens = parse_env::<usize>("MDVDB_CHUNK_OVERLAP_TOKENS", 50)?;

        let clustering_enabled = parse_env_bool("MDVDB_CLUSTERING_ENABLED", true)?;

        let clustering_rebalance_threshold =
            parse_env::<usize>("MDVDB_CLUSTERING_REBALANCE_THRESHOLD", 50)?;

        let search_default_limit = parse_env::<usize>("MDVDB_SEARCH_DEFAULT_LIMIT", 10)?;

        let search_min_score = parse_env::<f64>("MDVDB_SEARCH_MIN_SCORE", 0.0)?;

        let fts_index_dir =
            PathBuf::from(env_or_default("MDVDB_FTS_INDEX_DIR", ".markdownvdb.fts"));

        let search_default_mode = env_or_default("MDVDB_SEARCH_MODE", "hybrid")
            .parse::<SearchMode>()?;

        let search_rrf_k = parse_env::<f64>("MDVDB_SEARCH_RRF_K", 60.0)?;

        let config = Self {
            embedding_provider,
            embedding_model,
            embedding_dimensions,
            embedding_batch_size,
            openai_api_key,
            ollama_host,
            embedding_endpoint,
            source_dirs,
            index_file,
            ignore_patterns,
            watch_enabled,
            watch_debounce_ms,
            chunk_max_tokens,
            chunk_overlap_tokens,
            clustering_enabled,
            clustering_rebalance_threshold,
            search_default_limit,
            search_min_score,
            fts_index_dir,
            search_default_mode,
            search_rrf_k,
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
        if !(0.0..=1.0).contains(&self.search_min_score) {
            return Err(Error::Config(format!(
                "search_min_score ({}) must be in [0.0, 1.0]",
                self.search_min_score
            )));
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
        let _lock = ENV_MUTEX.lock().unwrap();
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
            "MDVDB_INDEX_FILE",
            "MDVDB_IGNORE_PATTERNS",
            "MDVDB_WATCH",
            "MDVDB_WATCH_DEBOUNCE_MS",
            "MDVDB_CHUNK_MAX_TOKENS",
            "MDVDB_CHUNK_OVERLAP_TOKENS",
            "MDVDB_CLUSTERING_ENABLED",
            "MDVDB_CLUSTERING_REBALANCE_THRESHOLD",
            "MDVDB_SEARCH_DEFAULT_LIMIT",
            "MDVDB_SEARCH_MIN_SCORE",
            "MDVDB_FTS_INDEX_DIR",
            "MDVDB_SEARCH_MODE",
            "MDVDB_SEARCH_RRF_K",
        ];
        for var in &vars_to_clear {
            std::env::remove_var(var);
        }

        // Load from a non-existent dir to get pure defaults
        let config = Config::load(Path::new("/nonexistent")).unwrap();

        assert_eq!(config.embedding_provider, EmbeddingProviderType::OpenAI);
        assert_eq!(config.embedding_model, "text-embedding-3-small");
        assert_eq!(config.embedding_dimensions, 1536);
        assert_eq!(config.embedding_batch_size, 100);
        assert_eq!(config.openai_api_key, None);
        assert_eq!(config.ollama_host, "http://localhost:11434");
        assert_eq!(config.embedding_endpoint, None);
        assert_eq!(config.source_dirs, vec![PathBuf::from(".")]);
        assert_eq!(config.index_file, PathBuf::from(".markdownvdb.index"));
        assert!(config.ignore_patterns.is_empty());
        assert!(config.watch_enabled);
        assert_eq!(config.watch_debounce_ms, 300);
        assert_eq!(config.chunk_max_tokens, 512);
        assert_eq!(config.chunk_overlap_tokens, 50);
        assert!(config.clustering_enabled);
        assert_eq!(config.clustering_rebalance_threshold, 50);
        assert_eq!(config.search_default_limit, 10);
        assert_eq!(config.search_min_score, 0.0);
        assert_eq!(config.fts_index_dir, PathBuf::from(".markdownvdb.fts"));
        assert_eq!(config.search_default_mode, SearchMode::Hybrid);
        assert_eq!(config.search_rrf_k, 60.0);
    }

    #[test]
    fn validation_rejects_zero_dimensions() {
        let _lock = ENV_MUTEX.lock().unwrap();
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
        let _lock = ENV_MUTEX.lock().unwrap();
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
        let _lock = ENV_MUTEX.lock().unwrap();
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
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("MDVDB_SEARCH_MIN_SCORE", "1.5");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_MIN_SCORE");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("search_min_score"));
    }

    #[test]
    fn validation_rejects_negative_score() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("MDVDB_SEARCH_MIN_SCORE", "-0.1");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_MIN_SCORE");
        assert!(result.is_err());
    }

    #[test]
    fn validation_rejects_zero_rrf_k() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("MDVDB_SEARCH_RRF_K", "0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_RRF_K");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("search_rrf_k"));
    }

    #[test]
    fn validation_rejects_negative_rrf_k() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("MDVDB_SEARCH_RRF_K", "-10.0");
        let result = Config::load(Path::new("/nonexistent"));
        std::env::remove_var("MDVDB_SEARCH_RRF_K");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("search_rrf_k"));
    }

    #[test]
    fn parse_error_on_non_numeric() {
        let _lock = ENV_MUTEX.lock().unwrap();
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
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("MDVDB_SOURCE_DIRS", " docs , notes ");
        let dirs = parse_comma_list_path("MDVDB_SOURCE_DIRS", vec![]);
        std::env::remove_var("MDVDB_SOURCE_DIRS");
        assert_eq!(dirs, vec![PathBuf::from("docs"), PathBuf::from("notes")]);
    }

    #[test]
    fn comma_separated_ignore_patterns() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("MDVDB_IGNORE_PATTERNS", " *.tmp , .git ");
        let patterns = parse_comma_list_string("MDVDB_IGNORE_PATTERNS", vec![]);
        std::env::remove_var("MDVDB_IGNORE_PATTERNS");
        assert_eq!(patterns, vec!["*.tmp".to_string(), ".git".to_string()]);
    }
}
