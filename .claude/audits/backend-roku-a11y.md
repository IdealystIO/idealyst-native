---
name: backend-roku-a11y
description: Roku Backend impl drops AccessibilityProps because Roku SceneGraph has no AT API; flag any sign that a future SDK has been wired but the param rename was missed.
targets:
  - crates/backend/roku
severity: low
---

# Roku accessibility plumbing

## Background

Roku's SceneGraph has no public assistive-technology (AT) API. Audio
Guide, closed captions, and similar features are handled by the Roku
OS itself, not by the app — there is no documented hook for an app
to attach semantic labels/roles to a node or to post live-region
announcements.

So `backend-roku`'s `Backend` impl currently accepts an
`AccessibilityProps` on every `create_*` (for trait conformance with
iOS / Android / web) and drops it on the floor: every param is
written `_a11y: &AccessibilityProps`. The underscore prefix is the
intentional marker that this is a no-op, deliberately.

If a future Roku SDK release exposes per-node semantic metadata or
an announcement API, the rename `_a11y → a11y` is the signal that
the param is now being consumed. This audit nudges that rename —
and the matching wire-op plumbing — when it should happen.

See the module-level docs in
`crates/backend/roku/src/lib.rs` (`# Accessibility` section) for
the plumbing checklist when Roku adds AT support.

## Checklist

- [ ] **`_a11y` discipline** — every `create_*` in
      `crates/backend/roku/src/lib.rs`'s `impl Backend for RokuBackend`
      block uses `_a11y: &AccessibilityProps` (not `a11y:`). Grep:
      `grep -n "a11y: &framework_core::accessibility::AccessibilityProps" crates/backend/roku/src/lib.rs`.
      A non-underscored `a11y:` is a finding: either (a) the param is
      now consumed and the module-level `# Accessibility` doc should
      be rewritten to describe how it's plumbed, or (b) the rename was
      a mistake and the underscore should go back.
- [ ] **No silent overrides** — `update_accessibility`,
      `announce_for_accessibility`, and `dump_accessibility_tree` are
      not overridden in the Roku impl. If one of them is overridden,
      the override must do something meaningful — not log-and-return,
      not push a wire op the BrightScript client ignores. Flag any
      override whose body is effectively a no-op.
- [ ] **Wire op coverage** — if a `RokuCommand` variant exists that
      carries accessibility metadata (e.g. `SetAccessibility`,
      `Announce`), every `create_*` whose AT props were renamed should
      actually emit it. Flag a renamed `a11y` param that isn't read.
- [ ] **Doc alignment** — the `# Accessibility` section in
      `crates/backend/roku/src/lib.rs` still describes the current
      situation. If AT support has been wired, the doc should
      describe *how* it's wired, not the "currently dropped"
      placeholder.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crates/backend/roku/src/file.rs:line`
- **Issue**: one-line description
- **Why**: brief reasoning (1–3 sentences) — name which checklist
  item triggered the finding.
- **Suggested fix**: actionable recommendation, or "needs design
  discussion".

End with a one-line summary: `Result: N high, M medium, K low findings.`
