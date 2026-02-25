---
title: "Contributing Guide"
tags: [contributing, workflow, standards]
category: guides
author: "Priya Sharma"
---

# Contributing Guide

How to contribute code, documentation, and ideas to the platform.

## Branch Strategy

We use trunk-based development:

- `main` is always deployable
- Feature branches are short-lived (1-3 days max)
- Branch naming: `<author>/<short-description>` (e.g., `jane/add-oauth-scopes`)
- Rebase on main before opening a PR

## Commit Messages

Follow conventional commits:

```
feat: add OAuth2 scope validation to auth service
fix: handle null email in user registration
docs: update API reference for v2 endpoints
refactor: extract token validation into shared middleware
test: add integration tests for document search
```

The prefix determines the changelog entry and version bump (feat = minor, fix = patch).

## Pull Request Process

### Before Opening

- [ ] Code compiles without warnings
- [ ] All existing tests pass (`make test`)
- [ ] New code has test coverage
- [ ] Linting passes (`make lint`)
- [ ] Documentation is updated if behavior changed

### PR Description Template

```markdown
## What

Brief description of the change.

## Why

Context and motivation. Link to the issue if applicable.

## How

Technical approach. Call out anything non-obvious.

## Testing

How you tested this. Include relevant test output or screenshots.
```

### Review Process

- Two approvals required from code owners
- CI must be green
- No unresolved comments
- Squash merge to main (keeps history clean)

## Code Style

### Rust

- Follow `rustfmt` defaults — don't fight the formatter
- Use `clippy` — treat warnings as errors in CI
- Prefer `thiserror` for library errors, `anyhow` for application code
- Document public APIs with doc comments

### TypeScript

- ESLint + Prettier with the shared config
- Strict TypeScript (`"strict": true`)
- No `any` unless truly unavoidable (and add a comment explaining why)
- Use `zod` for runtime validation of external data

### General

- No magic numbers — use named constants
- No commented-out code — delete it, git remembers
- Functions should do one thing
- If a function needs a comment explaining what it does, it should be renamed instead

## Testing Standards

- Unit tests for business logic (pure functions, validators, transformers)
- Integration tests for API endpoints (test the HTTP layer)
- E2E tests for critical user flows (login, create document, search)
- Test names describe the behavior: `test_expired_token_returns_401`
- No test should depend on another test's state

## Documentation

- Public API changes require API reference updates
- New features need a section in the relevant guide
- Architecture decisions are recorded as ADRs in `docs/adr/`
- Keep docs next to the code they describe
