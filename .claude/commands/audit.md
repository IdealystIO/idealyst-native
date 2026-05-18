---
description: Run codebase audits in parallel agents. Pass `list`, `<audit-name>`, `<audit-name> <crate>`, or `all`.
argument-hint: list | <audit-name> [crate-path] | all
allowed-tools: Bash(ls:*), Read, Agent
---

# /audit

The user invoked the audit runner with: **$ARGUMENTS**

## How this command works

Audits live in `.claude/audits/<name>.md`. Each audit file has YAML
frontmatter with `name`, `description`, `targets` (list of crate paths
the audit applies to), and `severity`. Audits are read-only checklists
run by a subagent against one crate at a time.

## Steps for you to follow

1. **List available audits.** Read every `.claude/audits/*.md` file
   except `README.md` and `_template.md`. Parse the YAML frontmatter to
   build a table of `{ name, description, targets, severity }`.

2. **Resolve `$ARGUMENTS` to a run plan.**
   - `list` (or empty arguments) → print the table of available audits
     and stop. Do not launch agents.
   - `all` → run every audit against every one of its declared
     `targets`.
   - `<audit-name>` → run that audit against every one of its declared
     `targets`.
   - `<audit-name> <crate-path>` → run that audit against just that one
     crate. If the path is not in the audit's declared `targets`, ask
     the user to confirm before proceeding.
   - Unknown audit name → list the available names and stop.

3. **Fan out in parallel.** For each `(audit, crate)` pair in the plan,
   launch one `Agent` call with `subagent_type: general-purpose`.
   **Put all `Agent` tool calls in a single assistant message so they
   run concurrently.** Each prompt must be self-contained — the agent
   has no view of this conversation.

   Use this prompt template (substitute the bracketed parts):

   ```
   You are running the "[audit-name]" audit against the crate at
   `[crate-path]` in the idealyst-native workspace
   (/Users/nicho/Desktop/idealyst-native).

   Read the audit spec at `.claude/audits/[audit-name].md` for the
   checklist and output format. Apply every checklist item to the
   target crate by reading source files and grepping the crate.

   You are read-only — do not edit any files. Return only the
   findings list in the exact format the audit spec requires, ending
   with the one-line summary. Do not include preamble.
   ```

4. **Aggregate.** When all agents return, present a single combined
   report grouped by audit, with each crate's findings as a subsection.
   At the top, include a roll-up table: `audit | crate | high | med |
   low`. At the bottom, list the highest-severity findings across the
   whole run (top 10 or fewer).

## Notes

- Do not edit code as part of running an audit. The audit produces a
  report; any fixes are a separate, explicit follow-up.
- If an audit file has malformed frontmatter, surface the parse error
  and skip it — don't guess at the targets.
- Crate paths in frontmatter are relative to the repo root. Treat them
  as literal paths (glob patterns with `**` are allowed; expand them
  with `ls` before launching agents).
