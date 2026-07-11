---
title: "API Versioning"
tags: [api, versioning, compatibility]
category: documentation
author: "Jane Chen"
status: published
---
# API Versioning

How the API evolves without breaking existing callers. The current surface is documented in the [API Reference](api-reference.md); this document covers how it changes over time.

## The Version Contract

The API carries a major version in its path (`/v2/...`). Within a major version we make only **backward-compatible** changes:

- Adding a new endpoint or an optional field — safe.
- Adding a new value to an existing enum — safe only if clients tolerate unknowns.
- Removing a field, renaming one, or tightening validation — **breaking**, and only allowed in a new major version.

## Deprecation

A field or endpoint on the way out is marked deprecated well before removal. Deprecated responses carry a `Deprecation` header and a sunset date, giving callers a documented window to migrate. Contract tests (see [Testing Strategy](testing-strategy.md)) guard against removing anything still under support.

## Coordinating Change

API changes rarely stand alone — a new field usually implies a [schema change](database-schema.md) and often a new event on the [Event Bus](event-bus.md). These land in dependency order, each backward-compatible, so a [rolling update](deployment.md#rolling-updates) never exposes a half-migrated state.

## See Also

- [API Reference](api-reference.md) — the versioned surface itself
- [Contributing Guide](../guides/contributing.md) — proposing and reviewing API changes
- [Testing Strategy](testing-strategy.md) — contract tests that enforce compatibility
- [Deployment Guide](deployment.md#rolling-updates) — shipping versioned changes safely

&nbsp;
