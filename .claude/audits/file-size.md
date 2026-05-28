---
name: file-size
description: Source files stay at a reasonable size; large files are split along natural structural seams (modules, impl blocks, related primitives) rather than living as monoliths.
targets:
  - crates/runtime/core
  - crates/runtime/macros
  - crates/dev/wire
  - crates/dev/client
  - crates/runtime/layout
  - crates/backend/ios/mobile
  - crates/backend/ios-stack
  - crates/backend/android/mobile
  - crates/backend/web
  - crates/backend/roku
  - crates/gpu-backend/engine
  - crates/dev/server
  - crates/ui/idea-ui
  - crates/mcp/server
severity: low
---

# File size

## Background

Large source files are a navigation, review, and merge-conflict tax. They
also tend to mask creeping coupling: when a single file contains the
implementation of ten different primitives or three unrelated subsystems,
edits to one inevitably read and touch the others. Worse, multiple agents
working in the same repo (see auto-memory `feedback_multi_agent_coordination`)
collide constantly on giant files.

This audit is **structural, not numeric.** A 2 000-line file that is one
cohesive state machine with no clean seam is acceptable; a 900-line file
that is obviously seven loosely related impls is not. The size threshold
exists only to surface candidates — every flagged file needs a *named seam*
in the suggested fix, or it should not be reported.

This is a **low-severity** audit: nothing here is a bug. Findings are
refactor candidates the human should triage when touching the area.

## Thresholds

Apply these as triggers for closer inspection, not as automatic findings:

- **≤ 600 lines**: do not report.
- **600–1 200 lines**: report only if a clear structural seam exists (see
  Checklist). Severity `low`.
- **1 200–2 000 lines**: report unless the file is genuinely cohesive
  (single state machine, generated code, monolithic codegen output).
  Severity `low`.
- **> 2 000 lines**: always report. Severity `low` if the file is
  defensibly monolithic; `medium` if obvious decomposition exists and the
  size is harming review/diff/merge ergonomics.

`mod.rs` / `lib.rs` are not exempt — a 2 000-line `lib.rs` that re-exports
from submodules is fine, but one that *implements* its surface inline is a
finding.

## Checklist

For each file in the target crate exceeding the threshold above:

- [ ] **Independent impl blocks** — does the file contain multiple `impl`
      blocks on unrelated types (or unrelated traits on the same type)
      that could each move to their own file? `impl FooButton`, `impl
      FooSlider`, `impl FooSwitch` all in one `primitives.rs` is the
      canonical example.
- [ ] **Per-primitive / per-variant sections** — does the file have a
      large `match` over a `Element` / wire / scene enum where each arm
      is a sizable block of logic? Each arm is usually a candidate for its
      own submodule, with `mod.rs` left as a thin dispatcher.
- [ ] **Stratified concerns inside one file** — does the file mix layers
      (e.g. wire decoding + reactive plumbing + view construction)? A
      horizontal split by concern is often clearer than the existing
      vertical lump.
- [ ] **Helper / free-function pile** — is there a long tail of private
      `fn` helpers below the main type that are only used by a subset of
      the impls? Those usually belong next to the impl that uses them.
- [ ] **Repeated section markers** — comment banners like `// ===== Style
      =====`, `// ----- FFI -----`, or repeated `//region`/`//endregion`
      are the author telling you where the seams already are. Each region
      is a finding candidate.
- [ ] **`#[cfg(...)]` islands** — large `#[cfg(target_os = "...")]` or
      `#[cfg(feature = "...")]` blocks inside a shared file usually want
      to live in their own platform/feature-specific module. (Cross-check
      with `framework-purity` / `backend-scope` audits — this may be a
      symptom of misplaced code, not just file bloat.)
- [ ] **Generated / vendored code** — if the file is the output of a
      build script, codegen, or vendored verbatim from upstream, do NOT
      flag it. State that explicitly in the finding instead of skipping
      silently.

## What is NOT a finding

- A long file with no clean seam. Splitting purely to hit a line count
  worsens code; the audit is "split where it makes structural sense,"
  not "split to N lines."
- A test file (`tests/`, `#[cfg(test)] mod tests`). Long test files are
  routine and rarely benefit from a split.
- Build-generated output, vendored upstream code, or large data tables
  (e.g. ICU/Unicode tables, glyph maps). Mention these explicitly as
  "intentionally monolithic" and move on.
- Files just over the threshold that are dominated by a single large
  type's `impl` block with tight internal coupling.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium  (this audit does not produce `high`)
- **Location**: `crate/src/file.rs` plus the current line count
  (e.g. `crates/gpu-backend/engine/src/backend_impl.rs (4 362 LOC)`)
- **Issue**: one-line description of the structural smell
- **Why**: which checklist item triggered, and *what specifically* in the
  file makes it a candidate (e.g. "lines 800–1 600 are an `impl
  WgpuTextOps` block that has no shared private state with the rest of
  the file").
- **Suggested fix**: name the seam concretely — e.g. "extract `impl
  WgpuTextOps` to `src/text_ops.rs`" or "split per-primitive arms of the
  `apply()` match into `src/apply/{button,slider,switch}.rs`". If the
  file is intentionally monolithic, say so and recommend no change.

End with a one-line summary: `Result: N medium, K low findings.`
(This audit does not produce `high` findings.)
