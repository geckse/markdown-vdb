---
title: "Event Bus & Messaging"
tags: [backend, events, architecture]
category: documentation
author: "Marcus Riveras"
status: published
---
# Event Bus & Messaging

How services communicate asynchronously. This is the operational companion to the [Event Bus](architecture.md#event-bus) overview in the architecture doc.

## Why Async

Not every interaction should be a blocking call. Publishing an event lets a service announce that something happened — a document was ingested, a user was created — without knowing or waiting for whoever cares. This decouples producers from consumers and absorbs bursts that would otherwise overwhelm a synchronous path.

## Delivery Guarantees

The bus provides **at-least-once** delivery. That has a direct consequence for consumers:

- Handlers must be **idempotent** — processing the same event twice must be safe.
- Transient handler failures are retried with backoff, following the same [retry strategy](error-handling.md#retry-strategy) as synchronous calls.
- After repeated failures, an event lands in a **dead-letter queue** for inspection rather than being silently dropped.

## Backpressure

When a consumer falls behind, the bus does not drop events — it lets the queue grow and surfaces the lag as a metric. A sustained [circuit-breaker](error-handling.md#circuit-breaker) trip on a downstream call is often the first sign a consumer is unhealthy, which is why queue depth is a primary signal in [Observability & Logging](observability.md).

## See Also

- [System Architecture](architecture.md#event-bus) — the bus's place in the topology
- [Error Handling Patterns](error-handling.md#retry-strategy) — retries and dead-lettering
- [Observability & Logging](observability.md) — monitoring queue depth and lag
- [Performance & Scaling](performance.md) — scaling consumers to clear backlog

&nbsp;
