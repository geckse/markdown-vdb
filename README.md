# Markdown Vector Database (mdvdb)
(pronounced /ˌɛm di ˌvi di ˈbi/)

A filesystem-native vector database built around Markdown files. 
Zero infrastructure — no servers, no containers, everything lives on disk.

Designed for AI agents that need fast semantic search over local knowledge bases.

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

> Coming soon — project is under active development.

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

## Project Status

🚧 **Under development** — 

## License

TBD
