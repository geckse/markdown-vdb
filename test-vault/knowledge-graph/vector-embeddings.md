---
title: "Vector Embeddings"
tags: [embeddings, vectors, ai, search]
category: knowledge-graph
author: "Priya Sharma"
status: published
---

# Vector Embeddings

Vector embeddings are dense numerical representations of text that capture semantic meaning. Similar texts produce vectors that are close together in the embedding space, enabling similarity search.

## Embedding Models

| Model | Dimensions | Context | Strengths |
|---|---|---|---|
| OpenAI text-embedding-3-small | 1536 | 8191 tokens | Good balance of quality and cost |
| OpenAI text-embedding-3-large | 3072 | 8191 tokens | Highest quality, higher cost |
| Ollama nomic-embed-text | 768 | 8192 tokens | Runs locally, no API costs |
| Ollama mxbai-embed-large | 1024 | 512 tokens | Fast local inference |

We currently use OpenAI text-embedding-3-small in production and Ollama nomic-embed-text for local development. See [our architecture](../docs/architecture.md#data-layer) for how this fits into the search service.

## How Embeddings Enable RAG

In our [RAG pipeline](rag-overview.md), embeddings serve as the bridge between natural language queries and stored documents:

1. At **ingest time**, each [chunk](chunking-strategies.md) is embedded and stored in an HNSW index
2. At **query time**, the query is embedded with the same model
3. **Nearest neighbor search** finds the chunks whose embeddings are closest to the query

This is why [chunking strategy](chunking-strategies.md) matters — the unit of embedding is the unit of retrieval.

## Distance Metrics

- **Cosine similarity** — measures angle between vectors, ignoring magnitude. Best for text.
- **Euclidean (L2)** — measures straight-line distance. Sensitive to vector magnitude.
- **Inner product** — equivalent to cosine for normalized vectors. Our HNSW index uses this.

## Content-Hash Deduplication

We skip re-embedding unchanged documents using SHA-256 content hashing. When a file's hash matches what's in the index, we skip the embedding API call entirely. This makes incremental [ingestion](../docs/deployment.md#database-migrations) fast — only changed files pay the embedding cost.

## Dimensionality and Storage

Higher-dimensional embeddings capture more nuance but cost more storage and compute:

- 1536-dim × 4 bytes × 10,000 chunks = ~58 MB
- 3072-dim × 4 bytes × 10,000 chunks = ~117 MB

Our [index format](../docs/architecture.md) uses memory-mapped HNSW, so only the pages accessed during a query are loaded into RAM.

## Related

- [RAG Overview](rag-overview.md) — the retrieval pattern that uses embeddings
- [Chunking Strategies](chunking-strategies.md) — what gets embedded
- [Graph RAG Architecture](graph-rag-architecture.md) — combining embeddings with graph structure
- [Evaluation Framework](evaluation-framework.md) — measuring embedding quality
