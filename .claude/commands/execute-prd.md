# Execute PRD

Implement a feature by following a Product Requirements Document step by step.

**PRD path:** $ARGUMENTS

## Instructions

1. **Read the PRD** — Read the file at the path above. Understand the full scope: overview, goals, non-goals, technical design, and implementation steps.

2. **Read referenced files** — The PRD's "Patterns to Follow" section references specific codebase files. Read them all to understand the conventions you must match.

3. **Implement step by step** — Follow the "Implementation Steps" section in order. For each step:
   - Read the relevant files before modifying them
   - Make the described changes
   - Verify the change works before moving to the next step

4. **Validate** — After all steps are complete, work through every item in the "Validation Criteria" checklist:
   - Run any tests mentioned
   - Verify each criterion is satisfied
   - If a criterion fails, fix the issue and re-validate

5. **Check anti-patterns** — Review your implementation against the "Anti-Patterns to Avoid" section. Fix any violations.

6. **Report results** — Summarize what was implemented, which validation criteria pass, and any issues encountered.

IMPORTANT: You cannot stop until every validation criterion is checked. Do not skip failing tests or claim "all tests pass" if any test has errors in the output.
