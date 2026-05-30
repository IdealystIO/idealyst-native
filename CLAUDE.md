# Project rules for Claude agents

These rules apply to all work in this repo. Follow them unless the user explicitly overrides one in the current conversation.

## 0. Never `git stash`

Do not run `git stash`, `git stash push`, `git stash pop`, or any variant — not for "saving work", not for "checking if a failure is pre-existing", not ever. Multiple agents may share this repo and a stash silently buries someone else's in-progress work. To check whether a failure is pre-existing, inspect the diff of the relevant files or read git history; don't stash to bisect.

If you genuinely think stashing is the only option, stop and ask the user first.

## 0a. No `Co-Authored-By: Claude` trailers

Do not add `Co-Authored-By: Claude …` trailers (or any other AI attribution trailer like `Generated-By: Claude`, `🤖 Generated with Claude Code`, etc.) to commit messages or PR bodies. This overrides the global Claude Code default in the harness's git-commit instructions for THIS repo.

If you've already authored a commit with a Claude trailer in this conversation, amend it (or rewrite via `git commit-tree` plumbing if the working tree isn't clean) to strip the trailer before moving on. Never let a Claude-trailered commit ship.

The repo's author of record is the user. Tools that helped write the code don't get an attribution line.

## 1. Test changes — especially in framework core

Run the test suite when you make changes. Architectural changes to framework core (anything in `crates/framework/core/`, the Backend trait, reactive system, wire protocol, scene model) MUST be accompanied by tests that cover the new behavior. Framework stability is non-negotiable — a change without test coverage is incomplete.

If existing tests don't cover the area you're touching, add coverage as part of the same change. Don't merge "the tests still pass" when the tests don't actually exercise what you changed.

## 2. Keep documentation aligned

When you change behavior, find the documentation that describes it and update it in the same change. Don't leave docs lying about the new state of the world.

- `.claude/audits/` contains audit definitions that sweep the codebase for inconsistencies. Run the relevant audit (`/audit <name>` or `/audit all`) when you suspect docs may have drifted.
- New features need documentation alongside the implementation, not as a follow-up.
- `idea-ui` is an adjacent project with its own docs — do not update idea-ui docs from this repo, and don't assume changes here propagate there.

## 3. Core stays minimal — peripheral features go through External

`crates/framework/core/` is for the lowest primitives only. If you're tempted to add a feature that feels like a "widget," "helper," "convenience," or anything composable from existing primitives, build it as a third-party extension using `Element::External` plus the per-backend registry (see [[project_third_party_extension]]). Do not bloat core with peripheral features.

If `Element::External` isn't wired up yet for the surface you need, that's a signal to wire it up — not a license to add the feature directly to core.

## 4. No timeline-deferral

Don't say "we can do this in a follow-up PR" or "let's punt this to tomorrow." If the change is needed for correctness or completeness, do it now in the same change. If it's genuinely out of scope, say so directly and explain why — but don't manufacture artificial scope boundaries to avoid work.

This applies to your own internal reasoning too: don't write TODOs as a way to skip the hard part of a problem.

## 5. Proven, documented solutions

Low-level framework implementations require:
- **Rationale**: A brief explanation of *why* this approach was chosen, especially when it's not obvious. Trade-offs against alternatives go in the commit message or a comment when the constraint isn't visible in the code.
- **Evidence**: The change should be verified to work — by tests, by running the relevant example, or by exercising the affected platform. State explicitly how you verified it.
- **Subtle invariants documented**: If the code relies on a non-obvious property (UIKit quirk, Taffy behavior, GPU pipeline ordering, etc.), leave a short comment explaining the *why*. Existing memory entries like [[project_ios_scrollview_bounds_origin]] and [[project_ios_clear_children_taffy_sync]] are the model — terse, specific, naming the bug being prevented.

Speculative or "this should work" changes to low-level code are not acceptable. If you don't fully understand why a fix works, dig until you do.

## 6. Profiling the framework — phase_timer + debug-stats

When diagnosing perf regressions or attributing time across the framework's hot paths, prefer the built-in `PhaseTimer` over guessing. It's an RAII wrapper that reports microsecond durations into a thread-local phase-counter map; zero overhead when the feature is off.

### How it works

- **`framework_core::debug` module** (gated by the `debug-stats` Cargo feature on `framework-core`) holds the thread-local counters keyed by `&'static str` phase name. Each entry tracks `call_count`, `total_us`, `max_us`.
- **`backend-web/src/phase_timer.rs`** exposes `PhaseTimer::start("phase_name")` — returns a guard that fires `record_apply_phase` on drop. Stub-struct equivalent when `debug-stats` is off, so the macro expands to dead code the optimizer strips.
- **Reading counters**: call `framework_core::debug::take_phase_counters()` (returns + clears) or `clear_phase_counters()`.

### Adding a timer

Wrap the work you want to attribute. RAII handles early-return paths:

```rust
let _t = crate::phase_timer::PhaseTimer::start("update_text_by_id");
// ... work ...
// timer fires on scope exit
```

Use **stable, specific** phase names — they're aggregation keys. Prefer `"text_flush_join"` over `"join"`. Existing names in the web backend: `"execute_batch_total"`, `"execute_batch_encode"`, `"execute_batch_ffi_call"`, `"execute_batch_decode"`. Follow that convention.

### Enabling for a measurement run

The `debug-stats` feature is **OFF by default** in the bench variants because the timer reads (`performance.now()` on web) skew the per-leaf numbers when 10 k+ ops hit the timer. To enable temporarily:

1. Define a `debug-stats` feature on the variant's own crate that forwards to deps — `features = ["framework-core/debug-stats"]` on the dep line is NOT enough; the variant's own `#[cfg(feature = "debug-stats")]` blocks (like `phase_counters_json`) only see THIS crate's features, not deps'. The variant must declare its own `debug-stats = ["framework-core/debug-stats", "backend-web/debug-stats"]` in `[features]`, then default to it (or pass `--features` at build time).
2. **Call `backend_web::install_time_source()` at startup.** Without it, `framework_core::time::now_micros()` returns `0` on wasm32, and every `PhaseTimer` records duration `0` — counts are real but all timings are useless. The variant's `start()` must call both `install_scheduler()` AND `install_time_source()`.
3. Add a `#[wasm_bindgen]` export that drains and JSON-serializes phase counters — typically extend the variant's existing `bench_stats_json()` (see [benchmark/idealyst-native/wasm/src/lib.rs](benchmark/idealyst-native/wasm/src/lib.rs)).
4. Run the bench, call the export from devtools (`window.benchStats()`). For iframe-hosted variants, log to console + `parent.postMessage` so the data reaches the parent devtools.
5. **Turn it back off before reporting benchmark numbers** — debug-stats inflates per-op cost.

### When to reach for it

- Comparing two implementations of the same hot path (e.g., `update_text_by_id` vs `update_text_by_id_with`) to see where time actually goes.
- Attributing apply-window cost across phases when a single high-level timer can't tell you whether the cost is in `format!()`, FFI marshalling, or the JS-side update loop.
- Validating that a "should be faster" change actually is — if the counters say the targeted phase didn't shrink, the change didn't work, regardless of what wall-clock says.

Do NOT add `PhaseTimer` calls without `#[cfg]` gating — the gate already lives in the struct itself, so plain `PhaseTimer::start(...)` at call sites is correct. The optimizer strips it when the feature is off.

## 7. Backend determines how things render — implementations are uniform, not patched per platform

Cross-platform ubiquity is the framework's reason to exist. One author tree, every backend, native output that looks and behaves the same. The Backend trait absorbs the toolkit differences (UIKit vs AppKit vs DOM vs wgpu); the *observable behavior* is identical.

That means **no per-platform hacks in framework/backend code** to make a feature work on platform Y. Animations should not have a 0.95 scale on iOS and a 0.93 scale on Android because "the renders differ." If a primitive looks or animates differently across backends, the backend that's wrong needs to be fixed at its root — not patched at the call site.

Concretely:

- **Backend implementations diverge in mechanism but converge in output.** UIKit uses `UIView.transform`, AppKit uses CALayer + frame offset, web uses CSS `transform`. The visual result is the same.
- **Don't add framework-side `if platform == X` workarounds** to compensate for a backend bug. Fix the bug. If a primitive cannot work on a backend without hacks, that's a sign the primitive's design is wrong for that backend — redesign or escalate to `Element::External`, don't compromise the others.
- **`is_simulator()` does not belong in the public API.** "Simulator vs device" is a dev-time concept that has no consistent meaning across backends (iOS Simulator, wgpu sim, web in DevTools, …) and any author code branching on it is necessarily fragile. The `Platform` enum + `Backend::platform()` exist for *legitimate* runtime variance (different keyboard shortcuts on `MacOs`, different copy on `Web`, etc.) — that branching is fine. A sim/device predicate is not.
- **Dev-only markers** ("am I in the dev build?") belong behind `#[cfg(debug_assertions)]`, not behind a runtime predicate. They should not survive into release builds.

When reviewing PRs / audits: a per-platform hack inside a backend (e.g., "subtract 2px on iOS only" to fix alignment) is a smell. The cause is almost always upstream — wrong default in the trait surface, wrong style translation, wrong intrinsic measurement. Fix the upstream cause so every backend benefits.

## 8. Every bug fix lands with a regression test

When you fix a bug or regression, add a test that fails before the fix and passes after. No exceptions for "small" or "obvious" fixes — those are exactly the ones that come back. The test belongs in the same change as the fix, not a follow-up.

- The test must actually exercise the broken path. A test that passes against the buggy code is not a regression test.
- If the bug is in a layer that's hard to unit-test (platform-specific UIKit/Android behavior, GPU output, real device input), add the closest reachable test — an integration test against the backend trait, a snapshot of the relevant state, or a scripted example that reproduces the scenario — and document in a comment why a tighter test isn't possible.
- Name the test after the bug, not the function. `regression_ios_scrollview_resets_on_layout` beats `test_apply_frames_3`.
- If you can't write a failing test first, you don't yet understand the bug. Keep digging until you can reproduce it deterministically.

## 9. Component implementation standards

Components in this repo have one canonical shape. When writing a new component, or modifying an existing one, conform to it.

### 9.1 Wrap with `#[component]`

Components MUST be declared with the `#[component]` attribute. The macro generates the props struct's `BuildElement` impl, the `pub type Tag = TagProps` alias that makes `Tag()` work as a `ui!` call site, and the `Default` glue the struct-literal dispatch relies on. Don't hand-roll bespoke builder methods, manual `BuildElement` impls, `pascal_to_snake` shims, or centralized `build_impl!`-style registries.

The *contract* — props struct + `Default` + `BuildElement` — is what `ui!` actually requires; `#[component]` is just the standard way to satisfy it. If you find yourself reaching for a manual impl, the macro should grow instead (e.g., it accepts `#[component(children)]` for container components that move children out of props, and `#[component(default(field = expr))]` for non-Default starting values).

For container components, name the fn PascalCase to match the tag (`fn Card`, not `fn card`). The fn and the `pub type Card = CardProps` alias coexist in different namespaces — the fn-call form (`Card(props)`) and the struct-literal form (`ui! { Card(...) }`) both work, and `#[component]` adds `#[allow(non_snake_case)]` automatically.

### 9.2 Render with `ui!`

Component bodies should compose their tree with the `ui!` macro. The only acceptable deviations:

- **Pedagogical examples** that explicitly showcase `jsx!` syntax or the hand-built `Element` form. These must include a comment naming what's being demonstrated and why `ui!` was skipped.
- **A documented technical limitation** where `ui!` genuinely can't express the construct. Vanishingly rare — if you think you've found one, write the comment explaining the limitation and link it to an issue or memory entry so the next person hits the constraint, not the workaround.

`jsx!` is a peer macro and is fine when a file or example is consistently `jsx!`-styled. The rule is: pick one, stay in it, don't mix `ui!`/`jsx!`/manual `Element` construction in the same component without a reason.

**Primitives are lowercase, components are PascalCase.** Inside `ui!`, the framework's leaf primitives (`view`, `text`, `button`, `image`, `icon`, `text_input`, `scroll_view`, `slider`, `toggle`, `link`, `overlay`, `anchored_overlay`, `presence`, `activity_indicator`, `flat_list`, `when`, `graphics`) are snake_case. User-defined components (anything declared with `#[component]`) are PascalCase. This mirrors React's `<div>` vs `<MyButton>` convention and makes "framework leaf" visually distinct from "user component" at every call site. The macro also still accepts the legacy PascalCase forms (`View`, `Text`, …) for back-compat — don't write new call sites that way, and convert legacy PascalCase primitives to lowercase when you touch the file. The framework's own crates (`crates/runtime/core/`, `crates/backend/`, `crates/gpu-backend/`, tests) still use the legacy PascalCase primitives and are NOT to be swept; the convention applies to app/SDK/example code.

### 9.3 Build children inside the macro, not around it

Do not assemble a `Vec<Element>` outside the macro and splat it in just to populate a parent. Write children inline.

```rust
// NO — children built ad-hoc outside the macro
let mut children = Vec::new();
children.push(ui! { Foo() });
children.push(ui! { Bar() });
ui! { View() { children } }

// YES — children live where they're rendered
ui! {
    View() {
        Foo()
        Bar()
    }
}
```

`ui!` already supports `for` (flat-splat sibling iteration with a compile-time `key=` when reactive), `if` / `match` branches, and bare-identifier child splats (`children`, no braces) for the legitimate cases where the children arrive as a prop or come from data. Use those. The out-of-macro `Vec::push` loop is almost never the right tool — it defeats keyed reconciliation, hides children from the macro's reactive-scope inference, and produces awkward indentation that obscures the tree.

**The one legitimate `Vec<Element>` shape**: a container component that accepts `children: Vec<Element>` as a prop and flattens incoming fragments via `ChildList::append_to()` before splatting. See `crates/ui/idea-ui/src/components/card.rs` (`Card`) and `center.rs` (`Center`) for the canonical pattern. This is about flattening *received* children, not about authoring new ones in a push loop — the distinction matters. If you're writing the children yourself, they belong inside `ui!`.

### 9.4 Conditional and iterative rendering belongs inside the macro

If a child only sometimes appears, express that with `if` / `if let` / `match` *inside* `ui!`, not by conditionally pushing into a `Vec<Element>` before the macro call. Same for iteration: `for item in items { ... }` inside `ui!` is the standard form. The website's `pages/type_safety.rs::section()`, `pages/targets.rs::target_row()`, and `shell.rs::layout_with_toc()` helpers all currently build children with `let mut children = Vec::new(); children.push(...)` and are exactly the shape Section 9.3 forbids — when you touch one of those files for another reason, convert it.

### 9.5 Local helpers vs. promoting to a component

A snake_case `fn xyz() -> Element` is fine as a one-off, file-local helper that takes no props and is called from one place (e.g., `examples/website/src/pages/targets.rs::phones()`). The moment it starts taking parameters, gets called from a second site, or grows variants, promote it to a `#[component]` so call sites can use `ui!` struct-literal syntax (`Phones(variant = ...)`) instead of positional function arguments. Don't grow positional helper signatures — that's how `tone: ToneRef, variant: VariantRef, label: String, ...` happens.

### 9.6 Optional callbacks: bind only when present

For `Option<Rc<dyn Fn()>>` props, don't wire an unconditional closure that silently no-ops when `None`. Conditionally attach instead:

```rust
if let Some(cb) = on_press {
    bound = bound.on_press(move || (cb)());
}
```

This pattern lives in `crates/ui/idea-ui/src/components/button.rs` and `modal.rs` — match it. A silent no-op handler blocks hit-test fall-through on some backends and confuses event-routing assertions.

### 9.7 Bring code you touch up to standard

When you're editing a component (or the file it lives in) for any other reason and you notice it violates 9.1–9.6, fix it in the same change. Don't ship "half-converted" files where a refactored component sits next to a legacy one. Don't manufacture a follow-up PR for the cleanup.

The inverse also holds: don't drive-by rewrite components you aren't otherwise modifying. Other agents may have in-flight work in the same file (see [[feedback_multi_agent_coordination]]) and noise commits create rebase pain. The rule is "fix what you're already touching," not "sweep the repo."

**Known cleanup hotspots** flagged by the initial audit (drop into these when you're already in the file):

- `examples/website/src/shell.rs` — `layout_with_toc`-style helpers build TOC entries via `Vec::with_capacity` + `.push(ui!{...})`.
- `examples/website/src/pages/type_safety.rs` — `section()` helper.
- `examples/website/src/pages/targets.rs` — `target_row()` helper.
- `examples/website/src/pages/further_reading.rs` — pre-sized vec extended in a loop.
- `examples/website/src/pages/why_rust.rs` — inline `if let Some(src)` + conditional push instead of `ui!`-internal `if`.
