# Audits

Audits are checklists run by a subagent against one or more crates.
Each audit lives in its own file under `.claude/audits/<name>.md`.

## How it works

1. Each audit file has YAML frontmatter declaring its `targets` (crate paths
   or globs the audit applies to) plus a `description`.
2. `/audit` reads these files, fans out one `Agent` call per
   `(audit, target crate)` pair in parallel, and aggregates the findings.
3. Each agent is read-only — it grep/reads the targeted crate and returns
   structured findings. It does not edit code.

## Authoring an audit

Copy `_template.md` to `<short-kebab-name>.md` and fill it in. Keep audits
narrow: one concern per file (e.g. "reactive lifetimes" not "code quality").
A focused checklist gives the agent a sharp goal and a comparable report
between runs.

### Frontmatter fields

| field         | required | meaning                                                  |
|---------------|----------|----------------------------------------------------------|
| `name`        | yes      | Short kebab-case slug; must match the filename.          |
| `description` | yes      | One sentence on what this audit checks.                  |
| `targets`     | yes      | List of crate paths (relative to repo root) or globs.    |
| `severity`    | no       | `low` \| `medium` \| `high` — default `medium`.          |

### Body sections

- **Background** — why this matters in *this* codebase (link to prior incidents).
- **Checklist** — specific things to grep/inspect. Be concrete.
- **Output format** — what the agent should report back (findings, locations, severity).

## Running

- `/audit list` — show all audits and their targets.
- `/audit <name>` — run one audit across all its target crates.
- `/audit <name> <crate-path>` — run one audit against a single crate.
- `/audit all` — run every audit across every target. (Slow.)

## Adding a new target to an existing audit

Edit the audit file's `targets:` list. No registration step.
