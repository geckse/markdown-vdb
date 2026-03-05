---
title: "Retrieval-Augmented Generation (RAG)"
tags: [rag, ai, llm, retrieval]
category: knowledge-graph
author: "Priya Sharma"
status: published
---

# Retrieval-Augmented Generation (RAG)

RAG is the pattern of retrieving relevant documents from a knowledge base and feeding them as context to an LLM before generating a response. Instead of relying solely on the model's training data, RAG grounds answers in your actual documents.

## How It Works

1. **Query** — user asks a question
2. **Retrieve** — find relevant chunks via [vector embeddings](vector-embeddings.md) similarity search
3. **Augment** — prepend retrieved chunks to the LLM prompt as context
4. **Generate** — LLM produces an answer grounded in the retrieved documents

## Why RAG Matters

- **Accuracy** — answers are grounded in real documents, not hallucinated
- **Freshness** — new documents are available immediately after indexing, no retraining needed
- **Auditability** — every answer can cite its source documents
- **Cost** — cheaper than fine-tuning; you just maintain an index

## Limitations of Basic RAG

Basic RAG retrieves chunks independently based on vector similarity. This misses:

- **Multi-hop reasoning** — answering "what is the deployment process for the auth service?" requires combining info from [architecture](../docs/architecture.md), [deployment](../docs/deployment.md), and [security practices](../guides/security-best-practices.md)
- **Structural context** — a chunk about "token validation" is more useful when you know it's part of the auth service, which connects to the API gateway
- **Relationship awareness** — basic RAG doesn't know that [error handling](../docs/error-handling.md) patterns apply across all services

This is why we need [Graph RAG](graph-rag-architecture.md) — to follow links and pull in structurally related context, not just semantically similar chunks.

## Implementation in Our Stack

Our [search engine](../docs/api-reference.md#semantic-search) already supports semantic search via vector embeddings. The next step is adding graph traversal on top. See the [Graph RAG Architecture](graph-rag-architecture.md) doc for the design.

## Related

- [Vector Embeddings](vector-embeddings.md) — the retrieval mechanism
- [Graph RAG Architecture](graph-rag-architecture.md) — the evolved approach
- [Chunking Strategies](chunking-strategies.md) — how documents get split for indexing
- [Evaluation Framework](evaluation-framework.md) — how we measure RAG quality
