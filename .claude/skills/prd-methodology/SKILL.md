---
name: prd-methodology
description: >
  PRP (Product Requirement Prompt) methodology for writing PRDs. Reference for
  best practices in structuring requirements documents for coding agents.
---

# PRP Methodology — Quick Reference

The PRP (Product Requirement Prompt) framework is a structured process for creating PRDs that coding agents can execute in a single pass.

## Core Principle

A PRD must contain ALL context needed for implementation. If a fresh Claude session with only the PRD can't build the feature correctly, the PRD is incomplete.

## The 3-Step Process

1. **Write initial description** — Brain dump what you want: feature, tech stack, constraints, integrations, examples, documentation references
2. **Generate the PRD** — Research the codebase + web, interview the user, produce a structured document following the base template
3. **Execute the PRD** — Clear context, start fresh, implement from the PRD alone

## What Makes a Good PRD

**DO:**
- Reference specific files and code patterns from the codebase
- Write testable validation criteria ("returns 401 on invalid token")
- Include explicit non-goals to prevent scope creep
- List anti-patterns specific to the project
- Order implementation steps by dependency (what must exist before what)
- Include migration strategy for existing data/behavior

**DON'T:**
- Use vague validation criteria ("works well", "is performant")
- Leave technical design abstract ("use appropriate data structures")
- Assume the implementing agent knows project conventions — spell them out
- Skip the non-goals section — agents will over-build without boundaries
- Write steps that can't be verified independently

## Interview Technique

The most valuable part of PRD generation is the interview. Goal: reduce assumptions to near zero.

- Ask at least 8-10 questions before writing
- Batch questions in groups of 3-4
- Provide recommended answers based on codebase research
- Cover: scope, users, technical constraints, data model, compatibility, edge cases, testing, anti-patterns
- Final question: "What else should I know that I haven't asked about?"

## Validation Criteria Standards

Every criterion must be:
- **Specific**: "Login with wrong password returns 401" not "error handling works"
- **Testable**: Can be verified by running a command or checking output
- **Independent**: Each criterion can be checked on its own
- **Complete**: Cover happy path, edge cases, backward compatibility, and security
