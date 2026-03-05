---
title: "Knowledge Graph Overview"
tags: [knowledge-graph, index, overview]
category: knowledge-graph
author: "Alex Park"
status: published
---

# Knowledge Graph Overview

This folder contains interlinked notes that form a knowledge graph for our AI-powered features. The linking structure here is intentional — it models how graph RAG retrieval works: follow edges from a query-relevant node to pull in neighboring context.

## Core Concepts

- [Retrieval-Augmented Generation](rag-overview.md) — the foundational pattern
- [Vector Embeddings](vector-embeddings.md) — how we represent documents as vectors
- [Graph RAG Architecture](graph-rag-architecture.md) — combining graph traversal with RAG

## Applied Projects

- [Semantic Search Project](../notes/ideas.md#semantic-search) — our first RAG application
- [AI-Powered Features](../notes/ideas.md#ai-powered-features) — roadmap for AI integration

## How to Navigate

Start at any node and follow links. Each note links to prerequisites, related concepts, and downstream applications. This mirrors how a graph RAG system would traverse the knowledge graph at query time: find the most relevant node, then expand to its neighbors for richer context.
