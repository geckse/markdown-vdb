---
title: "System Architecture"
tags: [architecture, backend, infrastructure]
category: documentation
author: "Jane Chen"
status: published
---

# System Architecture

An overview of the platform's core architecture, service boundaries, and communication patterns.

## Service Overview

The platform is composed of five core services that communicate via asynchronous message passing and synchronous REST APIs where low latency is required.

Each service owns its own database and exposes a well-defined API boundary. There are no shared databases between services — all data exchange happens through APIs or events.

## Auth Service

The auth service sits behind the API gateway and validates JWT tokens on every request. It issues short-lived access tokens (15 min) and long-lived refresh tokens (30 days).

Token validation uses RS256 asymmetric signing. The public key is distributed to all services at startup so they can verify tokens locally without calling the auth service on every request.

### Token Flow

1. Client sends credentials to `/auth/token`
2. Auth service validates against the user store
3. Returns JWT access token + refresh token
4. Client includes `Authorization: Bearer <token>` on subsequent requests
5. API gateway validates the token signature locally
6. If expired, client uses refresh token to get a new access token

## API Gateway

The API gateway is the single entry point for all external traffic. It handles:

- TLS termination
- Rate limiting (100 req/s per client by default)
- Request routing to downstream services
- Token validation (signature only, no auth service call)
- Request/response logging

The gateway does not transform payloads — it forwards requests as-is to the appropriate service based on URL prefix matching.

## Event Bus

Services communicate asynchronously through an event bus for operations that don't require an immediate response. Events are durable and replayed on consumer restart.

Common event types:

- `user.created` — triggers welcome email and analytics setup
- `document.updated` — triggers re-indexing and cache invalidation
- `payment.completed` — triggers license activation and receipt generation

Events follow a schema registry to ensure producers and consumers agree on the payload format.

## Data Layer

Each service maintains its own data store:

| Service | Store | Why |
|---|---|---|
| Auth | PostgreSQL | Relational user data, ACID compliance |
| Documents | PostgreSQL + S3 | Metadata in Postgres, files in S3 |
| Search | Elasticsearch | Full-text search, inverted index |
| Analytics | ClickHouse | Columnar storage for time-series queries |
| Notifications | Redis | Ephemeral queues, pub/sub |

## Deployment

All services are containerized and deployed to Kubernetes. Each service has:

- A Helm chart with environment-specific values
- Horizontal pod autoscaling based on CPU and request latency
- Liveness and readiness probes
- Structured JSON logging shipped to a centralized log aggregator
