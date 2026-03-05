---
title: "RAG Evaluation Framework"
tags: [evaluation, rag, testing, metrics]
category: knowledge-graph
author: "Priya Sharma"
status: draft
---

# RAG Evaluation Framework

How we measure whether our [RAG pipeline](rag-overview.md) is actually returning good answers. Without evaluation, we're tuning parameters blindly.

## Metrics

### Retrieval Metrics

These measure whether the right documents are retrieved, independent of the LLM's answer:

- **Recall@K** — what fraction of relevant documents appear in the top K results?
- **Precision@K** — what fraction of the top K results are relevant?
- **MRR (Mean Reciprocal Rank)** — how high does the first relevant result appear?
- **nDCG** — normalized discounted cumulative gain, accounting for result ordering

### Generation Metrics

These measure the quality of the LLM's answer given the retrieved context:

- **Faithfulness** — does the answer only use information from the retrieved context? (no hallucination)
- **Answer relevance** — does the answer actually address the question?
- **Context utilization** — does the answer use the key information from the retrieved chunks?

## Evaluation Dataset

We maintain a set of question-answer pairs grounded in our knowledge base:

| Question | Expected Sources | Expected Answer Contains |
|---|---|---|
| How does authentication work? | [architecture](../docs/architecture.md), [api-reference](../docs/api-reference.md), [security](../guides/security-best-practices.md) | JWT, RS256, 15-min expiry |
| What happens when the auth service goes down? | [incident-response](../runbooks/incident-response.md), [architecture](../docs/architecture.md), [error-handling](../docs/error-handling.md) | 401 errors, rollback, escalate |
| How do I deploy a new version? | [deployment](../docs/deployment.md), [contributing](../guides/contributing.md) | Helm, rolling update, kubectl |
| What's our retry strategy? | [error-handling](../docs/error-handling.md) | exponential backoff, jitter, 5 attempts |

## Graph RAG vs. Basic RAG Comparison

The key test for [Graph RAG](graph-rag-architecture.md) is multi-hop questions where the answer spans multiple documents:

| Question | Basic RAG Sources | Graph RAG Sources | Improvement |
|---|---|---|---|
| How should I handle JWT expiry during deployment? | architecture, deployment | architecture, deployment, **error-handling**, **incident-response**, **security** | +3 relevant docs |
| What's the full path of a search request? | api-reference, architecture | api-reference, architecture, **error-handling**, **deployment** | +2 relevant docs |

The added documents from graph expansion provide context that basic [vector search](vector-embeddings.md) misses because they're structurally related (linked) rather than just semantically similar.

## Running Evaluations

```bash
# Run the eval suite against current index
mdvdb eval --dataset eval/questions.json --mode semantic
mdvdb eval --dataset eval/questions.json --mode graph-rag --graph-depth 1

# Compare modes
mdvdb eval --compare semantic,graph-rag --dataset eval/questions.json
```

## Tuning Parameters

Parameters we tune based on evaluation results:

- **Chunk size** — via [chunking strategies](chunking-strategies.md) (current: 512 tokens)
- **Top-K** — how many seeds for vector search (current: 5)
- **Graph depth** — how many hops in [graph expansion](graph-rag-architecture.md#graph-expansion-strategy) (current: 1)
- **Embedding model** — which [model](vector-embeddings.md#embedding-models) to use
- **Reranking** — whether to use a cross-encoder for final ranking

## Related

- [RAG Overview](rag-overview.md) — the pipeline being evaluated
- [Graph RAG Architecture](graph-rag-architecture.md) — the graph expansion approach
- [Vector Embeddings](vector-embeddings.md) — the retrieval mechanism
- [Chunking Strategies](chunking-strategies.md) — affects retrieval granularity
