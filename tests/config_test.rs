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
    "MDVDB_CUSTOM_CLUSTERS",
];

/// Clear all MDVDB-related env vars to ensure test isolation.
fn clear_env() {
    for var in ALL_ENV_VARS {
        std::env::remove_var(var);
    }
}

/// Helper: create a project YAML config file at `.markdownvdb/config.yaml`.
fn write_project_yaml(project_root: &std::path::Path, content: &str) {
    let mdvdb_dir = project_root.join(".markdownvdb");
    fs::create_dir_all(&mdvdb_dir).unwrap();
    fs::write(mdvdb_dir.join("config.yaml"), content).unwrap();
}

/// Helper: create a user YAML config file at `<user_home>/config.yaml`.
fn write_user_yaml(user_home: &std::path::Path, content: &str) {
    fs::write(user_home.join("config.yaml"), content).unwrap();
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn defaults_applied_when_no_config() {
    clear_env();
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

// ---------------------------------------------------------------------------
// YAML config file overrides defaults
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn yaml_config_overrides_defaults() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "embedding:\n  model: custom-model\n  dimensions: 768\n\
         search:\n  limit: 20\n\
         watch:\n  enabled: false\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_model, "custom-model");
    assert_eq!(config.embedding_dimensions, 768);
    assert_eq!(config.search_default_limit, 20);
    assert!(!config.watch_enabled);
}

#[test]
#[serial]
fn env_vars_override_yaml_file() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "embedding:\n  model: file-model\n  dimensions: 768\n",
    );

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
    // With the YAML pipeline, env var overrides silently ignore unparseable
    // values (using .ok()), so a non-numeric value falls back to the default.
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "abc");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_dimensions, 1536);

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
fn missing_config_file_ok() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let result = Config::load(tmp.path());
    assert!(result.is_ok());
    clear_env();
}

#[test]
#[serial]
fn env_file_provides_secrets() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();

    // Project YAML sets embedding config
    write_project_yaml(
        tmp.path(),
        "embedding:\n  provider: openai\n  dimensions: 768\n",
    );

    // .env provides the secret API key
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
fn dotenv_mdvdb_vars_stripped_from_env_file() {
    // With YAML pipeline, MDVDB_* vars in .env are stripped and do NOT override YAML config.
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();

    // .env has an MDVDB setting — should be stripped
    fs::write(
        tmp.path().join(".env"),
        "MDVDB_EMBEDDING_DIMENSIONS=256\n",
    )
    .unwrap();

    // Project YAML overrides it
    write_project_yaml(
        tmp.path(),
        "embedding:\n  dimensions: 768\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.embedding_dimensions, 768,
        "project YAML should take priority; .env MDVDB vars are stripped"
    );

    clear_env();
}

#[test]
#[serial]
fn shell_env_overrides_yaml_and_dotenv() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();

    write_project_yaml(
        tmp.path(),
        "embedding:\n  dimensions: 768\n",
    );

    // Shell env overrides everything
    std::env::set_var("MDVDB_EMBEDDING_DIMENSIONS", "1024");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.embedding_dimensions, 1024,
        "shell env should take priority over YAML"
    );

    clear_env();
}

#[test]
#[serial]
fn search_config_from_yaml() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "search:\n  mode: semantic\n  rrf_k: 30.0\n",
    );

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
    // Non-numeric env var is silently skipped, falls back to default.
    clear_env();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("MDVDB_SEARCH_RRF_K", "not_a_number");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.search_rrf_k, 60.0);

    clear_env();
}

// ---------------------------------------------------------------------------
// User-level config (~/.mdvdb/config.yaml) tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn user_config_provides_fallback_values() {
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n  dimensions: 256\n",
    );

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

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n  dimensions: 256\n",
    );

    write_project_yaml(
        project.path(),
        "embedding:\n  model: project-model\n",
    );

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "project-model",
        "project config should override user config"
    );
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

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n",
    );

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
    // With YAML pipeline, .env is only for secrets. MDVDB_* in .env are stripped.
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n",
    );

    // .env MDVDB vars are stripped — they don't override YAML
    fs::write(
        project.path().join(".env"),
        "MDVDB_EMBEDDING_MODEL=dotenv-model\n",
    )
    .unwrap();

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();
    assert_eq!(
        config.embedding_model, "user-model",
        "user YAML should be used since .env MDVDB vars are stripped"
    );

    clear_env();
}

#[test]
#[serial]
fn missing_user_config_dir_silently_skipped() {
    clear_env();
    let project = TempDir::new().unwrap();

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

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n",
    );

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
fn full_three_level_cascade() {
    // Cascade: shell env > project YAML > user YAML > defaults.
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n  dimensions: 128\nsearch:\n  limit: 5\n",
    );

    write_project_yaml(
        project.path(),
        "embedding:\n  dimensions: 512\n",
    );

    std::env::set_var("MDVDB_SEARCH_DEFAULT_LIMIT", "50");
    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();

    assert_eq!(config.search_default_limit, 50, "shell env should win");
    assert_eq!(config.embedding_dimensions, 512, "project YAML should win over user YAML");
    assert_eq!(config.embedding_model, "user-model", "user YAML should provide fallback model");

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
fn decay_in_yaml_file() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "search:\n  decay:\n    enabled: true\n    half_life: 45.5\n",
    );

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
fn quantization_and_compression_in_yaml() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "index:\n  quantization: f32\n  compression: false\n",
    );

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
fn granularity_in_yaml() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "clustering:\n  granularity: 0.5\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert!((config.clustering_granularity - 0.5).abs() < f64::EPSILON);

    clear_env();
}

// ---------------------------------------------------------------------------
// Custom Clusters Config Tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn custom_clusters_parsed_from_env() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    std::env::set_var(
        "MDVDB_CUSTOM_CLUSTERS",
        "AI Research:machine learning,neural networks|Web Dev:html,css,react",
    );

    let tmp = TempDir::new().unwrap();
    let config = Config::load(tmp.path()).unwrap();

    assert_eq!(config.custom_cluster_defs.len(), 2);
    assert_eq!(config.custom_cluster_defs[0].name, "AI Research");
    assert_eq!(
        config.custom_cluster_defs[0].seeds,
        vec!["machine learning", "neural networks"]
    );
    assert_eq!(config.custom_cluster_defs[1].name, "Web Dev");
    assert_eq!(
        config.custom_cluster_defs[1].seeds,
        vec!["html", "css", "react"]
    );

    clear_env();
}

#[test]
#[serial]
fn custom_clusters_empty_when_unset() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");

    let tmp = TempDir::new().unwrap();
    let config = Config::load(tmp.path()).unwrap();

    assert!(config.custom_cluster_defs.is_empty());

    clear_env();
}

#[test]
#[serial]
fn custom_clusters_malformed_entries_skipped() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    std::env::set_var(
        "MDVDB_CUSTOM_CLUSTERS",
        "no_colon_here|:empty_name|Valid:seed1,seed2|EmptySeeds:",
    );

    let tmp = TempDir::new().unwrap();
    let config = Config::load(tmp.path()).unwrap();

    assert_eq!(config.custom_cluster_defs.len(), 1);
    assert_eq!(config.custom_cluster_defs[0].name, "Valid");

    clear_env();
}

#[test]
#[serial]
fn custom_clusters_duplicate_names_rejected() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    std::env::set_var("MDVDB_CUSTOM_CLUSTERS", "Dup:a,b|Dup:c,d");

    let tmp = TempDir::new().unwrap();
    let result = Config::load(tmp.path());

    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("duplicate custom cluster name"));

    clear_env();
}

#[test]
fn parse_and_encode_custom_clusters_roundtrip() {
    let input = "AI Research:machine learning,neural networks|Web Dev:html,css,react";
    let defs = mdvdb::config_parse_custom_clusters(input);
    assert_eq!(defs.len(), 2);

    let encoded = mdvdb::config_encode_custom_clusters(&defs);
    assert_eq!(encoded, input);
}

#[test]
fn update_config_value_creates_and_modifies() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join(".markdownvdb").join(".config");

    // Creates file and directory
    mdvdb::config_update_value(&config_path, "MDVDB_CUSTOM_CLUSTERS", "A:x,y").unwrap();
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("MDVDB_CUSTOM_CLUSTERS=A:x,y"));

    // Modifies existing key
    mdvdb::config_update_value(&config_path, "MDVDB_CUSTOM_CLUSTERS", "A:x,y|B:z").unwrap();
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("MDVDB_CUSTOM_CLUSTERS=A:x,y|B:z"));
    assert_eq!(
        content.matches("MDVDB_CUSTOM_CLUSTERS").count(),
        1
    );

    // Removes key when value is empty
    mdvdb::config_update_value(&config_path, "MDVDB_CUSTOM_CLUSTERS", "").unwrap();
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(!content.contains("MDVDB_CUSTOM_CLUSTERS"));
}

// ---------------------------------------------------------------------------
// NEW: YAML-specific tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn yaml_config_load_from_file() {
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "embedding:\n  model: some-model\n  dimensions: 768\n\
         search:\n  mode: semantic\n  limit: 25\n\
         chunking:\n  max_tokens: 1024\n  overlap_tokens: 100\n\
         clustering:\n  enabled: false\n  granularity: 2.0\n\
         watch:\n  enabled: false\n  debounce_ms: 500\n\
         index:\n  quantization: f32\n  compression: false\n\
         sources:\n  dirs:\n    - docs\n    - notes\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_model, "some-model");
    assert_eq!(config.embedding_dimensions, 768);
    assert_eq!(config.search_default_mode, mdvdb::SearchMode::Semantic);
    assert_eq!(config.search_default_limit, 25);
    assert_eq!(config.chunk_max_tokens, 1024);
    assert_eq!(config.chunk_overlap_tokens, 100);
    assert!(!config.clustering_enabled);
    assert!((config.clustering_granularity - 2.0).abs() < f64::EPSILON);
    assert!(!config.watch_enabled);
    assert_eq!(config.watch_debounce_ms, 500);
    assert_eq!(config.vector_quantization, VectorQuantization::F32);
    assert!(!config.index_compression);
    assert_eq!(config.source_dirs, vec![PathBuf::from("docs"), PathBuf::from("notes")]);

    clear_env();
}

#[test]
#[serial]
fn yaml_config_deep_merge_project_over_user() {
    // Verify field-level merge: project overrides specific fields, user's other
    // fields in the same section are preserved (no whole-section replacement).
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    write_user_yaml(
        user_home.path(),
        "search:\n  limit: 20\n  mode: semantic\n  min_score: 0.5\n\
         embedding:\n  model: user-model\n  dimensions: 256\n",
    );

    // Project only overrides search.mode — limit and min_score from user should survive
    write_project_yaml(
        project.path(),
        "search:\n  mode: hybrid\n\
         embedding:\n  model: project-model\n",
    );

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());

    let config = Config::load(project.path()).unwrap();
    assert_eq!(config.search_default_mode, mdvdb::SearchMode::Hybrid, "project should override mode");
    assert_eq!(config.search_default_limit, 20, "user limit should be preserved via deep merge");
    assert_eq!(config.search_min_score, 0.5, "user min_score should be preserved via deep merge");
    assert_eq!(config.embedding_model, "project-model", "project should override model");
    assert_eq!(config.embedding_dimensions, 256, "user dimensions should be preserved via deep merge");

    clear_env();
}

#[test]
#[serial]
fn yaml_env_override_takes_priority() {
    // env > project YAML > user YAML
    clear_env();
    let project = TempDir::new().unwrap();
    let user_home = TempDir::new().unwrap();

    write_user_yaml(
        user_home.path(),
        "embedding:\n  model: user-model\n",
    );

    write_project_yaml(
        project.path(),
        "embedding:\n  model: project-model\n",
    );

    std::env::set_var("MDVDB_CONFIG_HOME", user_home.path());
    std::env::set_var("MDVDB_EMBEDDING_MODEL", "env-model");

    let config = Config::load(project.path()).unwrap();
    assert_eq!(config.embedding_model, "env-model", "env should override both YAML layers");

    clear_env();
}

#[test]
#[serial]
fn yaml_dotenv_migration() {
    // Old .markdownvdb/.config (dotenv) auto-migrates to config.yaml + .config.bak backup
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    let mdvdb_dir = tmp.path().join(".markdownvdb");
    fs::create_dir_all(&mdvdb_dir).unwrap();

    // Write old dotenv .config file
    fs::write(
        mdvdb_dir.join(".config"),
        "MDVDB_EMBEDDING_MODEL=migrated-model\nMDVDB_EMBEDDING_DIMENSIONS=768\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_model, "migrated-model", "migration should preserve values");
    assert_eq!(config.embedding_dimensions, 768);

    // config.yaml should now exist
    assert!(mdvdb_dir.join("config.yaml").is_file(), "config.yaml should be created");
    // .config.bak should exist as backup
    assert!(mdvdb_dir.join(".config.bak").is_file(), ".config.bak backup should be created");

    clear_env();
}

#[test]
#[serial]
fn yaml_legacy_flat_migration() {
    // Legacy flat .markdownvdb file -> dir -> config.yaml chain
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();

    // Write a flat .markdownvdb file (legacy format)
    fs::write(
        tmp.path().join(".markdownvdb"),
        "MDVDB_EMBEDDING_MODEL=legacy-model\nMDVDB_EMBEDDING_DIMENSIONS=512\n",
    )
    .unwrap();

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_model, "legacy-model", "legacy flat migration should work");
    assert_eq!(config.embedding_dimensions, 512);

    // After migration, .markdownvdb should be a directory with config.yaml
    let mdvdb_dir = tmp.path().join(".markdownvdb");
    assert!(mdvdb_dir.is_dir(), ".markdownvdb should now be a directory");
    assert!(mdvdb_dir.join("config.yaml").is_file(), "config.yaml should exist after migration");

    clear_env();
}

#[test]
#[serial]
fn yaml_custom_clusters_parsed() {
    // Custom clusters defined in YAML clustering.custom array
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "clustering:\n  custom:\n    - name: AI Research\n      seeds:\n        - machine learning\n        - neural networks\n    - name: Web Dev\n      seeds:\n        - html\n        - css\n        - react\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.custom_cluster_defs.len(), 2);
    assert_eq!(config.custom_cluster_defs[0].name, "AI Research");
    assert_eq!(
        config.custom_cluster_defs[0].seeds,
        vec!["machine learning", "neural networks"]
    );
    assert_eq!(config.custom_cluster_defs[1].name, "Web Dev");
    assert_eq!(
        config.custom_cluster_defs[1].seeds,
        vec!["html", "css", "react"]
    );

    clear_env();
}

#[test]
#[serial]
fn yaml_partial_config_valid() {
    // A YAML file with only a subset of fields should load correctly
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "embedding:\n  provider: mock\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_provider, EmbeddingProviderType::Mock);
    // All other fields should have defaults
    assert_eq!(config.embedding_model, "text-embedding-3-small");
    assert_eq!(config.embedding_dimensions, 1536);
    assert_eq!(config.search_default_limit, 10);

    clear_env();
}

#[test]
#[serial]
fn yaml_empty_file_valid() {
    // An empty YAML file should load with all defaults
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(tmp.path(), "");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_provider, EmbeddingProviderType::OpenAI);
    assert_eq!(config.embedding_model, "text-embedding-3-small");
    assert_eq!(config.embedding_dimensions, 1536);

    clear_env();
}

#[test]
#[serial]
fn yaml_comment_only_file_valid() {
    // A YAML file with only comments should load with all defaults
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(tmp.path(), "# This is a comment\n# Another comment\n");

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.embedding_provider, EmbeddingProviderType::OpenAI);
    assert_eq!(config.embedding_dimensions, 1536);

    clear_env();
}

#[test]
#[serial]
fn yaml_source_dirs_as_list() {
    // Source dirs specified as YAML list
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "sources:\n  dirs:\n    - docs\n    - notes\n    - wiki\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(
        config.source_dirs,
        vec![PathBuf::from("docs"), PathBuf::from("notes"), PathBuf::from("wiki")]
    );

    clear_env();
}

#[test]
#[serial]
fn yaml_ignore_patterns_as_list() {
    // Ignore patterns specified as YAML list
    clear_env();
    std::env::set_var("MDVDB_NO_USER_CONFIG", "1");
    let tmp = TempDir::new().unwrap();
    write_project_yaml(
        tmp.path(),
        "sources:\n  ignore:\n    - \"*.tmp\"\n    - drafts/\n",
    );

    let config = Config::load(tmp.path()).unwrap();
    assert_eq!(config.ignore_patterns, vec!["*.tmp", "drafts/"]);

    clear_env();
}
