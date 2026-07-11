---
title: "Working with Claude"
tags: [claude, ai, workflow, best-practices]
category: guides
author: "Marcel Claus-Ahrens"
status: published
---
# Working with Claude

How to get consistent, high-quality results when using Claude and Claude Code across the project. 

## When to Reach for Claude

- Drafting and refactoring code with tests
- Explaining unfamiliar subsystems before you change them
- Writing and reviewing documentation
- Exploratory research across many files where you only need the conclusion
- Automating repetitive edits with a clear, verifiable outcome

Claude is a collaborator, not an oracle. Treat its output the way you'd treat a capable colleague's first draft: review it, run it, and own the result.

&nbsp;

## This is my section

&nbsp;

my section

## Writing Effective Prompts

- **State the goal, not just the task.** "Add retry logic so transient network errors don't fail ingestion" beats "add retries."
- **Give context up front.** Point at the relevant files, constraints, and conventions instead of making Claude guess.
- **Constrain the output.** Say what "done" looks like — passing tests, a specific format, no new dependencies.
- **Prefer one clear ask per turn.** Bundling five unrelated changes makes review harder and errors likelier.
- **Share the error, not a paraphrase.** Paste the actual output — stack trace, log line, failing assertion.

## Working with Claude Code

- Let it read the codebase before it writes — it matches existing style and conventions better with context.
- Keep changes reviewable: smaller diffs are easier to verify and safer to merge.
- Ask it to run the tests and show you the output rather than trusting "should work."
- Use plan mode for anything non-trivial so you can approve the approach before code is written.
- It respects `CLAUDE.md` — put project rules there so you don't repeat them every session.

## Claude Cowork

Coworking with Claude means treating a session as a shared workspace, not a one-shot request. You steer, Claude executes, and you both keep the same picture of the work.

- **Set up the workspace once.** Point Claude at the relevant files and state the goal, then let it work within that context instead of re-explaining each turn.
- **Work in a loop.** Ask, review, correct, repeat. Small back-and-forth beats a single giant prompt.
- **Delegate the legwork, keep the judgment.** Let Claude read, search, and draft across many files; you decide what's right and what ships.
- **Think out loud together.** Use plan mode to agree on an approach before code is written — it's cheaper to fix a plan than a diff.
- **Hand off cleanly.** End a session with tests run and state summarized so the next session (yours or Claude's) picks up without guesswork.

Coworking works best when the feedback loop is tight: the faster you react to each step, the less rework piles up.

**Example — cleaning up a spreadsheet.** Say you have a messy Excel export of vault stats and want it usable:

1. Share the file and the goal — "normalize the columns, dedupe rows, and add a monthly totals sheet."
2. Claude proposes a plan: which columns to rename, how to detect duplicates, what the summary should compute.
3. You approve or adjust the plan, then Claude writes the transform (a script or formulas) and shows a preview.
4. You spot-check a few rows, flag anything off, and Claude corrects it.
5. Claude hands back the cleaned workbook plus a short note on what changed — ready to re-import or share.

The point isn't that Claude touches Excel directly; it's that you cowork through the plan-do-review loop so the result is one you can trust.

## When Claude Gets Stuck

Even a good session hits walls — Claude loops on a failing test, misreads what you meant, or drifts off the goal. Getting unstuck is a skill of its own, and it's almost always faster to reset the frame than to keep nudging.

- **Name the drift early.** The moment an answer heads the wrong way, say so plainly instead of hoping the next turn self-corrects — small course corrections beat a big rewind.
- **Give it the missing fact, not a scolding.** If it's guessing, the gap is usually context: paste the real error, point at the right file, or state the constraint it didn't know.
- **Break the loop with a smaller step.** When it thrashes on a big change, ask for the narrowest possible version first — one function, one test — then build back up.
- **Ask it to explain before it fixes.** "Walk me through why this fails" surfaces the wrong assumption faster than another blind attempt.
- **Know when to start fresh.** A long, tangled thread carries its own confusion; a clean session with a sharp brief often beats untangling the old one.

Being stuck is information, not failure — it usually means the goal or the context isn't yet sharp enough to act on. Tighten one of those and the loop starts moving again. And when a session finally cracks a tricky problem, jot down what unblocked it so the next one starts a step ahead.

## Reviewing Claude's Output

Always verify before you trust:

- [ ] Code compiles and the relevant tests pass
- [ ] The change does only what you asked — no unrelated edits
- [ ] Edge cases and error paths are handled, not just the happy path
- [ ] No secrets, credentials, or real data were introduced
- [ ] Comments and naming match the surrounding code

If something looks wrong, say so directly. Claude responds well to specific corrections and will adjust.

## Guardrails

- **Never paste secrets** — API keys, tokens, or customer PII — into a prompt.
- **You own the merge.** Claude drafts; a human is accountable for what lands on `main`.
- **Don't skip tests to save time.** Untested code is unfinished code, whoever wrote it.
- **Cross-check facts.** For anything load-bearing — version numbers, API behavior, pricing — verify against the source.

## Tips That Compound

- Keep `CLAUDE.md` current — it's the cheapest way to raise every future session's quality.
- Save recurring instructions as project conventions instead of re-explaining them.
- When a session produces a useful pattern, write it down as a guide like this one.
- Short feedback loops win: run early, run often, correct fast.

&nbsp;