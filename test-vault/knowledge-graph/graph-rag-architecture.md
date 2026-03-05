---
title: "Graph RAG Architecture"
tags: [graph-rag, architecture, ai, knowledge-graph]
category: knowledge-graph
author: "Priya Sharma"
status: published
---

# Graph RAG Architecture

Graph RAG extends traditional [RAG](rag-overview.md) by using the link structure between documents to retrieve richer, more connected context. Instead of only returning the top-K most similar chunks, Graph RAG also traverses the knowledge graph to pull in structurally related documents.

## The Problem with Flat Retrieval

Consider the question: *"How should I handle authentication errors in production?"*

Basic RAG with [vector embeddings](vector-embeddings.md) might return:
1. A chunk from [error handling patterns](../docs/error-handling.md) about error response format
2. A chunk from [security best practices](../guides/security-best-practices.md) about token handling

But the complete answer requires connecting:
- [Auth service architecture](../docs/architecture.md#auth-service) — how tokens work
- [Error handling](../docs/error-handling.md) — how errors propagate between services
- [Incident response](../runbooks/incident-response.md#auth-service-down) — what to do when auth fails in production
- [API reference](../docs/api-reference.md#authentication) — the actual auth endpoints

These documents are all *linked* to each other. Graph RAG exploits these links.

## Architecture

```
Query
  │
  ▼
┌─────────────────────┐
│  Vector Search       │  ← Find seed nodes via embedding similarity
│  (top-K chunks)      │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  Graph Expansion     │  ← Follow outgoing + incoming links (depth=1 or 2)
│  (neighbor nodes)    │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  Re-rank & Filter    │  ← Score combined results by relevance + graph distance
│  (final context)     │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  LLM Generation      │  ← Generate answer from enriched context
└─────────────────────┘
```

## Graph Expansion Strategy

Given seed documents from vector search:

1. **Depth-1 expansion** — fetch all documents that the seed links to (outgoing) and all documents that link to the seed (incoming/backlinks)
2. **Edge weighting** — links in headings or introductions are weighted higher than links buried in lists
3. **Deduplication** — if a neighbor was already retrieved by vector search, don't double-count it
4. **Budget** — limit total context to N tokens; graph neighbors fill remaining budget after seed chunks

This maps directly to our existing [link graph](../docs/api-reference.md) infrastructure — the `links` and `backlinks` commands already return exactly the edges we need.

## Scoring

Final relevance score combines:

- **Semantic similarity** (cosine distance from [embeddings](vector-embeddings.md)) — how close the content is to the query
- **Graph distance** — direct links score higher than 2-hop neighbors
- **Link density** — documents with many incoming links (high PageRank) get a boost
- **Recency** — newer documents score slightly higher (time decay)

This is similar to how [our search engine](../docs/api-reference.md#search) already combines semantic and lexical signals via [RRF fusion](rag-overview.md).

## Example: Multi-Hop Query

**Query:** "What happens when a user's JWT expires during a deployment?"

**Step 1 — Vector search** finds:
- [Architecture: Token Flow](../docs/architecture.md#token-flow) (seed)
- [Deployment: Rolling Updates](../docs/deployment.md#rolling-updates) (seed)

**Step 2 — Graph expansion** adds:
- [Error Handling: Error Propagation](../docs/error-handling.md#error-propagation-between-services) (linked from architecture)
- [Security: Token Handling](../guides/security-best-practices.md#token-handling) (linked from architecture)
- [Incident Response: Auth Service Down](../runbooks/incident-response.md#auth-service-down) (links to architecture)

**Step 3 — Re-rank** orders by combined score, yielding a context window that spans authentication, deployment, error handling, and incident response — exactly what's needed for a complete answer.

## Implementation Plan

1. Reuse existing [chunking](chunking-strategies.md) and [embedding](vector-embeddings.md) pipeline
2. Build graph expansion on top of existing `LinkGraph` data structure
3. Add a `--graph-depth` parameter to search (default: 1)
4. Measure quality improvement with the [evaluation framework](evaluation-framework.md)

## Related

- [RAG Overview](rag-overview.md) — foundational pattern
- [Vector Embeddings](vector-embeddings.md) — the similarity search layer
- [Chunking Strategies](chunking-strategies.md) — how documents are split
- [Evaluation Framework](evaluation-framework.md) — measuring quality
- [Architecture](../docs/architecture.md) — the platform this runs on
