---
title: "Performance & Scaling"
tags: [backend, performance, scaling]
category: documentation
author: "Marcus Riveras"
status: draft
---
# Performance & Scaling

How the platform stays fast under load and grows to meet it. The operational levers live in the [Scaling](deployment.md#scaling) section of the deployment guide; this document is the reasoning behind them.

## Measure First

No performance work starts without a number. The latency percentiles and saturation metrics from [Observability & Logging](observability.md) define both the target and the proof that a change helped. Optimizing without a measured baseline is guessing.

## The Levers, in Order

Reach for the cheapest fix that moves the metric:

1. **Cache** the hot path — the highest-leverage move for read-heavy traffic, covered in [Caching Strategy](caching.md).
2. **Shed load** at the edge — [Rate Limiting](rate-limiting.md) keeps a spike from becoming an outage.
3. **Scale out** — add replicas for stateless services once caching and limits are exhausted.
4. **Scale the bottleneck** — a slow consumer or query rarely gets better with more replicas; fix the [Event Bus](event-bus.md) backlog or the query itself.

## Latency Budget

Each request has an end-to-end budget, divided among the hops it makes. When a downstream is slow, a [circuit breaker](error-handling.md#circuit-breaker) fails fast rather than letting one sluggish dependency consume the whole budget and stall the caller.

## See Also

- [Deployment Guide](deployment.md#scaling) — replica counts and autoscaling
- [Caching Strategy](caching.md) — the biggest read-path lever
- [Observability & Logging](observability.md) — the metrics that drive every decision
- [Event Bus & Messaging](event-bus.md) — scaling asynchronous consumers

&nbsp;
