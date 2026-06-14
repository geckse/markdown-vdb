# PRD: Phase 28 — YAML Configuration

## Overview

Replace the dotenv-style (`KEY=VALUE`) configuration format with YAML for all mdvdb-specific settings. Project config moves from `.markdownvdb/.config` to `.markdownvdb/config.yaml`. User config moves from `~/.mdvdb/config` to `~/.mdvdb/config.yaml`. The `.env` file continues to supply secrets (`OPENAI_API_KEY`, `OLLAMA_HOST`) that are not mdvdb-specific. Shell environment variables (`MDVDB_*`) continue to override YAML values for backwards compatibility. Existing dotenv configs are auto-migrated on first load.

## Problem Statement

The current dotenv format is flat and verbose — every key requires an `MDVDB_` prefix, and there's no natural grouping. Structured data like custom clusters requires an awkward DSL (`Name:seed1,seed2|Name2:seed3`). As the config surface grows (35+ fields across 8 domains), a hierarchical format becomes essential for readability and maintainability.

YAML provides native support for nested objects, typed arrays, inline comments, and optional fields — all of which map directly to mdvdb's configuration domains (embedding, search, clustering, etc.).

## Goals

- YAML as the primary config format for `.markdownvdb/config.yaml` (project) and `~/.mdvdb/config.yaml` (user)
- Hierarchical structure: fields grouped by domain (embedding, search, chunking, clustering, watch, index, sources)
- Custom clusters as native YAML arrays (no more pipe-separated DSL)
- `.env` still loaded for secrets (`OPENAI_API_KEY`, `OLLAMA_HOST`) — these are NOT mdvdb-specific
- Shell `MDVDB_*` env vars still override any YAML value (backwards compatibility)
- Auto-migration: detect old dotenv `.config` → convert to `config.yaml`, back up old file
- Deep merge: project YAML overrides user YAML at the field level (not whole-section replacement)
- `mdvdb init` generates `config.yaml` template
- Partial configs valid — any missing field falls back to its default
- `serde_yml` crate for Rust YAML parsing (replaces `serde_yaml` 0.9 which is unmaintained)

## Non-Goals

- Removing `.env` support — secrets stay in `.env`
- Removing `MDVDB_*` env var overrides — backwards compat layer stays
- TOML or JSON config — YAML only
- Comment preservation on config re-write — `serde_yml` doesn't support round-trip comments; init template has comments, runtime writes don't
- Migrating the binary index format — only config files change
- App-side changes — those are in a separate PRD (App Phase 37)

## Technical Design

### YAML Schema

```yaml
# .markdownvdb/config.yaml
embedding:
  provider: openai          # openai | ollama | custom | mock
  model: text-embedding-3-small
  dimensions: 1536
  batch_size: 100
  endpoint: null            # optional custom endpoint URL

search:
  mode: hybrid              # hybrid | semantic | lexical | edge
  limit: 10
  min_score: 0.0
  rrf_k: 60.0
  bm25_norm_k: 1.5
  boost_links: false
  boost_hops: 1
  expand_graph: 0
  expand_limit: 3
  decay:
    enabled: false
    half_life: 90.0
    exclude: []
    include: []

chunking:
  max_tokens: 512
  overlap_tokens: 50

clustering:
  enabled: true
  rebalance_threshold: 50
  granularity: 1.0
  custom:
    - name: "AI Research"
      seeds: ["machine learning", "neural networks", "deep learning"]
    - name: "Web Dev"
      seeds: ["html", "css", "javascript", "react"]

watch:
  enabled: true
  debounce_ms: 300

index:
  quantization: f16         # f16 | f32
  compression: true
  edge_embeddings: true
  edge_boost_weight: 0.15
  edge_cluster_rebalance: 50

sources:
  dirs: ["."]
  ignore: []
```

### Serde Structs

New types in `src/config.rs` for YAML deserialization. The existing `Config` struct remains as the runtime type — `YamlConfig` is an intermediate that gets converted.

```rust
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlEmbedding {
    pub provider: String,          // default: "openai"
    pub model: String,             // default: "text-embedding-3-small"
    pub dimensions: usize,         // default: 1536
    pub batch_size: usize,         // default: 100
    pub endpoint: Option<String>,  // default: None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlSearch {
    pub mode: String,              // default: "hybrid"
    pub limit: usize,              // default: 10
    pub min_score: f64,            // default: 0.0
    pub rrf_k: f64,                // default: 60.0
    pub bm25_norm_k: f64,          // default: 1.5
    pub boost_links: bool,         // default: false
    pub boost_hops: usize,         // default: 1
    pub expand_graph: usize,       // default: 0
    pub expand_limit: usize,       // default: 3
    pub decay: YamlDecay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlDecay {
    pub enabled: bool,             // default: false
    pub half_life: f64,            // default: 90.0
    pub exclude: Vec<String>,      // default: []
    pub include: Vec<String>,      // default: []
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlChunking {
    pub max_tokens: usize,         // default: 512
    pub overlap_tokens: usize,     // default: 50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlClustering {
    pub enabled: bool,             // default: true
    pub rebalance_threshold: usize, // default: 50
    pub granularity: f64,          // default: 1.0
    pub custom: Vec<YamlCustomCluster>, // default: []
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlCustomCluster {
    pub name: String,
    pub seeds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlWatch {
    pub enabled: bool,             // default: true
    pub debounce_ms: u64,          // default: 300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlIndex {
    pub quantization: String,      // default: "f16"
    pub compression: bool,         // default: true
    pub edge_embeddings: bool,     // default: true
    pub edge_boost_weight: f64,    // default: 0.15
    pub edge_cluster_rebalance: usize, // default: 50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YamlSources {
    pub dirs: Vec<String>,         // default: ["."]
    pub ignore: Vec<String>,       // default: []
}
```

Each sub-struct implements `Default` with values matching today's hardcoded defaults. The `#[serde(default)]` on every struct means partial YAML files are valid — missing keys get defaults.

### Loading Pipeline

`Config::load(project_root)` becomes:

```
1. Load .env for secrets (dotenvy::from_path — sets env vars if not already set)
2. Load user YAML (~/.mdvdb/config.yaml) as serde_yml::Value
3. Load project YAML (.markdownvdb/config.yaml) as serde_yml::Value
4. Deep-merge project Value over user Value (recursive map merge)
5. Deserialize merged Value into YamlConfig
6. Apply MDVDB_* env var overrides onto YamlConfig struct fields
7. Convert YamlConfig -> Config (parse enums, read secrets from env, resolve paths)
8. Validate
```

**Priority chain (unchanged):** Shell env `MDVDB_*` > `.markdownvdb/config.yaml` > `.env` (secrets) > `~/.mdvdb/config.yaml` > defaults

### Deep Merge Strategy

Both user and project YAML files are first parsed as `serde_yml::Value` (not directly into `YamlConfig`). This is critical — if we deserialized both into `YamlConfig`, `#[serde(default)]` would fill in defaults for the project file, making it impossible to distinguish "user explicitly set this to the default" from "not specified".

```rust
fn merge_yaml_values(base: serde_yml::Value, overlay: serde_yml::Value) -> serde_yml::Value {
    match (base, overlay) {
        (Value::Mapping(mut base_map), Value::Mapping(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged = if let Some(base_val) = base_map.remove(&key) {
                    merge_yaml_values(base_val, overlay_val)
                } else {
                    overlay_val
                };
                base_map.insert(key, merged);
            }
            Value::Mapping(base_map)
        }
        (_, overlay) => overlay, // Scalars, sequences: overlay wins
    }
}
```

### Env Var Override Layer

Explicit field-by-field overrides — not a generic flattening scheme, because naming is inconsistent (e.g., `MDVDB_BM25_NORM_K` vs `search.bm25_norm_k`, `MDVDB_WATCH` vs `watch.enabled`).

```rust
fn apply_env_overrides(yaml: &mut YamlConfig) {
    if let Ok(v) = std::env::var("MDVDB_EMBEDDING_PROVIDER") {
        yaml.embedding.provider = v;
    }
    if let Ok(v) = std::env::var("MDVDB_EMBEDDING_DIMENSIONS") {
        if let Ok(n) = v.parse::<usize>() { yaml.embedding.dimensions = n; }
    }
    // ... one block per env var (35 total)
}
```

Full env var mapping:

| Env Var | YAML Path |
|---|---|
| `MDVDB_EMBEDDING_PROVIDER` | `embedding.provider` |
| `MDVDB_EMBEDDING_MODEL` | `embedding.model` |
| `MDVDB_EMBEDDING_DIMENSIONS` | `embedding.dimensions` |
| `MDVDB_EMBEDDING_BATCH_SIZE` | `embedding.batch_size` |
| `MDVDB_EMBEDDING_ENDPOINT` | `embedding.endpoint` |
| `MDVDB_SOURCE_DIRS` | `sources.dirs` |
| `MDVDB_IGNORE_PATTERNS` | `sources.ignore` |
| `MDVDB_WATCH` | `watch.enabled` |
| `MDVDB_WATCH_DEBOUNCE_MS` | `watch.debounce_ms` |
| `MDVDB_CHUNK_MAX_TOKENS` | `chunking.max_tokens` |
| `MDVDB_CHUNK_OVERLAP_TOKENS` | `chunking.overlap_tokens` |
| `MDVDB_CLUSTERING_ENABLED` | `clustering.enabled` |
| `MDVDB_CLUSTERING_REBALANCE_THRESHOLD` | `clustering.rebalance_threshold` |
| `MDVDB_CLUSTER_GRANULARITY` | `clustering.granularity` |
| `MDVDB_CUSTOM_CLUSTERS` | `clustering.custom` (pipe-format parsed into vec) |
| `MDVDB_SEARCH_DEFAULT_LIMIT` | `search.limit` |
| `MDVDB_SEARCH_MIN_SCORE` | `search.min_score` |
| `MDVDB_SEARCH_MODE` | `search.mode` |
| `MDVDB_SEARCH_RRF_K` | `search.rrf_k` |
| `MDVDB_BM25_NORM_K` | `search.bm25_norm_k` |
| `MDVDB_SEARCH_DECAY` | `search.decay.enabled` |
| `MDVDB_SEARCH_DECAY_HALF_LIFE` | `search.decay.half_life` |
| `MDVDB_SEARCH_DECAY_EXCLUDE` | `search.decay.exclude` |
| `MDVDB_SEARCH_DECAY_INCLUDE` | `search.decay.include` |
| `MDVDB_SEARCH_BOOST_LINKS` | `search.boost_links` |
| `MDVDB_SEARCH_BOOST_HOPS` | `search.boost_hops` |
| `MDVDB_SEARCH_EXPAND_GRAPH` | `search.expand_graph` |
| `MDVDB_SEARCH_EXPAND_LIMIT` | `search.expand_limit` |
| `MDVDB_VECTOR_QUANTIZATION` | `index.quantization` |
| `MDVDB_INDEX_COMPRESSION` | `index.compression` |
| `MDVDB_EDGE_EMBEDDINGS` | `index.edge_embeddings` |
| `MDVDB_EDGE_BOOST_WEIGHT` | `index.edge_boost_weight` |
| `MDVDB_EDGE_CLUSTER_REBALANCE` | `index.edge_cluster_rebalance` |

**Secrets** (`OPENAI_API_KEY`, `OLLAMA_HOST`) are not in the YAML schema. They continue to be read directly from env during `YamlConfig -> Config` conversion, exactly as today.

### YamlConfig -> Config Conversion

```rust
impl Config {
    fn from_yaml(yaml: YamlConfig, project_root: &Path) -> Result<Self, Error> {
        let embedding_provider = yaml.embedding.provider.parse::<EmbeddingProviderType>()?;
        let search_default_mode = yaml.search.mode.parse::<SearchMode>()?;
        let vector_quantization = yaml.search.mode.parse::<VectorQuantization>()?;
        let openai_api_key = std::env::var("OPENAI_API_KEY").ok();
        let ollama_host = std::env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());
        let source_dirs = yaml.sources.dirs.iter().map(PathBuf::from).collect();
        let custom_cluster_defs = yaml.clustering.custom.into_iter()
            .map(|c| CustomClusterDef { name: c.name, seeds: c.seeds })
            .collect();
        // ... map all remaining fields directly
        let config = Config { /* ... */ };
        config.validate()?;
        Ok(config)
    }
}
```

### Auto-Migration

Function `migrate_dotenv_to_yaml(dotenv_path: &Path, yaml_path: &Path) -> Result<(), Error>`:

1. Read dotenv file line by line, parse `KEY=VALUE` pairs
2. Map each `MDVDB_*` key to the corresponding `YamlConfig` field using the env var table
3. Build a `YamlConfig` struct from parsed values
4. Serialize to YAML with `serde_yml::to_string()`
5. Write to `config.yaml`
6. Rename old dotenv file to `.config.bak` (safety net)
7. Log: `"Migrated config from dotenv to YAML: .markdownvdb/config.yaml"`

**Detection logic in `Config::load()`:**

```rust
let yaml_path = project_root.join(".markdownvdb").join("config.yaml");
let dotenv_path = project_root.join(".markdownvdb").join(".config");
let legacy_flat = project_root.join(".markdownvdb");

if yaml_path.is_file() {
    // Load YAML directly
} else if dotenv_path.is_file() {
    migrate_dotenv_to_yaml(&dotenv_path, &yaml_path)?;
    // Then load YAML
} else if legacy_flat.is_file() {
    // Move flat file to .markdownvdb/.config (existing migration)
    // Then migrate dotenv to YAML
}
```

Same detection + migration for user-level: `~/.mdvdb/config` -> `~/.mdvdb/config.yaml`.

### Config Writing

Replace `update_config_value(path, key, value)` with YAML-aware functions:

```rust
/// Write the full YAML config to a file.
pub fn write_yaml_config(path: &Path, config: &YamlConfig) -> Result<(), Error>

/// Read a YAML config file, update a specific nested key, and write back.
/// key_path uses dot notation: "embedding.provider", "search.decay.enabled"
pub fn update_yaml_config_value(
    path: &Path,
    key_path: &str,
    value: serde_yml::Value,
) -> Result<(), Error>
```

The `encode_custom_clusters()` and `parse_custom_clusters_value()` public functions remain available for env var backwards compat but are no longer needed for config file operations.

### Init Template Changes

`MarkdownVdb::init()` generates `.markdownvdb/config.yaml`:

```yaml
# markdown-vdb project configuration
# See: https://github.com/user/markdown-vdb#configuration

embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: 1536
  batch_size: 100

search:
  mode: hybrid
  limit: 10
  min_score: 0.0

chunking:
  max_tokens: 512
  overlap_tokens: 50

clustering:
  enabled: true

watch:
  enabled: true
  debounce_ms: 300

sources:
  dirs: ["."]
```

`MarkdownVdb::init_global()` generates `~/.mdvdb/config.yaml`:

```yaml
# mdvdb user-level configuration
# API keys should go in .env files, not here

embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: 1536
```

### Clusters CLI Update

The `clusters add/remove/list` subcommands in `src/main.rs` currently read/write the pipe-separated `MDVDB_CUSTOM_CLUSTERS` in the dotenv file. Update to:

- `clusters add <NAME> --seeds <SEEDS>`: Read `config.yaml`, append to `clustering.custom` array, write back
- `clusters remove <NAME>`: Read `config.yaml`, filter from `clustering.custom` array, write back
- `clusters list`: Read `config.yaml`, display `clustering.custom` entries

### Dependency Change

Replace `serde_yaml = "0.9"` with `serde_yml` in `Cargo.toml`. `serde_yml` is the maintained successor — API is nearly identical (`serde_yml::from_str`, `serde_yml::to_string`, `serde_yml::Value`). Update all existing `serde_yaml` imports across the codebase (`src/parser.rs`, `src/schema.rs`, `src/config.rs`).

## Implementation Steps

1. **Dependency swap** — Replace `serde_yaml` with `serde_yml` in `Cargo.toml`. Update all existing imports in `src/parser.rs`, `src/schema.rs`.

2. **YAML serde structs** — `src/config.rs`: Add `YamlConfig` and all sub-structs with `Serialize + Deserialize + Default` derives. Implement `Default` for each sub-struct matching today's hardcoded defaults.

3. **Deep merge** — `src/config.rs`: Add `merge_yaml_values()` function for recursive `serde_yml::Value` merging.

4. **Env var overrides** — `src/config.rs`: Add `apply_env_overrides(yaml: &mut YamlConfig)` with explicit per-field checks.

5. **YamlConfig -> Config conversion** — `src/config.rs`: Add `Config::from_yaml()` that parses enums, reads secrets, resolves paths, validates.

6. **Migration function** — `src/config.rs`: Add `migrate_dotenv_to_yaml()` with KEY->YAML mapping, backup, and logging.

7. **Rewrite Config::load()** — `src/config.rs`: New pipeline: `.env` for secrets → load user YAML → load project YAML → deep merge → env overrides → convert → validate. With auto-migration detection.

8. **Config writing** — `src/config.rs`: Replace `update_config_value()` with `write_yaml_config()` and `update_yaml_config_value()`.

9. **Init templates** — `src/lib.rs`: Update `init()` and `init_global()` to write `config.yaml` with YAML templates.

10. **Clusters CLI** — `src/main.rs`: Update `clusters add/remove/list` to read/write `clustering.custom` in `config.yaml`.

11. **Config CLI** — `src/main.rs`: Update `mdvdb config` display command if it references dotenv format.

12. **Update tests** — `src/config.rs` unit tests + `tests/config_test.rs` integration tests: Rewrite dotenv file writes to YAML. Add migration tests. Add deep merge tests. Add env override + YAML combination tests. Update any other test files that write `.markdownvdb/.config`.

13. **CLAUDE.md update** — Update config format documentation, file paths, priority chain description.

## Files Modified

| File | Change |
|---|---|
| `Cargo.toml` | Replace `serde_yaml = "0.9"` with `serde_yml` |
| `src/config.rs` | `YamlConfig` structs, `Config::load()` rewrite, `merge_yaml_values()`, `apply_env_overrides()`, `Config::from_yaml()`, `migrate_dotenv_to_yaml()`, `write_yaml_config()`, `update_yaml_config_value()` |
| `src/lib.rs` | `init()` and `init_global()` templates → YAML. Config file path constants. |
| `src/main.rs` | `clusters add/remove/list` → YAML read/write. `config` display command. |
| `src/parser.rs` | `serde_yaml` → `serde_yml` import update |
| `src/schema.rs` | `serde_yaml` → `serde_yml` import update |
| `tests/config_test.rs` | Rewrite all dotenv file writes to YAML, add migration + merge tests |
| `tests/cli_test.rs` | Update any tests that write `.markdownvdb/.config` |
| `tests/api_test.rs` | Update config file creation in test helpers |
| `tests/ingest_test.rs` | Update config file creation |
| `CLAUDE.md` | Config format docs, file paths, env var table |

## Validation Criteria

- [ ] `cargo test` passes — all existing + new tests
- [ ] `cargo clippy --all-targets` — zero warnings
- [ ] `mdvdb init` creates `.markdownvdb/config.yaml` with valid YAML template
- [ ] `mdvdb init --global` creates `~/.mdvdb/config.yaml`
- [ ] Partial `config.yaml` (e.g., only `embedding.provider: ollama`) loads correctly with defaults for all other fields
- [ ] User YAML + project YAML deep-merge works: project overrides specific fields without replacing entire sections
- [ ] `MDVDB_SEARCH_MODE=semantic` env var overrides `search.mode: hybrid` in YAML
- [ ] `.env` with `OPENAI_API_KEY=sk-xxx` still provides the API key
- [ ] Old dotenv `.markdownvdb/.config` auto-migrates to `config.yaml` on first load
- [ ] Old file renamed to `.config.bak` after migration
- [ ] Legacy flat `.markdownvdb` file migration still works (two-step: flat → dotenv dir → YAML)
- [ ] User-level `~/.mdvdb/config` auto-migrates to `~/.mdvdb/config.yaml`
- [ ] Custom clusters in YAML: `clustering.custom` array with `name` + `seeds` works after ingest
- [ ] `mdvdb clusters add "Test" --seeds "foo,bar"` writes to `config.yaml` correctly
- [ ] `mdvdb clusters remove "Test"` removes from `config.yaml`
- [ ] `mdvdb config` shows resolved configuration correctly
- [ ] All existing `serde_yaml` usages (parser.rs, schema.rs) work with `serde_yml`
- [ ] `MDVDB_CUSTOM_CLUSTERS=A:x,y|B:z` env var override still works (backwards compat)
- [ ] Empty/missing config files result in sensible defaults (no errors)
- [ ] Priority chain correct: env var > project YAML > .env secrets > user YAML > defaults

## Anti-Patterns to Avoid

- **Do not put secrets in YAML** — `OPENAI_API_KEY` and `OLLAMA_HOST` stay in `.env` / shell env. They are not mdvdb-specific and should not be version-controlled.

- **Do not use generic env-var-to-YAML flattening** — The naming is inconsistent (`MDVDB_BM25_NORM_K` vs `search.bm25_norm_k`, `MDVDB_WATCH` vs `watch.enabled`). Use explicit per-field mapping.

- **Do not deserialize both YAML files into YamlConfig for merging** — `#[serde(default)]` fills in defaults, making it impossible to distinguish "explicitly set to default" from "not specified". Use `serde_yml::Value` deep merge.

- **Do not delete the old config file during migration** — Rename to `.config.bak` as a safety net. Users can revert if needed.

- **Do not remove dotenvy dependency** — Still needed for `.env` file loading (secrets).

- **Do not remove MDVDB_* env var support** — These must continue to work for CI/CD, Docker, and scripting use cases.

## Patterns to Follow

- **Config loading:** Current `Config::load()` at `src/config.rs:131` — same priority chain, different file format
- **Enum parsing:** `EmbeddingProviderType::from_str()` at `src/config.rs:19`, `SearchMode::from_str()`, `VectorQuantization::from_str()` — reused in `Config::from_yaml()`
- **Config validation:** `Config::validate()` at `src/config.rs:275` — unchanged, runs after conversion
- **Init templates:** `MarkdownVdb::init()` at `src/lib.rs:1578` — same pattern, YAML content
- **Config file detection:** Legacy migration at `src/lib.rs:413-426` — extend with YAML detection
- **Test helpers:** `mock_config()` pattern in test files — update to build `Config` from `YamlConfig`

## Dependencies

None — this is a foundation change that other features depend on, not the reverse. Existing Phase 27 (custom clusters) dotenv format will be migrated as part of this phase.
