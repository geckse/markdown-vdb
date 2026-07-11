---
title: "Rate Limiting"
tags: [api, gateway, reliability, backend]
category: documentation
author: "Marcus Riveras"
status: published
---
# Rate Limiting

How the platform protects itself from overload and abuse. The client-facing limits are listed in the [API Reference](api-reference.md#rate-limits); this document explains how they're enforced.

## Where It Happens

Rate limiting is applied at the [API Gateway](architecture.md#api-gateway), before a request ever reaches a backend service. This keeps the enforcement logic in one place and shields every downstream service uniformly.

## Buckets

Limits are tracked per identity using a token-bucket algorithm:

- **Per token** — the primary limit, keyed on the authenticated subject.
- **Per IP** — a coarser fallback for unauthenticated traffic and for [auth failures](authentication.md#failure-modes).
- **Global** — a safety ceiling that protects shared infrastructure regardless of who's calling.

The most restrictive matching bucket wins.

## What Callers See

A throttled request returns `429 Too Many Requests` with a `Retry-After` header. This is a transient condition — well-behaved clients back off and retry, exactly as described in the [retry strategy](error-handling.md#retry-strategy).

## Tuning

Limits are configuration, not code — they can be adjusted per environment without a deploy. Every throttling decision is emitted as a metric so limits can be tuned against real traffic; see [Observability & Logging](observability.md).

## See Also

- [API Reference](api-reference.md#rate-limits) — the published per-tier limits
- [System Architecture](architecture.md#api-gateway) — where enforcement lives
- [Error Handling Patterns](error-handling.md) — how `429` fits the error taxonomy
- [Observability & Logging](observability.md) — metrics for tuning limits

&nbsp;
