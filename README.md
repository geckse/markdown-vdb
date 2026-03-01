# Markdown Vector Database (mdvdb)
(pronounced /ˌɛm di ˌvi di ˈbi/)

![language](https://img.shields.io/badge/language-Rust-b7410e)
![license](https://img.shields.io/badge/license-MIT-green)
![version](https://img.shields.io/badge/version-0.1.0-blue)
![tests](https://img.shields.io/badge/tests-579%20passing-brightgreen)
![clippy](https://img.shields.io/badge/clippy-0%20warnings-brightgreen)

A filesystem-native vector database built around Markdown files.
Zero infrastructure — no servers, no containers, everything lives on disk.

Three search modes out of the box: **hybrid** (semantic + lexical, fused via RRF), **semantic** (embedding similarity), and **lexical** (BM25 full-text). Designed for AI agents that need fast, flexible search over local knowledge bases.

## Built For

| Use Case | What mdvdb gives you | Workflow |
|---|---|---|
| **Knowledge Bases** | Hybrid search across docs, wikis, and runbooks. Section-level results link straight to the relevant heading — not just the file. Frontmatter filters let you scope queries by tag, status, or any custom field. | `mdvdb ingest` → `mdvdb search "deploy" --filter status=published` |
| **Agent Memory** | Two-layer memory model: daily logs (append-only) + curated topic files, connected by wikilinks and standard links. Frontmatter fields (`type`, `tags`, `status`, `confidence`, `source`) make memories filterable. Link graph lets agents traverse context chains via `links` / `backlinks`, and `orphans` surfaces disconnected notes. `--boost-links` re-ranks results using the agent's own cross-references. `--decay` applies exponential recency weighting — old logs fade naturally while actively maintained notes stay prominent, no manual archiving needed. Single-file ingest (`--file`) for near-instant indexing after writes. See the full [Agent Memory Graph guide](docs/guides/agent-memory-graph.md). | `mdvdb ingest` → `mdvdb search "auth" --decay --boost-links --filter type=topic` → `mdvdb links topics/auth.md` → `mdvdb orphans` |
| **Documentation Sites** | Index your docs repo and expose search via the library API. Auto-clustering surfaces topic groups without manual tagging. File watching keeps the index current as writers push changes. | `mdvdb watch` → `mdvdb clusters --json` → `mdvdb search "getting started"` |
| **Personal Zettelkasten** | Search your slip-box by meaning instead of exact keywords. Works with Obsidian vaults, Logseq graphs, or plain folders — anything that's `.md` files on disk. Non-destructive: never touches your notes. | `mdvdb ingest` → `mdvdb search "emergence in complex systems"` → `mdvdb links slip/note.md` |
| **RAG Pipelines** | Drop-in retrieval layer for retrieval-augmented generation. JSON output (`--json`) pipes directly into your LLM toolchain. Pluggable embeddings let you match the same model your generator uses. Switch to `--lexical` when you don't need embedding overhead. | `mdvdb ingest` → `mdvdb search "context" --json \| jq` |
| **Research & Literature Notes** | Filter by frontmatter fields like `author`, `year`, or `topic` while searching semantically. Clusters reveal thematic groupings across hundreds of papers or reading notes without manual curation. | `mdvdb ingest` → `mdvdb search "attention mechanism" --filter year=2024` → `mdvdb clusters` |

**Guides:** [Agent Memory](docs/guides/agent-memory.md) · [Agent Memory Graph](docs/guides/agent-memory-graph.md)

## Features

- **Three search modes** — hybrid (semantic + BM25 via RRF fusion), semantic, and lexical — switch with `--mode` or `--semantic`/`--lexical` flags
- **Section-level results** — returns the specific heading/section that matched, not just the file
- **Pluggable embeddings** — OpenAI, Ollama, or any custom endpoint
- **Single index file** — portable, memory-mapped, sub-100ms queries
- **Link graph** — wikilinks and standard markdown links tracked in the index; `links`, `backlinks`, `orphans` commands; `--boost-links` re-ranks search results
- **Time decay** — `--decay` applies exponential recency weighting with configurable half-life
- **File watching** — automatic re-indexing on changes
- **Metadata filtering** — combine any search mode with frontmatter filters
- **Auto-clustering** — K-means topic clusters with TF-IDF keyword labels
- **Path-scoped search** — `--path` restricts results to a directory subtree
- **File tree** — `mdvdb tree` shows sync status of every file at a glance
- **Diagnostics** — `mdvdb doctor` checks config, provider connectivity, and index health
- **Preview mode** — `mdvdb ingest --preview` shows what would change without touching the index
- **Non-destructive** — never modifies your markdown files

## Quick Start

```bash
# Initialize config
mdvdb init

# Index your markdown files
mdvdb ingest

# Hybrid search (default — semantic + lexical fused via RRF)
mdvdb search "how to deploy to production"

# Semantic only
mdvdb search "how to deploy to production" --semantic

# Lexical only (no embedding API call needed)
mdvdb search "deploy" --lexical

# Search with filters and path scope
mdvdb search "authentication" --filter status=published --path docs/ --limit 5

# Time-decayed search (favor recent files)
mdvdb search "auth" --decay --decay-half-life 30

# Check index health
mdvdb doctor

# Watch for changes
mdvdb watch
```

## Installation

```bash
# Build from source
git clone https://github.com/gecko/markdown-vdb.git
cd markdown-vdb
cargo install --path .
```

Requires Rust 1.70+.

## Configuration

Create a `.markdownvdb` file in your project root (or run `mdvdb init`):

```env
# Embedding provider: openai, ollama, custom
MDVDB_EMBEDDING_PROVIDER=openai
MDVDB_EMBEDDING_MODEL=text-embedding-3-small
OPENAI_API_KEY=sk-...

# Directories to index (comma-separated)
MDVDB_SOURCE_DIRS=docs,notes,wiki

# Chunking
MDVDB_CHUNK_MAX_TOKENS=512

# Search defaults
MDVDB_SEARCH_MODE=hybrid
MDVDB_TIME_DECAY=false
MDVDB_DECAY_HALF_LIFE_DAYS=30
```

Shared credentials can go in `~/.mdvdb/config` so your API key works across all projects without repeating it in each `.markdownvdb` file.

Config resolution order: shell env → `.markdownvdb/.config` → `.markdownvdb` → `.env` → `~/.mdvdb/config` → defaults.

See [PROJECT.md](PROJECT.md) for the full configuration reference.

## CLI Commands

| Command | Description |
|---|---|
| `mdvdb search <query>` | Search with hybrid, semantic, or lexical mode |
| `mdvdb ingest` | Index or re-index markdown files |
| `mdvdb status` | Show index health and stats |
| `mdvdb schema` | List available metadata fields and types |
| `mdvdb clusters` | Browse auto-generated topic clusters |
| `mdvdb tree` | Show file tree with sync status indicators |
| `mdvdb get <path>` | Retrieve a specific document's metadata and frontmatter |
| `mdvdb links <path>` | Show outgoing links from a file |
| `mdvdb backlinks <path>` | Show incoming links pointing to a file |
| `mdvdb orphans` | Find files with no inbound or outbound links |
| `mdvdb doctor` | Run diagnostic checks on config, provider, and index |
| `mdvdb watch` | Watch for file changes and re-index automatically |
| `mdvdb config` | Show resolved configuration |
| `mdvdb init` | Create a default config file |

All commands support `--json` for machine-readable output.

## Library Usage

```rust
use mdvdb::{MarkdownVdb, SearchQuery, SearchMode};

let vdb = MarkdownVdb::open(".").await?;

// Hybrid search (default)
let results = vdb.search(
    SearchQuery::new("deployment guide")
        .with_limit(10)
        .with_min_score(0.7)
).await?;

// Lexical-only search (no embedding call)
let results = vdb.search(
    SearchQuery::new("deploy")
        .with_mode(SearchMode::Lexical)
).await?;

// Semantic search with time decay and link boosting
let results = vdb.search(
    SearchQuery::new("authentication")
        .with_mode(SearchMode::Semantic)
        .with_decay(true)
        .with_boost_links(true)
        .with_filter("status", "published")
).await?;

for result in results {
    println!("{} (score: {:.2})", result.file.path, result.score);
    println!("  {}", result.chunk.heading_hierarchy.join(" > "));
}
```

## How It Works

1. **Scan** — recursively find `.md` files (respects `.gitignore`)
2. **Parse** — extract frontmatter, headings, and body content
3. **Chunk** — split by headings, with token-limit size guard
4. **Embed** — generate vectors via OpenAI/Ollama (batched, concurrent, with content-hash skip)
5. **Index** — store in a single memory-mapped file (usearch HNSW + rkyv metadata) plus Tantivy BM25 segments
6. **Search** — hybrid (embed query → HNSW + BM25 → RRF fusion), semantic (HNSW only), or lexical (BM25 only) → metadata filter → link boost → time decay → ranked results

## Architecture

```
Markdown files → Discovery → Parsing → Chunking → Embedding → Index (HNSW + BM25)
                                                                       ↓
                        Query → [Embed] → HNSW ─┐
                                          BM25 ──┤→ RRF Fusion → Filter → Decay → Results
                                     Link Graph ─┘
```

**Index format:** `[64B header][rkyv metadata][usearch HNSW]` (memory-mapped) + `fts/` directory (Tantivy BM25 segments).

**Key dependencies:** `usearch` (HNSW vectors), `tantivy` (BM25 lexical search), `rkyv` (zero-copy serde), `memmap2` (memory mapping), `tiktoken-rs` (tokenization), `pulldown-cmark` (markdown parsing), `linfa` (K-means clustering).

## Project Status

Development. Wait until 0.2.0 is released for actual use.

## License

MIT
