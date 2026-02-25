---
title: "Security Best Practices"
tags: [security, best-practices, compliance]
category: guides
author: "Priya Sharma"
status: published
---

# Security Best Practices

Mandatory security practices for all services and contributors.

## Authentication & Authorization

### Token Handling

- Never log tokens, even partially
- Store tokens in httpOnly cookies on the frontend, never localStorage
- Validate token signatures on every request — do not trust claims without verification
- Use short-lived access tokens (15 min) to minimize the blast radius of a leaked token

### Permission Model

The platform uses role-based access control (RBAC) with four roles:

| Role | Read | Write | Admin | Billing |
|---|---|---|---|---|
| Viewer | yes | no | no | no |
| Editor | yes | yes | no | no |
| Admin | yes | yes | yes | no |
| Owner | yes | yes | yes | yes |

Roles are scoped to workspaces. A user can be an Admin in one workspace and a Viewer in another.

## Input Validation

### General Rules

- Validate all input at the API boundary, before any processing
- Use allowlists, not blocklists (accept known-good, reject everything else)
- Validate types, ranges, and lengths — a "name" field should not accept 10MB of text
- Sanitize HTML input to prevent XSS — use a proven library, don't write your own sanitizer

### SQL Injection Prevention

- Always use parameterized queries — never string concatenation
- ORMs handle this by default, but be careful with raw queries

```rust
// GOOD
sqlx::query("SELECT * FROM users WHERE id = $1").bind(user_id)

// BAD — never do this
format!("SELECT * FROM users WHERE id = {}", user_id)
```

## Secrets Management

- Never commit secrets to version control
- Use Kubernetes secrets mounted as environment variables
- Rotate secrets quarterly (API keys, database passwords, JWT signing keys)
- Use different secrets per environment (dev, staging, production)

## Dependency Security

- Run `cargo audit` (Rust) and `npm audit` (Node.js) weekly
- Dependabot is enabled — review and merge security PRs promptly
- Pin dependency versions in production — no `^` or `~` ranges
- Review new dependencies before adding them — check maintenance status, download count, and known vulnerabilities

## Incident Response

If you suspect a security incident:

1. **Don't panic**. Document what you see.
2. **Alert** the security channel (`#security-incidents` on Slack)
3. **Contain** — revoke affected tokens, block IPs if needed
4. **Investigate** — use trace IDs to follow the request path
5. **Remediate** — fix the vulnerability, deploy the patch
6. **Post-mortem** — write up what happened and what we'll change to prevent recurrence

## Compliance

- All data at rest is encrypted (AES-256)
- All data in transit uses TLS 1.3
- PII access is logged and auditable
- Data retention: user data deleted within 30 days of account closure
- Annual penetration test by a third-party firm
