# PRD: [Feature Title]

## Overview

One-paragraph summary of what this feature does and why it matters.

## Problem Statement

What problem does this solve? Why is the current state insufficient?

## Goals

- Bulleted list of what this feature achieves
- Each goal should be measurable or verifiable

## Non-Goals

- Explicit list of what this feature does NOT do
- Prevents scope creep during implementation

## Technical Design

### Data Model Changes

Current schema vs proposed schema. Show exact field additions, type changes, or new entities.

### Interface Changes

New or modified function signatures, API endpoints, or component props.

### New Commands / API / UI

User-facing additions being introduced.

### Migration Strategy

How existing data or behavior transitions to the new version. Include backward compatibility approach.

## Implementation Steps

Numbered, ordered steps. Each step references specific files and describes exact changes. Detailed enough that a coding agent can follow them without additional context.

1. **Step title** — Description of what to do, which files to modify, and what the expected outcome is.
2. **Step title** — ...
3. ...

## Validation Criteria

- [ ] Checklist of conditions that must be true when complete
- [ ] Include functional tests (happy path)
- [ ] Include edge case tests
- [ ] Include backward compatibility checks
- [ ] Include performance considerations if relevant

## Anti-Patterns to Avoid

- Specific mistakes the implementing agent should NOT make
- Based on project conventions and known pitfalls
- Each anti-pattern should explain WHY it's wrong, not just WHAT to avoid

## Patterns to Follow

- Reference specific files and code patterns from the existing codebase that the implementation should mirror
- Include file paths so the agent can read examples
- Explain why these patterns matter for consistency
