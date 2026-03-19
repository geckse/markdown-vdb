use std::fs;
use std::path::PathBuf;

use mdvdb::config::{Config, EmbeddingProviderType, VectorQuantization};
use mdvdb::Error;
use serial_test::serial;
use tempfile::TempDir;

/// All MDVDB env vars that could affect config loading.
const ALL_ENV_VARS: &[&str] = &[
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
    "MDVDB_CONFIG_HOME",
    "MDVDB_NO_USER_CONFIG",
    "MDVDB_VECTOR_QUANTIZATION",
    "MDVDB_INDEX_COMPRESSION",
    "MDVDB_SEARCH_BOOST_LINKS",
    "MDVDB_SEARCH_BOOST_HOPS",
    "MDVDB_SEARCH_EXPAND_GRAPH",
    "MDVDB_SEARCH_EXPAND_LIMIT",
];

/// Clear all MDVDB-related env vars to ensure test isolation.
fn clear_env() {
    for var in ALL_ENV_VARS {
        std::env::remove_var(var);
    }
}

#[test]
#[serial]
fn defaults_applied_when_no_config() {
    clear_env();
    // Disable user config file (~/.mdvdb/config) so it doesn't inject values
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    let config = Config::load(tmp.path()).unwrap();

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
    assert!((config.clustering_granularity - 1.0).abs() < f64::EPSILON, "default granularity should be 1.0");
    assert_eq!(config.search_default_limit, 10);
    assert_eq!(config.search_min_score, 0.0);
    assert_eq!(config.search_default_mode, mdvdb::SearchMode::Hybrid);
    assert_eq!(config.search_rrf_k, 60.0);
    assert_eq!(config.bm25_norm_k, 1.5);
    assert!(!config.search_decay_enabled, "decay should be disabled by default");
    assert_eq!(config.search_decay_half_life, 90.0, "default half-life should be 90 days");
    assert_eq!(config.vector_quantization, VectorQuantization::F16, "default quantization should be F16");
    assert!(config.index_compression, "index compression should be enabled by default");
    assert_eq!(config.search_boost_hops, 1, "default boost hops should be 1");
    assert_eq!(config.search_expand_graph, 0, "default expand graph should be 0 (disabled)");
    assert_eq!(config.search_expand_limit, 3, "default expand limit should be 3");
}

#[test]
#[serial]
fn dotenv_file_overrides_defaults() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let dotenv_path = tmp.path().join(".markdownvdb");
    fs::write(
        &dotenv_path,
        "MDVDB_EMBEDDING_MODEL=custom-model\n\
         MDVDB_EMBEDDING_DIMENSIONS=768\n\
         MDVDB_SEARCH_DEFAULT_LIMIT=20\n\
         MDVDB_WATCH=false\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_model, "custom-model");
    assert_eq!(config.embedding_dimensions, 768);
    assert_eq!(config.search_default_limit, 20);
    assert!(!config.watch_enabled);
}

#[test]
#[serial]
fn env_vars_override_file() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let dotenv_path = tmp.path().join(".markdownvdb");
    fs::write(
        &dotenv_path,
        "MDVDB_EMBEDDING_MODEL=file-model\n\
         MDVDB_EMBEDDING_DIMENSIONS=768\n",
    )
    .unwrap();

    // Shell env should win over file
    std::env::set_var("MDVDB_EMBEDDING_MODEL", "env-model");
    std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "256");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_model, "env-model");
    assert_eq!(config.embedding_dimensions, 256);

    clear_env();
}

#[test]
#[serial]
fn comma_separated_source_dirs() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SOURCE_DIRS", "docs,notes");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.source_dirs,
        vec![PathBuf::from("docs"), PathBuf::from("notes")]
    );

    clear_env();
}

#[test]
#[serial]
fn whitespace_trimmed_in_lists() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SOURCE_DIRS", "docs , notes ");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.source_dirs,
        vec![PathBuf::from("docs"), PathBuf::from("notes")]
    );

    clear_env();
}

#[test]
#[serial]
fn case_insensitive_provider() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    for variant in &["OpenAI", "OPENAI", "openai"] {
        std::env::set_var("MDVDB_EMBEDDING_PROVIDER", variant);
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(
            config.embedding_provider,
            EmbeddingProviderType::OpenAI,
            "Failed for variant: {variant}"
        );
    }

    clear_env();
}

#[test]
#[serial]
fn invalid_dimensions_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "0");

    let result = Config::load(tmp.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Config(msg) => assert!(msg.contains("embedding_dimensions")),
        other => panic!("expected Error::Config, got: {other}"),
    }

    clear_env();
}

#[test]
#[serial]
fn invalid_dimensions_non_numeric() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "abc");

    let result = Config::load(tmp.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Config(msg) => assert!(msg.contains("MDVDB_EMBEDDING_DIMENSIONS")),
        other => panic!("expected Error::Config, got: {other}"),
    }

    clear_env();
}

#[test]
#[serial]
fn unknown_provider_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_EMBEDDING_PROVIDER", "unknown");

    let result = Config::load(tmp.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Config(msg) => assert!(msg.contains("unknown")),
        other => panic!("expected Error::Config, got: {other}"),
    }

    clear_env();
}

#[test]
#[serial]
fn overlap_exceeds_max_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_CHUNK_MAX_TOKENS", "10");
    std::env::set_var("MDVDB_CHUNK_OVERLAP_TOKENS", "20");

    let result = Config::load(tmp.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Config(msg) => assert!(msg.contains("chunk_overlap_tokens")),
        other => panic!("expected Error::Config, got: {other}"),
    }

    clear_env();
}

#[test]
#[serial]
fn score_out_of_range() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_MIN_SCORE", "1.5");

    let result = Config::load(tmp.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        Error::Config(msg) => assert!(msg.contains("search_min_score")),
        other => panic!("expected Error::Config, got: {other}"),
    }

    clear_env();
}

#[test]
#[serial]
fn missing_dotenv_file_ok() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    // No .markdownvdb file in tmp dir — should not error
    let result = Config::load(tmp.path());
    assert!(result.is_ok());
    clear_env();
}

#[test]
#[serial]
fn env_file_provides_fallback_values() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    // .markdownvdb has mdvdb-specific settings but no API key
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_PROVIDER=openai\nMDVDB_EMBEDDING_DIMENSIONS=768\n",
    )
    .unwrap();

    // .env has the shared secret
    fs::write(
        tmp.path().join(".env"),
        "OPENAI_API_KEY=sk-test-from-dotenv\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.openai_api_key, Some("sk-test-from-dotenv".into()));
    assert_eq!(config.embedding_dimensions, 768);

    clear_env();
}

#[test]
#[serial]
fn markdownvdb_overrides_env_file() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    // .env has a dimension setting
    fs::write(
        tmp.path().join(".env"),
        "MDVDB_EMBEDDING_DIMENSIONS=256\n",
    )
    .unwrap();

    // .markdownvdb overrides it
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_DIMENSIONS=768\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.embedding_dimensions, 768,
        ".markdownvdb should take priority over .env"
    );

    clear_env();
}

#[test]
#[serial]
fn shell_env_overrides_both_files() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    fs::write(
        tmp.path().join(".env"),
        "MDVDB_EMBEDDING_DIMENSIONS=256\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_DIMENSIONS=768\n",
    )
    .unwrap();

    // Shell env overrides everything
    std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "1024");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.embedding_dimensions, 1024,
        "shell env should take priority over both files"
    );

    clear_env();
}

#[test]
#[serial]
fn search_config_from_dotenv() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_SEARCH_MODE=semantic\n\
         MDVDB_SEARCH_RRF_K=30.0\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.search_default_mode, mdvdb::SearchMode::Semantic);
    assert_eq!(config.search_rrf_k, 30.0);

    clear_env();
}

#[test]
#[serial]
fn search_mode_case_insensitive() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    for (variant, expected) in [
        ("hybrid", mdvdb::SearchMode::Hybrid),
        ("SEMANTIC", mdvdb::SearchMode::Semantic),
        ("Lexical", mdvdb::SearchMode::Lexical),
    ] {
        std::env::set_var("MDVDB_SEARCH_MODE", variant);
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(
            config.search_default_mode, expected,
            "Failed for variant: {variant}"
        );
    }

    clear_env();
}

#[test]
#[serial]
fn invalid_search_mode_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_MODE", "invalid");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "invalid search mode should be rejected");

    clear_env();
}

#[test]
#[serial]
fn invalid_rrf_k_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_RRF_K", "not_a_number");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "non-numeric rrf_k should be rejected");

    clear_env();
}

// ---------------------------------------------------------------------------
// User-level config (~/.mdvdb/config) tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn user_config_provides_fallback_values() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    // Write a user config that sets the model.
    fs::write(
        user_home.path().join("config"),
        "MDVDB_EMBEDDING_MODEL=user-model\nMDVDB_EMBEDDING_DIMENSIONS=256\n",
    )
    .unwrap();

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "user-model",
        "user config should provide fallback model"
    );
    assert_eq!(
        config.embedding_dimensions, 256,
        "user config should provide fallback dimensions"
    );

    clear_env();
}

#[test]
#[serial]
fn project_config_overrides_user_config() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    // User config sets model.
    fs::write(
        user_home.path().join("config"),
        "MDVDB_EMBEDDING_MODEL=user-model\nMDVDB_EMBEDDING_DIMENSIONS=256\n",
    )
    .unwrap();

    // Project config overrides model.
    fs::write(
        project.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_MODEL=project-model\n",
    )
    .unwrap();

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "project-model",
        "project config should override user config"
    );
    // Dimensions not set in project config, so user config provides fallback.
    assert_eq!(
        config.embedding_dimensions, 256,
        "user config should provide fallback for keys not in project config"
    );

    clear_env();
}

#[test]
#[serial]
fn shell_env_overrides_user_config() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    fs::write(
        user_home.path().join("config"),
        "MDVDB_EMBEDDING_MODEL=user-model\n",
    )
    .unwrap();

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());
    std::env::set_var("MDVDB_EMBEDDING_MODEL", "env-model");

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "env-model",
        "shell env should override user config"
    );

    clear_env();
}

#[test]
#[serial]
fn dotenv_overrides_user_config() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    fs::write(
        user_home.path().join("config"),
        "MDVDB_EMBEDDING_MODEL=user-model\n",
    )
    .unwrap();

    fs::write(
        project.path().join(".env"),
        "MDVDB_EMBEDDING_MODEL=dotenv-model\n",
    )
    .unwrap();

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "dotenv-model",
        ".env should override user config"
    );

    clear_env();
}

#[test]
#[serial]
fn missing_user_config_dir_silently_skipped() {
    clear_env();
    let project = TempDir::new().unwrap();

    // Point to a non-existent directory.
    std::env::set_var("MDVDB_CONFIG_HOME", "/nonexistent/mdvdb/config/dir");

    let result = Config::load(project.path());
    assert!(result.is_ok(), "missing user config dir should not cause errors");

    clear_env();
}

#[test]
#[serial]
fn no_user_config_env_disables_user_config() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    fs::write(
        user_home.path().join("config"),
        "MDVDB_EMBEDDING_MODEL=user-model\n",
    )
    .unwrap();

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "text-embedding-3-small",
        "MDVDB_NO_USER_CONFIG should prevent loading user config"
    );

    clear_env();
}

#[test]
#[serial]
fn full_four_level_cascade() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    // User config: sets model and dimensions.
    fs::write(
        user_home.path().join("config"),
        "MDVDB_EMBEDDING_MODEL=user-model\n\
         MDVDB_EMBEDDING_DIMENSIONS=128\n\
         MDVDB_SEARCH_DEFAULT_LIMIT=5\n",
    )
    .unwrap();

    // .env: overrides model from user config.
    fs::write(
        project.path().join(".env"),
        "MDVDB_EMBEDDING_MODEL=dotenv-model\n",
    )
    .unwrap();

    // Project .markdownvdb: overrides dimensions from user config.
    fs::write(
        project.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_DIMENSIONS=512\n",
    )
    .unwrap();

    // Shell env: overrides search limit from user config.
    std::env::set_var("MDVDB_SEARCH_DEFAULT_LIMIT", "50");
    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();

    // Shell env wins for search limit.
    assert_eq!(config.search_default_limit, 50, "shell env should win");
    // Project config wins for dimensions.
    assert_eq!(config.embedding_dimensions, 512, "project config should win over .env and user");
    // .env wins for model (over user config).
    assert_eq!(config.embedding_model, "dotenv-model", ".env should win over user config");

    clear_env();
}

// ---------------------------------------------------------------------------
// Time decay config tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn decay_env_vars_override_defaults() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    std::env::set_var("MDVDB_SEARCH_DECAY", "true");
    std::env::set_var("MDVDB_SEARCH_DECAY_HALF_LIFE", "30.0");

    let config = Config::load(tmp.path()).unwrap();

    assert!(config.search_decay_enabled, "decay should be enabled via env");
    assert_eq!(config.search_decay_half_life, 30.0, "half-life should be 30 from env");

    clear_env();
}

#[test]
#[serial]
fn decay_half_life_zero_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    std::env::set_var("MDVDB_SEARCH_DECAY_HALF_LIFE", "0");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "half-life of 0 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("half_life"), "error should mention half_life: {}", err_msg);

    clear_env();
}

#[test]
#[serial]
fn decay_half_life_negative_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    std::env::set_var("MDVDB_SEARCH_DECAY_HALF_LIFE", "-10");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "negative half-life should be rejected");

    clear_env();
}

#[test]
#[serial]
fn decay_in_dotenv_file() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let dotenv_path = tmp.path().join(".markdownvdb");
    fs::write(
        &dotenv_path,
        "MDVDB_SEARCH_DECAY=true\nMDVDB_SEARCH_DECAY_HALF_LIFE=45.5\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();

    assert!(config.search_decay_enabled);
    assert_eq!(config.search_decay_half_life, 45.5);

    clear_env();
}

// ---------------------------------------------------------------------------
// Vector quantization and index compression config tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn quantization_f32_from_env() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_VECTOR_QUANTIZATION", "f32");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.vector_quantization, VectorQuantization::F32);

    clear_env();
}

#[test]
#[serial]
fn quantization_case_insensitive() {
    clear_env();
    let tmp = TempDir::new().unwrap();

    for variant in &["F16", "f16", "F32", "f32"] {
        std::env::set_var("MDVDB_VECTOR_QUANTIZATION", variant);
        let config = Config::load(tmp.path()).unwrap();
        // Just verify it parses without error
        assert!(
            config.vector_quantization == VectorQuantization::F16
                || config.vector_quantization == VectorQuantization::F32,
            "Failed for variant: {variant}"
        );
        clear_env();
    }
}

#[test]
#[serial]
fn invalid_quantization_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_VECTOR_QUANTIZATION", "f8");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "invalid quantization type should be rejected");

    clear_env();
}

#[test]
#[serial]
fn compression_disabled_via_env() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_INDEX_COMPRESSION", "false");

    let config = Config::load(tmp.path()).unwrap();
    assert!(!config.index_compression, "compression should be disabled");

    clear_env();
}

#[test]
#[serial]
fn quantization_and_compression_in_dotenv() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_VECTOR_QUANTIZATION=f32\nMDVDB_INDEX_COMPRESSION=false\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.vector_quantization, VectorQuantization::F32);
    assert!(!config.index_compression);

    clear_env();
}

// ---------------------------------------------------------------------------
// Graph traversal config tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn config_boost_hops_parse() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_BOOST_HOPS", "2");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.search_boost_hops, 2);

    clear_env();
}

#[test]
#[serial]
fn config_boost_hops_rejects_zero() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_BOOST_HOPS", "0");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "boost_hops of 0 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("search_boost_hops"),
        "error should mention search_boost_hops: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn config_boost_hops_rejects_four() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_BOOST_HOPS", "4");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "boost_hops of 4 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("search_boost_hops"),
        "error should mention search_boost_hops: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn config_expand_graph_rejects_four() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_EXPAND_GRAPH", "4");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "expand_graph of 4 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("search_expand_graph"),
        "error should mention search_expand_graph: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn config_expand_limit_rejects_zero() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_EXPAND_LIMIT", "0");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "expand_limit of 0 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("search_expand_limit"),
        "error should mention search_expand_limit: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn config_expand_limit_rejects_eleven() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_EXPAND_LIMIT", "11");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "expand_limit of 11 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("search_expand_limit"),
        "error should mention search_expand_limit: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn granularity_from_env() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_CLUSTER_GRANULARITY", "2.5");

    let config = Config::load(tmp.path()).unwrap();
    assert!((config.clustering_granularity - 2.5).abs() < f64::EPSILON);

    clear_env();
}

#[test]
#[serial]
fn granularity_too_low_rejected() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_CLUSTER_GRANULARITY", "0.1");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "granularity of 0.1 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("clustering_granularity"),
        "error should mention clustering_granularity: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn granularity_too_high_rejected() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_CLUSTER_GRANULARITY", "5.0");

    let result = Config::load(tmp.path());
    assert!(result.is_err(), "granularity of 5.0 should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("clustering_granularity"),
        "error should mention clustering_granularity: {}",
        err_msg
    );

    clear_env();
}

#[test]
#[serial]
fn granularity_in_dotenv() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_CLUSTER_GRANULARITY=0.5\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert!((config.clustering_granularity - 0.5).abs() < f64::EPSILON);

    clear_env();
}
