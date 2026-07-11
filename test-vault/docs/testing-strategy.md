---
title: "Testing Strategy"
tags: [backend, testing, quality]
category: documentation
author: "Marcel Claus-Ahrens"
status: draft
---
# Testing Strategy

What we test, at which level, and why. The mechanics of running and adding tests live in the [Contributing Guide](../guides/contributing.md); this document is about where to spend the effort.

## The Pyramid

- **Unit tests** — fast, isolated, the bulk of the suite. One behavior per test.
- **Integration tests** — exercise a service against real dependencies (database, bus) in a sandbox.
- **Contract tests** — pin the shape of the [API](api-reference.md) so a change that breaks a caller fails in CI, not in production.

Push tests down the pyramid: prefer a unit test to an integration test, and an integration test to a manual check.

## Test the Error Paths

Happy-path coverage is the easy half. The [error categories](error-handling.md#error-categories) — validation failures, timeouts, permission denials — are where real bugs hide, so every handler's failure branches must be exercised, not just its success branch.

## CI Gate

The full suite runs on every change and must pass before merge. A green suite is the precondition for the [deployment steps](deployment.md#deployment-steps) — nothing ships that CI hasn't vouched for.

## See Also

- [Contributing Guide](../guides/contributing.md) — how to run and write tests locally
- [Error Handling Patterns](error-handling.md#error-categories) — the failure modes tests must cover
- [API Reference](api-reference.md) — the contract that contract tests protect
- [API Versioning](versioning.md) — testing across supported versions

&nbsp;
