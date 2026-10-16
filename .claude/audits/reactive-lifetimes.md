---
name: reactive-lifetimes
description: Owned-scope, signal, and effect lifetime correctness — stale handles, keepalive, and Rc cycles.
targets:
  - crates/runtime/world
  - crates/runtime/scene
  - crates/runtime/vocabulary
  - crates/sdk/client/navigators
  - crates/ui/idea-ui
  - crates/ui/idea-ui-nav
severity: high
---

# Reactive lifetimes

## Background

The kernel (`crates/runtime/world/src/lib.rs`) stores signals and effects in
per-world arenas of **generational** slots behind `Copy` handles. Dropping is
the entire teardown story: slots created inside `collect_owned` are freed when
the returned `Owned` drops (the scene folds each component's `Owned` into its
realized subtree, `Element::Owned` in `crates/runtime/scene/src/element.rs`),
everything else when its `World` drops. A stale handle — its `Owned` scope
already dropped — **panics** on use instead of aliasing the slot's next
occupant. So the lifetime bugs that matter are:

- a handle that outlives the scope that owns it (a panic the first time a
  late callback, timer, or async task touches it);
- state created **outside** any owning scope, which lives until its world
  drops (the slow leak);
- reactive scopes a builder creates and then fails to anchor, so they drop
  early — navigator layout / sidebar scopes are the known case (see
  auto-memory `feedback_navigator_scope_keepalive`).

## Checklist

- [ ] **Handles captured by long-lived callbacks.** Timers, `spawn_async`
      tasks, platform callbacks, and SDK listeners that capture a `Signal` /
      `Effect` / `Memo` handle must be torn down with the scope that owns the
      handle (`after_ms_scoped`, `raf_loop_scoped`, `spawn_then`, an
      `on_cleanup` that cancels them). Flag captures whose lifetime is not
      tied to the owning scope.
- [ ] **State created outside an owning scope.** `signal()` / `effect!` /
      `memo` called outside `collect_owned` (or outside a component body,
      which is collected) lives until the world drops. Flag construction in
      module-level init, `thread_local!` initializers, or builder paths that
      run before the component scope exists.
- [ ] **Keepalive pattern.** Navigator layout / sidebar / screen builders
      that create reactive scopes must anchor the `Owned` (or an effect) in
      the parent's scope. Flag builders that create scopes but return
      without anchoring them.
- [ ] **`Rc<RefCell<…>>` cycles** — particularly between nodes and effects
      capturing those nodes. Look for `Rc::clone` of a node inside an effect
      closure where the effect is owned by that same node.
- [ ] **`mem::forget` / `Box::leak`** — flag every occurrence and check the
      lifetime story is documented (see auto-memory
      `feedback_no_mem_forget_outside_core`).
- [ ] **Disposer registration** — `on_cleanup` / `on_scope_drop` hooks must
      be registered inside the scope that owns the resource; flag orphaned
      cleanups.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: brief reasoning (1–3 sentences) — explicitly say how this could
  manifest at runtime (stale-handle panic, leak, early teardown, etc.).
- **Suggested fix**: actionable recommendation, or "needs design discussion"

End with a one-line summary: `Result: N high, M medium, K low findings.`
