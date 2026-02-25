# Generate PRD

Generate a comprehensive Product Requirements Document for the following feature.

**Feature:** $ARGUMENTS

## Instructions

1. **Read the base template** — Read `docs/prds/base-template.md` to understand the required PRD structure. Read `docs/prds/example-prd.md` to see the quality bar.

2. **Research the codebase** — Explore the project to understand the existing architecture, conventions, data models, and patterns. Read `CLAUDE.md` for project context.

3. **Research externally (if needed)** — If the feature involves third-party libraries, APIs, or platforms, search the web for current documentation and best practices.

4. **Interview the user** — IMPORTANT: You MUST ask clarifying questions before writing the PRD. The goal is to reduce assumptions to near zero. Use the AskUserQuestion tool to ask at least 8-10 questions in batches of 3-4, covering:
   - Scope: What is in scope vs explicitly out of scope?
   - Users: Who is this for? What is their workflow?
   - Behavior: What should happen in edge cases?
   - Technical: Are there constraints on implementation approach?
   - Data: What data model changes are needed?
   - Compatibility: Must this be backward compatible?
   - Testing: What validation criteria matter most?
   - Priority: Which parts are must-have vs nice-to-have?
   - Anti-patterns: What mistakes should be explicitly avoided?

   Provide recommended answers based on your research. Wait for responses before asking the next batch. After all questions, ask: "Is there anything else I should know that I haven't asked about?"

5. **Generate the PRD** — Write the full PRD following the base template structure. Every section must be filled in with specific, actionable content. No placeholders, no vague language.

6. **Save** — Write the PRD to `docs/prds/<feature-name>.md` (kebab-case). Present a summary to the user.
