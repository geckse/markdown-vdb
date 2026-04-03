---
title: "Chunking Strategies"
tags: [chunking, rag, indexing, text-processing]
category: knowledge-graph
author: "Alex Park"
status: published
---

# Chunking Strategies

Chunking determines how documents are split into pieces for [embedding](vector-embeddings.md) and retrieval. The chunk is the atomic unit of [RAG](rag-overview.md) — what you embed is what you retrieve.

## Strategy Comparison

| Strategy                 | Pros                         | Cons                                 | Best For              |
| ------------------------ | ---------------------------- | ------------------------------------ | --------------------- |
| Fixed-size (token count) | Simple, predictable          | Splits mid-sentence, loses structure | Unstructured text     |
| Heading-based            | Preserves document structure | Uneven sizes, headings vary widely   | Technical docs, wikis |
| Semantic (paragraph)     | Natural boundaries           | Still misses hierarchical context    | Prose, articles       |
| Recursive (hybrid)       | Balanced size + structure    | More complex to implement            | General purpose       |

## Our Approach: Heading-Based + Size Guard

We use **heading-based splitting** as the primary strategy, with a **token size guard** as secondary:

1. Split at heading boundaries (`#`, `##`, `###`, etc.)
2. Each chunk inherits its heading hierarchy as metadata (e.g., `Architecture > Auth Service > Token Flow`)
3. If a chunk exceeds the token limit (default: 512 tokens), split it further at paragraph boundaries
4. Each chunk gets a deterministic ID: `"file-path#chunk-index"`

This works well for our knowledge base because our documents are structured with clear headings. For example, [the architecture doc](../docs/architecture.md) naturally splits into service-level chunks, and [the API reference](../docs/api-reference.md) splits into endpoint-level chunks.

## Chunk Metadata

Each chunk carries:

- **File path** — which document it came from
- **Heading breadcrumb** — the heading hierarchy (e.g., `Error Handling > Retry Strategy`)
- **Frontmatter** — inherited from the parent document (title, tags, author, etc.)
- **Token count** — for budget management during [Graph RAG](graph-rag-architecture.md) context assembly
- **Content hash** — SHA-256 for change detection during [ingestion](rag-overview.md)

## Overlap

We don't use chunk overlap (where adjacent chunks share some text). Our heading-based approach preserves natural boundaries, and the heading breadcrumb provides sufficient context. Overlap is more useful for fixed-size chunking where context loss at split points is a real problem.

## Impact on Retrieval

Chunk size directly affects retrieval quality:

- **Too small** (< 100 tokens) — chunks lack context, retrieval returns fragments
- **Too large** (> 1000 tokens) — chunks are diluted, irrelevant content pollutes the [embedding](vector-embeddings.md)
- **Right size** (200-500 tokens) — focused enough for precise retrieval, large enough for context

Our 512-token guard hits the sweet spot for the [embedding models](vector-embeddings.md#embedding-models) we use (all support 8K+ context, but shorter chunks embed more precisely).

## Related

- [Vector Embeddings](vector-embeddings.md) — what happens after chunking
- [RAG Overview](rag-overview.md) — the retrieval pattern that depends on good chunking
- [Graph RAG Architecture](graph-rag-architecture.md) — how chunks participate in graph expansion
- [Evaluation Framework](evaluation-framework.md) — measuring chunk quality via end-to-end retrieval metrics
