# Markdown VDB

A filesystem-native vector database built around Markdown files. Rust, zero infrastructure, optimized for AI agents.

All 18 implementation phases plus graph-enhanced search, the Leiden/topics clustering rework, and frontmatter relations (phase 31) are complete and passing (1032 tests, clippy clean).

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Agent Interface                       │
│              CLI (clap) + Library API                    │
│         mdvdb search | ingest | status | watch          │
├──────────┬──────────┬───────────┬───────────────────────┤
│  Search  │  Schema  │ Clustering│   File Watcher        │
│  Engine  │  System  │  (linfa)  │   (notify)            │
├──────────┴──────────┴───────────┴───────────────────────┤
│                   Index Storage                         │
│        usearch (HNSW) + rkyv (metadata) + memmap2       │
│          parking_lot::RwLock (concurrency)               │
├──────────┬──────────────────────────────────────────────┤
│ Embedding│   OpenAI | Ollama | Custom (reqwest)         │
│ Providers│   Batch processing + content-hash skip       │
├──────────┴──────────────────────────────────────────────┤
│                  Chunking Engine                        │
│      Heading-split + token size guard (tiktoken-rs)     │
├─────────────────────────────────────────────────────────┤
│              Markdown Parsing & Discovery                │
│    pulldown-cmark + serde_yml + ignore + sha2           │
├─────────────────────────────────────────────────────────┤
│               Foundation & Configuration                │
│      serde_yml + dotenvy + thiserror + anyhow + tracing │
└─────────────────────────────────────────────────────────┘
```

## Project Structure

```
src/
├── main.rs              # CLI entry point (clap + anyhow)
├── lib.rs               # Public library API (MarkdownVdb)
├── config.rs            # Config loading: shell env MDVDB_* → project YAML → .env secrets → user YAML → defaults
├── format.rs            # Human-readable output formatting (colors, bars, timestamps)
├── error.rs             # Error enum (thiserror)
├── logging.rs           # Tracing subscriber setup
├── discovery.rs         # File scanning with ignore patterns (.gitignore + .mdvdbignore)
├── parser.rs            # Markdown parsing: frontmatter, headings, body
├── chunker.rs           # Heading-based chunking + token size guard
├── search.rs            # Query pipeline, metadata filtering, time decay, graph expansion, results
├── fts.rs               # Full-text search (Tantivy BM25 wrapper)
├── links.rs             # Link graph extraction, backlinks, orphan detection, multi-hop BFS, neighborhood
├── relations.rs         # Frontmatter relations: link-shape predicate, 3-step target resolution, RelationValue/ReferencedBy
├── tree.rs              # File tree with sync status indicators
├── schema.rs            # Auto-infer + overlay schema system
├── clustering/
│   ├── mod.rs           # Cluster types, Clusterer facade, stability matching, topics (multi-label)
│   ├── leiden.rs        # Cosine k-NN graph + seeded Leiden community detection + hierarchy
│   ├── kmeans.rs        # Seeded K-means fallback (also backs edge clustering)
│   └── labels.rs        # TF-IDF keywords (unigrams+bigrams, smoothed IDF), label generation
├── watcher.rs           # Filesystem watcher (notify + debouncer)
├── ingest.rs            # Full + incremental ingestion pipeline
├── embedding/
│   ├── mod.rs           # EmbeddingProvider trait + factory
│   ├── provider.rs      # Trait definition
│   ├── openai.rs        # OpenAI-compatible provider
│   ├── ollama.rs        # Ollama provider
│   ├── batch.rs         # Concurrent batch orchestration (up to 4) + hash skip
│   └── mock.rs          # Mock provider for testing
└── index/
    ├── mod.rs           # Index public API
    ├── types.rs         # StoredChunk, StoredFile, IndexMetadata (rkyv)
    ├── storage.rs       # File I/O: header + rkyv region + usearch region
    └── state.rs         # Runtime operations with RwLock concurrency

tests/
├── api_test.rs          # Library API integration tests
├── cli_test.rs          # CLI binary integration tests
├── chunker_test.rs      # Chunking pipeline tests
├── clustering_test.rs   # Leiden/K-means + topics clustering tests
├── config_test.rs       # Configuration loading tests
├── discovery_test.rs    # File discovery tests
├── embedding_test.rs    # Embedding provider tests
├── fts_test.rs          # Full-text search (Tantivy BM25) tests
├── graph_test.rs        # Graph traversal + multi-hop search tests
├── index_test.rs        # Index storage + mtime tests
├── ingest_test.rs       # Ingestion pipeline tests
├── links_test.rs        # Link graph + backlinks tests
├── parser_test.rs       # Markdown parsing tests
├── relations_test.rs    # Frontmatter relations (populate, graph edges, filters, doctor) tests
├── schema_test.rs       # Schema inference tests
├── search_test.rs       # Search engine + time decay tests
├── tree_test.rs         # File tree tests
└── watcher_test.rs      # File watcher tests

docs/prds/               # PRD specifications for all 18 phases (reference)
```

## Core Design Decisions

- **Config:** YAML config files with deep merge strategy. Resolution: shell env `MDVDB_*` > `.markdownvdb/config.yaml` (project) > `.env` (secrets only) > `~/.mdvdb/config.yaml` (user) > defaults. Legacy dotenv configs are auto-migrated on first load. **Secrets** (`OPENAI_API_KEY`, `OLLAMA_HOST`) never live in YAML — they resolve shell env > `<root>/.env` > `<root>/.markdownvdb/.env` > `~/.mdvdb/.env` > legacy `~/.mdvdb/config` (all via dotenvy, non-overriding; `MDVDB_*` keys introduced by these files are stripped). Dotenv migration preserves non-MDVDB keys into a sibling `.env` instead of dropping them. The YAML config is organized into 7 domains: `embedding` (provider, model, dimensions, batch size), `search` (default limit, mode, weights, decay settings), `chunking` (max tokens, overlap), `clustering` (algorithm, knn, resolution, min_cluster_size, topics defaults, custom topic definitions), `watch` (debounce interval), `index` (directory path), and `sources` (ignore patterns). Multiple YAML files are deep-merged recursively (maps merged key-by-key, scalars overwritten by higher-priority source).
- **Index directory:** `.markdownvdb/` contains `config.yaml` + `index` (binary: `[64B header][rkyv metadata][usearch HNSW]`; unreadable/outdated index files are deleted and rebuilt on open) + `fts/` (Tantivy BM25 segments). Configured via `MDVDB_INDEX_DIR` or `index.dir` in YAML.
- **Paths:** ALL file paths in the index are relative to project root. Never absolute.
- **Shards (phase 33):** A Shard is a project-local named recursive folder lens stored under
  `shards:` in the raw `.markdownvdb/config.yaml`. It reuses the Collection's one index, watcher,
  link graph, and root-relative document identities; it is not an index partition or access
  boundary. Never read Shards from merged user config. All path scopes use
  `path_util::path_is_in_scope` so `docs` cannot match `docs-old`. Shard/config/topic/settings
  YAML mutations share `.markdownvdb/config.lock`, preserve unrelated keys, and write atomically.
  Management never touches the index and therefore requires no index/version bump. Full spec:
  `docs/prds/phase-33-named-shards.md` (app counterpart:
  `app/docs/prds/phase-47-named-shards.md`).
- **Shard-native analysis (phase 34):** Shards remain shared-index lenses, but their graph analysis
  is independent: strict in-Shard topology, automatic clusters recomputed from only in-Shard
  stored document vectors, and local Topic definitions stored as `shards.<id>.topics` in raw
  project YAML with no Collection/ancestor/sibling inheritance. Disposable state lives in
  `.markdownvdb/cache/shards/<id>.json`; it may contain derived cluster/Topic state and centroids,
  never document embeddings or topology. Read-only graph/cluster requests never initialize an
  embedding provider. A stale Topic centroid reports `needs_ingest`, while corpus-only changes
  reuse compatible centroids. `graph --shard ID --path DESCENDANT` analyzes the complete Shard and
  only projects visible topology. No index or compact-wire version changes. Full spec:
  `docs/prds/phase-34-shard-native-graph-analysis.md` (app counterpart:
  `app/docs/prds/phase-48-shard-native-graph-analysis.md`).
- **Errors:** `thiserror` for typed library errors, `anyhow` only at CLI boundary in `main.rs`
- **Concurrency:** `parking_lot::RwLock` (not std). Read lock for queries, write lock only during upsert.
- **Writes:** Always atomic — write to `.tmp`, fsync, rename. Never write directly to index file.
- **Frontmatter:** User-authored fields are edited only through explicit app/CLI actions. The always-on Formula module materializes formula results into Markdown frontmatter atomically; Markdown remains the source of truth. Formula write-backs must preserve unrelated YAML and must synchronize source hashes without re-embedding an unchanged Markdown body. Computed sets (formula/lookup/rollup) **adopt-by-declaration**: an existing same-named frontmatter value without index ownership proof is overwritten and claimed, because the overlay declares the field computed and the index (where proof lives) is disposable — rebuilds must self-heal. Malformed frontmatter is never adopted through; unset authority stays strict (only proven-owned values are removed). See the 2026-08 amendment in `docs/prds/phase-35-lookup-rollup-computed-fields.md`.
- **Relations (phase 31):** Whole-value frontmatter links (`client: clients/acme.md` by default; wiki links such as `client: "[[clients/acme]]"` and markdown links remain supported) are foreign keys. Detection is **value-driven** (never schema-driven — persisted schemas go stale after single-file ingest); the schema contributes only the `Relation` label (`FieldType::Relation`, PascalCase `"Relation"`) and the overlay-declared `relation_target` folder (slash-less; overlay key `target:`, `type:` accepted as alias for `field_type:`). Resolution order for frontmatter targets: contains `/` → root-relative (source-dir fallback) → overlay target folder → source-dir-relative; body-link resolution is unchanged. Relation edges join the link graph as `LinkEntry`s tagged with `field` + `line_number: 0` sentinel (`field != null ⇔ line_number == 0`; dedup key `(target, field)`; edge id `@fm.<field>`), so backlinks/orphans/boost/`--expand` see them with zero changes. `--populate` on `get`/`collection`/`search` resolves values to `RelationValue {raw, path, exists, title, frontmatter}` (depth 1, never nested; `frontmatter` and `field` are always-present JSON keys — do NOT skip-serialize); `get --populate` adds `referenced_by` sorted by `(source, field)`. Collection populates page rows only; `rows[].frontmatter` stays raw. `Equals`/`In` filters normalize relation syntax at match time (purely syntactic). Doctor has a "Relations" check (dangling targets, overlay hygiene, unquoted-`[[x]]` YAML footgun). Full spec: `docs/prds/phase-31-frontmatter-relations.md` (app counterpart: `app/docs/prds/phase-42-frontmatter-relations.md`).
- **Embeddings:** Trait-based pluggable providers. Batch-first (up to 4 concurrent). Skip unchanged files via SHA-256 hash. The OpenAI provider transparently splits any `embed_batch` call into API-limit-sized requests (280k tokens/request, 2048 inputs/request, safety margins under OpenAI's hard limits) and truncates single inputs over 8k tokens. Link-context paragraphs are capped at 2000 bytes (window centered on the link's line, `MAX_LINK_PARAGRAPH_BYTES` in `parser.rs`), and byte-identical edge-context texts are embedded once per ingest (aliased in `lib.rs`), so a link-list file cannot multiply embedding cost.
- **Ignore files:** `.gitignore` respected automatically. `.mdvdbignore` (same syntax) for index-only exclusions. 15 built-in dir ignores always applied. `MDVDB_IGNORE_PATTERNS` env var for additional patterns.
- **Chunking:** Primary split by headings, secondary token-count size guard. Deterministic `"path#index"` IDs.
- **Clustering:** Document-level vectors (averaged chunk vectors per file, unit-normalized; cosine everywhere). Default algorithm is **Leiden community detection** on an exact cosine k-NN graph (`clustering.algorithm: leiden`, seeded → deterministic); K-means remains as `kmeans` fallback. Cluster ids are **stable across re-clustering** (Jaccard member-overlap matching against the previous state, fresh ids minted from a persisted counter) but NOT contiguous — treat as opaque. One derived parent hierarchy level (`parent_id`/`parent_clusters`) when >6 clusters. Labels via cross-cluster TF-IDF with bigrams and smoothed IDF; each cluster stores a `representative` doc. Zero-norm docs land in `unclustered`.
- **Topics (custom clusters):** Collection Topics are user-defined via `clustering.custom` in YAML
  (`name` + optional `description` + optional `seeds` + optional `threshold`) or
  `MDVDB_CUSTOM_CLUSTERS`. Shard-local Topics use the same shape under
  `shards.<id>.topics` and never inherit. Centroid =
  normalize(0.6·embed("name: description") + 0.4·mean(normalized seed embeddings)). Assignment is
  **multi-label**: a doc joins every topic with cosine ≥ max(topic.threshold,
  `clustering.topics.min_similarity` [default 0.30]); docs matching nothing go to the explicit
  **Unassigned** bucket; per-membership similarity scores are persisted. Fingerprints cover
  definitions, floor, model, and dimensions; unchanged compatible centroids are reused.
- **CLI:** `mdvdb clusters add|update|remove|list|unassigned` manage topics in `config.yaml`; `mdvdb clusters --custom` shows computed topics; `mdvdb config set <dotted.key> <value>` writes any YAML config key (used by the Tesseract app GUI).
- **CLI output:** stdout for data (JSON with `--json`, human-readable otherwise), stderr for errors/logs. Search JSON uses wrapped format: `{"results": [...], "query": "...", "total_results": N}`. When `--expand` is used, includes `"graph_context": [...]` with linked-file chunks.

## Key Conventions

- Return `Result<T, Error>` from all library functions — never `unwrap()` in library code
- Pass `Config` as parameter — no global mutable state, no `lazy_static`
- All env var reading and YAML parsing happens in `Config::load()` — other modules receive typed config
- Derive `serde::Serialize` on all API response types for JSON output
- Derive `rkyv::Archive`/`Serialize`/`Deserialize` on all types stored in the index
- Use `tracing::info!`/`debug!`/`error!` for logging, never `println!` in library code
- Keep clippy clean — `cargo clippy --all-targets` must pass with zero warnings

## Testing Requirements

**Every change must have automated tests. No exceptions.**

- Every new feature, bug fix, or behavioral change MUST include tests that verify the change works
- Every existing feature that is modified MUST have its existing tests updated if behavior changes, and new tests added for new behavior
- `cargo test` must pass with zero failures before any change is considered complete
- Unit tests go in `#[cfg(test)] mod tests` blocks in the source file
- Integration tests go in `tests/` — one file per module (e.g., `tests/search_test.rs` for the search engine)
- CLI tests use `std::process::Command` with `env!("CARGO_BIN_EXE_mdvdb")` and validate JSON output structure
- API tests use `mock_config()` with `EmbeddingProviderType::Mock` (8 dimensions) — no API keys needed
- Use `tempfile::TempDir` for filesystem isolation in all tests that touch files
- Do NOT skip writing tests to save time. Untested code is unfinished code.

## Public API (lib.rs)

The `MarkdownVdb` struct is the main entry point:

```rust
MarkdownVdb::open(root)                    // Open with auto-loaded config
MarkdownVdb::open_with_config(root, cfg)   // Open with explicit config
MarkdownVdb::init(path)                    // Create .markdownvdb config file

vdb.ingest(options)     // Index markdown files (full or incremental)
vdb.search(query)       // Search with filters, decay, graph expansion → SearchResponse
vdb.preview(reindex, file) // Dry-run: what would ingest do
vdb.info(path)          // Vault/folder stats + full-reindex token estimate
vdb.status()            // Index stats (doc/chunk/vector counts)
vdb.schema()            // Inferred metadata schema
vdb.clusters()          // Document clusters with labels
vdb.custom_clusters()   // User-defined topics with multi-label assignments + scores
vdb.topic_unassigned()  // Documents matching no topic (Unassigned bucket)
vdb.file_tree()         // File tree with sync status
vdb.get_document(path)  // Single document info + frontmatter + modified_at
vdb.get_document_populated(path) // + resolved relations map + referenced_by (reverse lookups)
vdb.links(path)         // Outgoing + incoming links for a file
vdb.links_neighborhood(path, depth) // Multi-hop link tree (depth 1-3)
vdb.orphans()           // Files with no links
vdb.doctor()            // Diagnostic checks
vdb.watch(cancel)       // File watcher with CancellationToken
vdb.config()            // Access current config
```

Project-local Shards are managed through `ShardStore::new(root)` with
`list/get/resolve_path/add/update/remove/retarget`; local Topic CRUD operates on the same locked
raw-YAML entry. Shard-aware analysis methods mirror cluster, Topic, unassigned, standard graph,
compact graph, and chunk graph queries while leaving Collection methods unchanged.

Key re-exports: `Config`, `SearchQuery`, `SearchResult`, `SearchResultFile`, `SearchResponse`, `GraphContextItem`, `GraphAnalysisInfo`, `GraphAnalysisContext`, `ClusterAnalysisStatus`, `TopicAnalysisStatus`, `MetadataFilter`, `Schema`, `SchemaField`, `FieldType`, `ClusterInfo`, `ClusterState`, `CustomClusterDef`, `CustomClusterInfo`, `CustomClusterState`, `IndexStatus`, `IngestOptions`, `IngestResult`, `VaultInfo`, `SyncBreakdown`, `SearchMode`, `FileTree`, `FileTreeNode`, `FileState`, `LinkGraph`, `LinkEntry`, `ResolvedLink`, `OrphanFile`, `NeighborhoodNode`, `NeighborhoodResult`, `RelationValue`, `ReferencedBy`, `RelationContext`, `FrontmatterLink`, `ShardDefinition`, `ShardInfo`, `ShardList`, `ShardMutation`, `ShardTopicMutation`, `ShardStore`.

## Development Workflow

```bash
cargo test               # Run all 612 tests
cargo clippy --all-targets  # Lint (must be clean)
cargo build --release    # Release build
cargo run -- ingest      # Test ingest locally
cargo run -- search "query" --json  # Test search
```

## Technology Stack

| Layer | Crate | Purpose |
|---|---|---|
| Runtime | `tokio` | Async I/O for embeddings, file watching |
| CLI | `clap` | Derive-based subcommands, completions |
| CLI output | `colored` + `indicatif` | Colored terminal output, progress spinners |
| Markdown | `pulldown-cmark` | Streaming heading-aware parsing + link extraction |
| Frontmatter | `serde_yml` | Dynamic YAML → JSON metadata |
| Tokenizer | `tiktoken-rs` | Accurate token counting for chunks |
| Embeddings | `reqwest` | HTTP client for OpenAI/Ollama APIs |
| Vectors | `usearch` | Sub-ms HNSW nearest neighbor search |
| Full-text search | `tantivy` | BM25 lexical search engine |
| Serialization | `rkyv` | Zero-copy deserialization from mmap |
| Memory mapping | `memmap2` | On-demand index loading via OS page cache |
| File watching | `notify` + `notify-debouncer-full` | Cross-platform FS events + debouncing |
| Concurrency | `parking_lot` | Fast RwLock for read-heavy workloads |
| Clustering | `leiden-rs` + `linfa-clustering` | Leiden community detection (default) + seeded K-means fallback |
| Async streams | `futures` | Concurrent batch embedding (buffer_unordered) |
| File scanning | `ignore` | Gitignore-native directory traversal |
| Hashing | `sha2` | Content change detection (SHA-256) |
| Config | `serde_yml` + `dotenvy` | YAML `.markdownvdb/config.yaml` config + `.env` secrets |
| Errors | `thiserror` / `anyhow` | Typed lib errors, ergonomic CLI errors |
| Serialization | `serde` + `serde_json` | JSON output, request/response bodies |
| Logging | `tracing` + `tracing-subscriber` | Structured, async-aware, spans |

## PRD Reference

NEVER EVER REMOVE ANY PRDS.

Full specifications for all 18 phases live in `docs/prds/`. These document the design intent and acceptance criteria for each subsystem.

| Phase | PRD | Summary |
|---|---|---|
| 1 | `phase-1-foundation-config.md` | Cargo project, config, errors, logging |
| 2 | `phase-2-markdown-parsing.md` | File discovery, ignore rules, frontmatter, headings, SHA-256 |
| 3 | `phase-3-chunking-engine.md` | Heading split, token size guard, overlap, chunk metadata |
| 4 | `phase-4-embedding-providers.md` | Provider trait, OpenAI, Ollama, batch, mock |
| 5 | `phase-5-index-storage.md` | Index file format, rkyv, usearch, memmap, RwLock |
| 6 | `phase-6-semantic-search.md` | Query pipeline, filters, section-level results |
| 7 | `phase-7-metadata-schema.md` | Auto-infer, overlay YAML, schema introspection |
| 8 | `phase-8-file-watching.md` | FS watcher, debounce, incremental re-index, ingest pipeline |
| 9 | `phase-9-clustering.md` | K-means, nearest-centroid, rebalance, keyword labels |
| 10 | `phase-10-cli-library.md` | CLI subcommands, JSON output, MarkdownVdb library API |
| 11 | `phase-11-environment-vars-and-config.md` | `.env` fallback config, priority chain |
| 12 | `phase-12-cli-great.md` | CLI polish: colors, score bars, spinners, humanized output |
| 13 | `phase-13-file-tree-path-scoped-search.md` | `mdvdb tree` command, `--path` scoped search |
| 14 | `phase-14-hybrid-search.md` | BM25 lexical search (Tantivy), RRF fusion, hybrid/semantic/lexical modes |
| 15 | `phase-15-link-graph.md` | Link extraction, backlinks, orphans, link-aware search boost |
| 16 | `phase-16-settings-in-user-location.md` | User-level config at `~/.mdvdb/config` |
| 17 | `phase-17-interactive-ingest-progress.md` | Rich progress display, `--preview`, `--reindex`, Ctrl+C cancellation |
| 18 | `phase-18-time-decay.md` | Optional time-based decay for search scores (exponential half-life) |
| 21 | *(spec)* | Multi-hop graph traversal: BFS link boost, graph context expansion, deep neighborhood |
| 24 | `phase-24-mdvdbignore.md` | `.mdvdbignore` file support (`.gitignore` syntax, index-only exclusions) |
| 30 | `phase-30-leiden-clustering-and-topics.md` | Leiden auto-clustering (stable ids, hierarchy, seeded determinism) + multi-label topics (descriptions, thresholds, Unassigned, fingerprint). Supersedes semantics of phases 9 & 27; app counterpart: app repo `phase-40-topics-and-graph-coloring.md` |
| 31 | `phase-31-frontmatter-relations.md` | Frontmatter relations (wiki-link foreign keys): `FieldType::Relation` + overlay `target:`, field-tagged link-graph edges, `--populate` (RelationValue + referenced_by), relation-aware filters, doctor Relations check. App counterpart: app repo `phase-42-frontmatter-relations.md` |
| 32 | `phase-32-vault-info-command.md` | Read-only whole-vault/folder statistics, sync breakdown, reindex cost estimate, edge-aware status, and doctor index invariant fix. App counterpart: app repo `phase-45-collection-info-modal.md` |
| 33 | `phase-33-named-shards.md` | Project-local named recursive folder lenses over one shared Collection index; CRUD, `--shard`, segment-safe scoping, config locking, and Tesseract tree selection. App counterpart: `app/docs/prds/phase-47-named-shards.md` |
