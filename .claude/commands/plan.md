# Plan: Create a Structured Implementation Plan

Take the following feature description and create a comprehensive, structured implementation plan.

**Feature:** $ARGUMENTS

## Process

1. **Understand the request** — Parse the feature description above. Identify what needs to change and what stays the same.

2. **Research the codebase** — Read relevant source files to understand current patterns. Check existing code for conventions, architecture, and testing patterns. Read `CLAUDE.md` for project context.

3. **Ask clarifying questions** — IMPORTANT: Before writing the plan, use the AskUserQuestion tool to ask at least 5 questions about requirements, edge cases, and design decisions. Cover: scope boundaries, error handling, backward compatibility, testing expectations, and UI/UX decisions. Provide suggested answers based on your codebase research.

4. **Write the plan** — After receiving answers, create a structured plan with these sections:

   ### Summary
   One-paragraph description of what will be built.

   ### Files to Modify
   List every file that needs changes, with a description of what changes.

   ### Files to Create
   List any new files, with their purpose.

   ### Implementation Steps
   Numbered steps in implementation order. Each step should reference specific files and describe the exact changes.

   ### Testing Plan
   Which existing tests need updating, what new tests to write, and how to verify the feature end-to-end.

   ### Patterns to Follow
   Reference specific patterns from the existing codebase that the implementation should match.

5. **Save the plan** — Write the plan to `docs/plans/$ARGUMENTS.md` (kebab-case the arguments). Create the `docs/plans/` directory if it doesn't exist.

The plan should be detailed enough that a fresh Claude session with only the plan as input could implement the feature correctly.
