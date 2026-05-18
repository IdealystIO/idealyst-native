---
name: backend-scope
description: Backend crates contain only platform glue — no application logic, no UI components, no framework-internal duplication.
targets:
  - crates/backend/ios/core
  - crates/backend/ios/mobile
  - crates/backend/ios/tv
  - crates/backend/ios-stack
  - crates/backend/android/core
  - crates/backend/android/mobile
  - crates/backend/android/tv
  - crates/backend/roku
  - crates/backend/roku/macros
  - crates/backend/roku/transpile
  - crates/backend/web
severity: high
---

# Backend scope

## Architectural rule

**`backend/*` crates exist only to glue the Rust application framework
to a specific native platform.** They implement framework-defined traits
using platform APIs. They do **not** contain:

- Business logic or application-level features
- UI component libraries (those live in `crates/ui/`)
- Reactive primitives, scheduling, or other framework-internal concepts
  (those live in `framework/`)
- General-purpose abstractions reusable across backends

If a piece of code in a backend crate would be useful to *another*
backend, it probably belongs in `framework/` instead.

## Checklist

For each backend crate:

- [ ] **Application logic** — flag anything that looks like domain
      behavior or business rules, not platform translation.
- [ ] **UI component definitions** — flag any code that defines reusable
      UI components (Button, Card, etc.). Backends translate primitive
      operations; component composition belongs in `crates/ui/`.
- [ ] **Reactive primitives** — backends should not define their own
      `Signal`, `Effect`, `Scope` analogues. If they need to participate
      in reactivity, they should consume framework's primitives.
- [ ] **Cross-backend duplication** — grep two backends for the same
      pattern (e.g. style resolution, layout flushing). Duplication is a
      signal the logic should be lifted to `framework/`.
- [ ] **Public surface** — the crate's `pub` items should overwhelmingly
      be (a) `extern` entry points the foreign host calls or (b)
      implementations of framework traits. Flag broad `pub` modules
      exposing internals.
- [ ] **Dependencies** — should depend on `framework-core` (and other
      framework crates) plus platform-specific deps. Flag dependencies on
      `crates/ui/*` or on sibling backends.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: explain what category the code falls into (app logic, UI
  composition, reactive primitive, cross-backend duplication).
- **Suggested fix**: name the target crate the code should move to, or
  "needs design discussion".

End with a one-line summary: `Result: N high, M medium, K low findings.`
