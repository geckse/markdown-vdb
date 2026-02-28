# Markdown Vector Database (mdvdb)
(pronounced /ˌɛm di ˌvi di ˈbi/)

![language](https://img.shields.io/badge/language-Rust-b7410e)
![license](https://img.shields.io/badge/license-MIT-green)
![version](https://img.shields.io/badge/version-0.1.0-blue)
![tests](https://img.shields.io/badge/tests-524%20passing-brightgreen)
![clippy](https://img.shields.io/badge/clippy-0%20warnings-brightgreen)

A filesystem-native vector database built around Markdown files. 
Zero infrastructure — no servers, no containers, everything lives on disk.

Designed for AI agents that need fast semantic search over local knowledge bases.

## Built For

| Use Case | What mdvdb gives you | Workflow |
|---|---|---|
| **Knowledge Bases** | Semantic search across docs, wikis, and runbooks. Section-level results link straight to the relevant heading — not just the file. Frontmatter filters let you scope queries by tag, status, or any custom field. | `mdvdb ingest` → `mdvdb search "deploy" --filter status=published` |
| **Agent Memory** | Two-layer memory model: daily logs (append-only) + curated topic files, connected by wikilinks and standard links. Frontmatter fields (`type`, `tags`, `status`, `confidence`, `source`) make memories filterable. Link graph lets agents traverse context chains via `links` / `backlinks`, and `orphans` surfaces disconnected notes. `--boost-links` re-ranks results using the agent's own cross-references. `--decay` applies exponential recency weighting — old logs fade naturally while actively maintained notes stay prominent, no manual archiving needed. Single-file ingest (`--file`) for near-instant indexing after writes. See the full [Agent Memory Graph guide](docs/guides/agent-memory-graph.md). | `mdvdb ingest` → `mdvdb search "auth" --decay --boost-links --filter type=topic` → `mdvdb links topics/auth.md` → `mdvdb orphans` |
| **Documentation Sites** | Index your docs repo and expose search via the library API. Auto-clustering surfaces topic groups without manual tagging. File watching keeps the index current as writers push changes. | `mdvdb watch` → `mdvdb clusters --json` → `mdvdb search "getting started"` |
| **Personal Zettelkasten** | Search your slip-box by meaning instead of exact keywords. Works with Obsidian vaults, Logseq graphs, or plain folders — anything that's `.md` files on disk. Non-destructive: never touches your notes. | `mdvdb ingest` → `mdvdb search "emergence in complex systems"` → `mdvdb links slip/note.md` |
| **RAG Pipelines** | Drop-in retrieval layer for retrieval-augmented generation. JSON output (`--json`) pipes directly into your LLM toolchain. Pluggable embeddings let you match the same model your generator uses. | `mdvdb ingest` → `mdvdb search "context" --json \| jq` |
| **Research & Literature Notes** | Filter by frontmatter fields like `author`, `year`, or `topic` while searching semantically. Clusters reveal thematic groupings across hundreds of papers or reading notes without manual curation. | `mdvdb ingest` → `mdvdb search "attention mechanism" --filter year=2024` → `mdvdb clusters` |

**Guides:** [Agent Memory](docs/guides/agent-memory.md) · [Agent Memory Graph](docs/guides/agent-memory-graph.md)

## Features

- **Markdown-first** — `.md` files are the source of truth
- **Semantic search** — find content by meaning, not keywords
- **Section-level results** — returns the specific heading/section that matched, not just the file
- **Pluggable embeddings** — OpenAI, Ollama, or any custom endpoint
- **Single index file** — portable, memory-mapped, sub-100ms queries
- **File watching** — automatic re-indexing on changes
- **Metadata filtering** — combine semantic search with frontmatter filters
- **Auto-clustering** — browse topics before searching
- **Non-destructive** — never modifies your markdown files

## Quick Start

```bash
# Initialize config
mdvdb init

# Index your markdown files
mdvdb ingest

# Search
mdvdb search "how to deploy to production"

# Search with filters
mdvdb search "authentication" --filter status=published --limit 5

# Check index health
mdvdb status

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
```

All settings have sensible defaults. Shell environment variables override the config file.

See [PROJECT.md](PROJECT.md) for the full configuration reference.

## CLI Commands

| Command | Description |
|---|---|
| `mdvdb search <query>` | Semantic search with optional filters |
| `mdvdb ingest` | Index or re-index markdown files |
| `mdvdb status` | Show index health and stats |
| `mdvdb schema` | List available metadata fields and types |
| `mdvdb clusters` | Browse auto-generated topic clusters |
| `mdvdb get <path>` | Retrieve a specific document's content and metadata |
| `mdvdb watch` | Watch for file changes and re-index automatically |
| `mdvdb init` | Create a default config file |

All commands support `--json` for machine-readable output.

## Library Usage

```rust
use mdvdb::MarkdownVdb;
use mdvdb::SearchQuery;

let vdb = MarkdownVdb::open(".").await?;

let results = vdb.search(
    SearchQuery::new("deployment guide")
        .with_limit(10)
        .with_min_score(0.7)
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
4. **Embed** — generate vectors via OpenAI/Ollama (batched, with content-hash caching)
5. **Index** — store in a single memory-mapped file (usearch HNSW + rkyv metadata)
6. **Search** — embed query → nearest neighbors → metadata filter → ranked results

## Architecture

```
Markdown files → Discovery → Parsing → Chunking → Embedding → Index
                                                                 ↓
                                        Query → Embed → HNSW search → Filter → Results
```

**Index format:** single binary file — `[64B header][rkyv metadata][usearch HNSW]` — memory-mapped for instant loads.

**Key dependencies:** `usearch` (HNSW vectors), `rkyv` (zero-copy serde), `memmap2` (memory mapping), `tiktoken-rs` (tokenization), `pulldown-cmark` (markdown parsing), `linfa` (clustering).

## Project Status

All core subsystems implemented and tested. Active development continues on advanced features (link graph analysis, full-text search hybrid mode, interactive progress display).

## License

MIT
