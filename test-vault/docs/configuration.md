---
title: "Configuration Management"
tags: [backend, devops, configuration]
category: documentation
author: "Marcus Riveras"
status: draft
---
# Configuration Management

How the platform is configured across environments without changing code. This builds on the [Environment Configuration](deployment.md#environment-configuration) section of the deployment guide.

## Config vs. Secrets

The two are handled differently on purpose:

- **Configuration** — non-sensitive settings (timeouts, limits, feature flags) held in version-controlled files, one overlay per environment.
- **Secrets** — signing keys, database passwords, API tokens — held in a secrets manager, injected at runtime, never committed. The rules are in [Security Best Practices](../guides/security-best-practices.md).

## Precedence

Settings resolve from most to least specific: environment variable → environment overlay → base defaults. A missing value falls through to the next source, and a required-but-unset secret fails startup loudly rather than running in a degraded state.

## What Belongs in Config

Anything that legitimately varies between environments: rate-limit ceilings (see [Rate Limiting](rate-limiting.md)), token lifetimes (see [Authentication & Authorization](authentication.md)), log verbosity, and feature flags. Anything that must be identical everywhere belongs in code, not config.

## Changing Config Safely

A config change is a deploy-grade event: it's reviewed, versioned, and rolled out the same way. Every change is logged so a behavior shift can be traced back through [Observability & Logging](observability.md).

## See Also

- [Deployment Guide](deployment.md#environment-configuration) — per-environment overlays
- [Security Best Practices](../guides/security-best-practices.md) — secret handling and rotation
- [Authentication & Authorization](authentication.md) — where signing keys and TTLs apply
- [Database Schema](database-schema.md) — connection settings and pool sizing

&nbsp;
