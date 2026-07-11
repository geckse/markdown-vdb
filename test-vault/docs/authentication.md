---
title: "Authentication & Authorization"
tags: [api, auth, security, backend]
category: documentation
author: "Jane Chen"
status: published
---
# Authentication & Authorization

How callers prove who they are and what they're allowed to do. This expands on the [Auth Service](architecture.md#auth-service) design and the client-facing contract in the [API Reference](api-reference.md#authentication).

## Tokens

Every authenticated request carries a bearer token in the `Authorization` header. Tokens are short-lived JWTs signed by the auth service:

- **Access token** — 15-minute lifetime, presented on every request.
- **Refresh token** — 30-day lifetime, exchanged for a new access token when the old one expires.

Never log a raw token. See [Security Best Practices](../guides/security-best-practices.md) for handling and storage rules.

## Scopes

Authorization is scope-based. A token encodes the scopes granted to its subject, and each endpoint declares the scope it requires:

- `documents:read` — list and fetch documents
- `documents:write` — create, update, delete documents
- `search:query` — run full-text and semantic search
- `admin` — cluster and configuration operations

A valid token with insufficient scope yields a `403 Forbidden`, distinct from a missing or expired token's `401 Unauthorized`. Both are permanent caller errors — see [Error Handling Patterns](error-handling.md#error-categories).

## Failure Modes

- **Expired access token** → `401`; the caller should refresh and retry once.
- **Revoked refresh token** → `401`; the caller must re-authenticate from scratch.
- **Repeated auth failures** are rate-limited to blunt credential-stuffing — see [Rate Limiting](rate-limiting.md).

## See Also

- [API Reference](api-reference.md#authentication) — request format and token exchange endpoints
- [System Architecture](architecture.md#auth-service) — where auth sits in the service topology
- [Security Best Practices](../guides/security-best-practices.md) — token storage, rotation, and secrets
- [Configuration Management](configuration.md) — where signing keys and token TTLs are set

&nbsp;
