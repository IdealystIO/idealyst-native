---
name: ui-scope
description: UI crates contain only reusable UI component implementations for application authors — no framework internals, no backend code.
targets:
  - crates/ui/idea-ui
  - crates/ui/idea-ui-docs-derive
  - crates/ui/icons-lucide
severity: medium
---

# UI scope

## Architectural rule

**`crates/ui/*` contains additional UI implementations that applications
consume.** It is a library of composed components and presentation helpers
built on top of `framework-core`. It does **not** contain:

- Framework internals (reactive primitives, scheduling, build walkers)
- Platform-specific code (those concerns belong in `backend/`)
- Business logic or app-specific features (those live in the application,
  e.g. `examples/*`)

## Checklist

For each UI crate:

- [ ] **Backend imports** — must not depend on or `use` anything from
      `crates/backend/*`. UI talks to backends through framework
      abstractions only.
- [ ] **Platform conditionals** — flag `#[cfg(target_os = ...)]` /
      `#[cfg(target_arch = ...)]` that gate behavior. UI components
      should render identically across backends; per-backend behavior
      is the backend's responsibility.
- [ ] **Reactive system reimplementation** — UI consumes `Signal` /
      `Effect` / `Scope` from `framework-core`; it should not define
      its own.
- [ ] **App-specific code** — flag domain-specific names (anything tied
      to a particular product feature) that suggest a component belongs
      in the application, not the shared library.
- [ ] **Public surface** — components should expose a clean
      `Props`-driven API. Flag broad `pub use` re-exports of framework
      internals or backend types.
- [ ] **Dependencies** — should depend on `framework-core` and possibly
      sibling UI crates. Flag dependencies on `crates/backend/*` or on
      application crates under `examples/`.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: which architectural rule this violates.
- **Suggested fix**: name the target crate the code should move to, or
  "needs design discussion".

End with a one-line summary: `Result: N high, M medium, K low findings.`
