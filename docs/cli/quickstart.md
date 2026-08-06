---
title: "Quick Start"
description: "Initialize a Markdown collection, index it, and query content and frontmatter"
category: "guides"
---

# Quick Start

This guide creates a local mdvdb collection, configures an embedding provider, indexes Markdown,
and exercises search, frontmatter queries, Relations, computed fields, Shards, and Topics.

This documentation follows the current `main` branch. Some capabilities in the complete walkthrough
may be newer than the latest tagged binary; see [Installation](./installation.md) to choose a tagged
release or install current `main`.

## 1. Initialize a collection

Install `mdvdb` first, then run `init` at the root of a folder containing Markdown files:

```bash
cd my-notes
mdvdb init
```

This creates `.markdownvdb/config.yaml`. The index, full-text segments, and disposable caches also
live under `.markdownvdb/`; your Markdown files remain the source of truth.

## 2. Configure an embedding provider

The generated YAML defaults to OpenAI with automatic dimension detection:

```yaml
# .markdownvdb/config.yaml
embedding:
  provider: openai
  model: text-embedding-3-small
  dimensions: auto
  batch_size: 100
```

Keep credentials out of YAML. Set them in the shell or in `.markdownvdb/.env`:

```dotenv
# .markdownvdb/.env
OPENAI_API_KEY=replace-with-your-key
```

You can also pipe a value already held in your environment into mdvdb's secret writer:

```bash
printf '%s' "$OPENAI_API_KEY" \
  | mdvdb config secret set OPENAI_API_KEY --stdin
```

For a local Ollama model instead:

```bash
ollama pull nomic-embed-text
mdvdb config set embedding.provider ollama
mdvdb config set embedding.model nomic-embed-text
mdvdb config set embedding.dimensions auto
```

OpenAI, OpenRouter, Gemini, Azure OpenAI, AWS Bedrock, Hugging Face, Ollama, and custom
OpenAI-compatible endpoints are supported. Model IDs are provider-native strings; they are not
hard-coded allowlists.

Probe the configured model and inspect the resolved settings:

```bash
mdvdb embedding probe
mdvdb config
mdvdb doctor
```

`dimensions: auto` uses a compatible existing index dimension when possible and otherwise probes
the provider before creating or replacing the vector index.

## 3. Add frontmatter and links

Frontmatter behaves like typed columns over your Markdown files. For example:

```markdown
---
title: Production deployment
kind: guide
status: published
owner: platform
updated: 2026-08-01
---

# Production deployment

See [[runbooks/rollback]] before promoting a release.
```

mdvdb infers field types, treats files as rows in folder collections, and extracts standard
Markdown links and wiki links into its graph.

## 4. Preview and ingest

Preview an ingest without changing the index:

```bash
mdvdb ingest --preview
```

Then build the index:

```bash
mdvdb ingest
```

Ingestion discovers Markdown, parses frontmatter and links, chunks by heading, embeds changed
content, updates BM25 search, and computes graph analysis. Later ingests skip unchanged content.

## 5. Search content and frontmatter

Hybrid search combines semantic and lexical retrieval by default:

```bash
mdvdb search "how do we roll back production?"
```

Choose a mode, return JSON, or combine search with frontmatter filters:

```bash
# Vector similarity only
mdvdb search "release recovery" --semantic

# BM25 only; an existing index can be queried without contacting the provider
mdvdb search "ROLLBACK_FAILED" --lexical

# Frontmatter predicates are ANDed
mdvdb search "deployment" \
  --filter kind=guide \
  --filter status=published \
  --json
```

Use `collection` when you want deterministic table-like access rather than relevance ranking:

```bash
mdvdb collection docs \
  --recursive \
  --filter status=published \
  --sort updated \
  --order desc \
  --limit 20 \
  --json
```

This is the SQL-like frontmatter model: Markdown files are rows, frontmatter keys are typed
columns, `--filter` narrows rows, `--sort`/`--order` order them, and `--limit`/`--offset` paginate
them. Search adds semantic and lexical ranking over the same records.

## 6. Add Relations and computed fields

A whole frontmatter value that points to another Markdown document acts as a Relation:

```markdown
---
title: Invoice 2026-001
status: sent
client: clients/acme.md
subtotal: 1200
tax: 228
---
```

Resolve the related document inline and inspect reverse references:

```bash
mdvdb get invoices/2026-001.md --populate --json
mdvdb search "outstanding invoice" --populate --json
```

Schema overlays can declare relation targets and computed columns in
`.markdownvdb.schema.yml`:

```yaml
scopes:
  invoices:
    fields:
      client:
        field_type: relation
        target: clients
      total:
        field_type: formula
        formula: subtotal + tax
        result_type: number
```

Validate a Formula, then ingest:

```bash
mdvdb modules validate formula \
  --formula 'subtotal + tax' \
  --result-type number
mdvdb ingest
```

Successful Formula, Lookup, and Rollup values are atomically materialized into Markdown
frontmatter. That makes computed values available to later filters, sorting, export, search, and
other computed fields while Markdown remains the value authority.

Useful discovery commands are:

```bash
mdvdb schema --path invoices
mdvdb modules list
mdvdb modules status formula --path invoices
mdvdb modules status lookup_rollup --path invoices
```

See [Relations](./concepts/relations.md) and
[Computed fields](./concepts/computed-fields.md) for the complete overlay contracts.

## 7. Scope work with Shards and Topics

A Shard gives a recursive folder scope a stable name while reusing the collection's index, file
identities, links, and watcher:

```bash
mdvdb shards add research \
  --name "Research" \
  --path notes/research

mdvdb search "evaluation methodology" --shard research
mdvdb collection --shard research --recursive --json
mdvdb graph --shard research --json
```

Automatic Leiden communities are derived from the documents. Topics are user-defined, multi-label
semantic groupings with optional descriptions, seeds, and thresholds:

```bash
mdvdb clusters --shard research add Methods \
  --description "Research methods and evaluation design" \
  --seeds methodology,experiment,evaluation

# Ingest computes the new Topic centroid and assignments
mdvdb ingest

mdvdb clusters --shard research --custom
mdvdb clusters --shard research unassigned
```

Shard Topics belong only to that Shard. Collection, parent-Shard, sibling-Shard, and child-Shard
Topics do not inherit into one another.

## Useful workflows

```bash
# Inspect collection size, sync state, and a reindex estimate
mdvdb info

# Keep the index current
mdvdb watch

# Favor recent agent memory while expanding linked context
mdvdb search "what changed in authentication?" \
  --decay \
  --decay-half-life 30 \
  --boost-links \
  --expand 1 \
  --json

# Check graph health
mdvdb links notes/authentication.md
mdvdb backlinks notes/authentication.md
mdvdb orphans
mdvdb doctor
```

## Quick reference

| Task | Command |
|---|---|
| Initialize project config | `mdvdb init` |
| Inspect resolved config | `mdvdb config` |
| Probe embedding dimensions | `mdvdb embedding probe` |
| Preview ingestion | `mdvdb ingest --preview` |
| Incrementally ingest | `mdvdb ingest` |
| Force a new embedding generation | `mdvdb ingest --reindex` |
| Search | `mdvdb search "query"` |
| Query frontmatter rows | `mdvdb collection [PATH]` |
| Inspect inferred schema | `mdvdb schema` |
| Manage reusable folder scopes | `mdvdb shards ...` |
| Inspect communities / Topics | `mdvdb clusters ...` |
| Watch for changes | `mdvdb watch` |

## Further reading

- [Configuration](./configuration.md) — YAML, secrets, providers, and precedence
- [Command Reference](./commands/index.md) — command index
- [Search Modes](./concepts/search-modes.md) — hybrid, semantic, lexical, and edge retrieval
- [Time Decay](./concepts/time-decay.md) — recency weighting for memory and changing corpora
- [Frontmatter as structured data](./concepts/frontmatter-data.md) — the SQL-like query model and its limits
- [Use-case playbooks](./use-cases/llm-wiki.md) — start with an LLM wiki, AI memory, or knowledge operations
- [Tesseract](./tesseract.md) — optional desktop editing, tables, graphs, and agent tooling
- [Provider transport guide](https://github.com/geckse/markdown-vdb/blob/main/docs/embedding-providers.md) — provider-specific authentication and codecs
