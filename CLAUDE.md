# Project rules for Claude agents

These rules apply to all work in this repo. Follow them unless the user explicitly overrides one in the current conversation.

## 1. Test changes — especially in framework core

Run the test suite when you make changes. Architectural changes to framework core (anything in `crates/framework/core/`, the Backend trait, reactive system, wire protocol, scene model) MUST be accompanied by tests that cover the new behavior. Framework stability is non-negotiable — a change without test coverage is incomplete.

If existing tests don't cover the area you're touching, add coverage as part of the same change. Don't merge "the tests still pass" when the tests don't actually exercise what you changed.

## 2. Keep documentation aligned

When you change behavior, find the documentation that describes it and update it in the same change. Don't leave docs lying about the new state of the world.

- `.claude/audits/` contains audit definitions that sweep the codebase for inconsistencies. Run the relevant audit (`/audit <name>` or `/audit all`) when you suspect docs may have drifted.
- New features need documentation alongside the implementation, not as a follow-up.
- `idea-ui` is an adjacent project with its own docs — do not update idea-ui docs from this repo, and don't assume changes here propagate there.

## 3. Core stays minimal — peripheral features go through External

`crates/framework/core/` is for the lowest primitives only. If you're tempted to add a feature that feels like a "widget," "helper," "convenience," or anything composable from existing primitives, build it as a third-party extension using `Primitive::External` plus the per-backend registry (see [[project_third_party_extension]]). Do not bloat core with peripheral features.

If `Primitive::External` isn't wired up yet for the surface you need, that's a signal to wire it up — not a license to add the feature directly to core.

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
