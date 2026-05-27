---
name: reactive-lifetimes
description: Scope, signal, and effect lifetime correctness — pruning, keepalive, and Rc cycles.
targets:
  - crates/runtime/core
  - crates/runtime/reactive/arena
  - crates/runtime/reactive/refs
  - crates/ui/idea-ui
severity: high
---

# Reactive lifetimes

## Background

The reactive system has a documented accumulating-subscriber leak
(`LEAK_REPORT.md`): `Signal::subscribers` retains `EffectId`s after the
owning scope drops, because pruning only runs on signal writes. Hot
read-only signals (e.g. the active theme) accumulate dead ids across
rebuilds. There is also a known footgun where layout/sidebar reactive
scopes need a keepalive Effect to survive past `build_*_navigator` return
(see auto-memory `feedback_navigator_scope_keepalive`).

## Checklist

- [ ] `Signal::subscribers` growth — any signal that is read in a long-lived
      effect but written rarely is at risk. Look for places where a signal
      is `.get()` from many short-lived scopes without periodic writes.
- [ ] Scope drop hooks — verify every `Scope`-creating path has matching
      teardown that nulls effect arena slots. Grep for scope/effect arena
      mutation pairs.
- [ ] Keepalive pattern — `build_*_navigator` / sidebar / layout builders
      that create reactive scopes should retain an `Effect` keepalive in
      the parent's scope. Flag builders that create scopes but return
      without anchoring them.
- [ ] `Rc<RefCell<…>>` cycles — particularly between nodes and effects
      capturing those nodes. Look for `Rc::clone` of a node inside an
      `Effect::new` closure where the effect is owned by that same node.
- [ ] `mem::forget` / `Box::leak` — flag every occurrence and check the
      lifetime story is documented.
- [ ] Disposer registration — `on_cleanup` / disposer hooks must be paired
      with the scope that owns them; flag orphaned cleanups.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: brief reasoning (1–3 sentences) — explicitly say how this could
  manifest at runtime (leak, stale subscription, double-free, etc.).
- **Suggested fix**: actionable recommendation, or "needs design discussion"

End with a one-line summary: `Result: N high, M medium, K low findings.`
