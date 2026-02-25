---
title: "Developer Onboarding Guide"
tags: [onboarding, getting-started, internal]
category: notes
author: "Alex Park"
---

# Developer Onboarding Guide

Welcome to the team. This guide gets you from zero to productive.

## Day 1: Environment Setup

### Prerequisites

- macOS or Linux (Windows via WSL2)
- Docker Desktop
- Node.js 20+ (use `nvm`)
- Rust toolchain (for the search indexer)
- kubectl + helm

### Clone and Run

```bash
git clone git@github.com:acme/platform.git
cd platform
make setup    # installs dependencies, pulls images, seeds the database
make dev      # starts all services locally via docker-compose
```

The dev environment runs at `http://localhost:3000`. Hot reload is enabled for all services.

### Verify Everything Works

```bash
make test     # runs unit tests for all services
make e2e      # runs end-to-end tests against the local environment
```

If `make e2e` fails, check that Docker has at least 8GB memory allocated.

## Day 2: Codebase Tour

### Repository Structure

```
platform/
├── services/
│   ├── auth/           # Authentication service (Rust)
│   ├── documents/      # Document CRUD (Node.js)
│   ├── search/         # Search indexer and query API (Rust)
│   ├── analytics/      # Event tracking (Python)
│   └── notifications/  # Email, push, in-app (Node.js)
├── packages/
│   ├── shared-types/   # TypeScript types shared across services
│   └── ui-components/  # React component library
├── deploy/
│   ├── chart/          # Helm chart
│   └── terraform/      # Infrastructure as code
└── docs/               # You're reading from here
```

### Key Patterns

- **Service boundaries**: each service owns its data. No shared databases.
- **Event-driven**: cross-service communication via the event bus, not direct calls.
- **Trunk-based development**: short-lived branches, merge to main daily.
- **Feature flags**: new features ship behind flags, enabled per environment.

## Day 3: First Contribution

### Pick a Starter Issue

Look for issues labeled `good-first-issue` in the issue tracker. These are scoped, well-described tasks that touch a single service.

### Development Workflow

1. Create a branch: `git checkout -b alex/fix-token-refresh`
2. Make your changes
3. Write tests (unit + integration where applicable)
4. Run `make lint` and `make test`
5. Open a PR — two approvals required
6. CI runs tests, linting, and a preview deployment
7. Merge when green

### Code Review Norms

- Respond to reviews within 24 hours
- Approve means "I'm confident this is correct and safe to deploy"
- Request changes for bugs, security issues, or missing tests
- Use comments (not request changes) for style preferences or suggestions

## Useful Links

- API reference: `docs/api-reference.md`
- Architecture overview: `docs/architecture.md`
- Deployment guide: `docs/deployment.md`
- Runbooks: `runbooks/` (for on-call)
- Slack: `#engineering`, `#incidents`, `#random`
