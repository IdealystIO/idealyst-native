---
name: example-audit
description: One-sentence summary of what this audit checks.
targets:
  - crates/path/to/crate
severity: medium
---

# {Audit name}

## Background

Why this concern matters in this codebase. Link to any prior incident reports
(e.g. `LEAK_REPORT.md`) or memory entries that motivate the audit.

## Checklist

Specific things to look for. Each item should be concrete enough that two
different agent runs would flag the same code.

- [ ] Item one — describe the pattern, including the rough grep/regex if useful.
- [ ] Item two.
- [ ] Item three.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: brief reasoning (1–3 sentences)
- **Suggested fix**: actionable recommendation, or "needs design discussion"

End with a one-line summary: `Result: N high, M medium, K low findings.`
