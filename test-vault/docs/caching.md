---
title: "Caching Strategy"
tags: [backend, performance, data]
category: documentation
author: "Jane Chen"
status: draft
---
# Caching Strategy

How and where the platform caches data to cut latency and load. Caching sits in front of the [Data Layer](architecture.md#data-layer) and is one of the biggest levers described in [Performance & Scaling](performance.md).

## Layers

Caching happens at more than one level, each with a different lifetime:

- **Request cache** — memoizes expensive reads within a single request.
- **Service cache** — a shared in-memory or Redis tier for hot data across requests.
- **CDN edge** — caches immutable, public responses close to the caller.

## Invalidation

The hard part is not caching — it's knowing when a cached value is stale. We prefer:

1. **Short TTLs** for data that tolerates brief staleness.
2. **Event-driven invalidation** for data that must be fresh, keyed on the same writes that update the [Database Schema](database-schema.md).
3. **Versioned keys** so a schema or format change never serves a stale shape.

## What Not to Cache

Never cache authenticated, user-specific responses at a shared layer, and never cache error responses. [Search results](api-reference.md#search) are cached only for identical queries within a short window, because the underlying index changes as documents are ingested.

## See Also

- [System Architecture](architecture.md#data-layer) — what sits behind the cache
- [Performance & Scaling](performance.md) — where caching fits the broader latency budget
- [API Reference](api-reference.md#search) — endpoints whose responses are cache-sensitive
- [Database Schema](database-schema.md) — the source of truth caches derive from

&nbsp;
