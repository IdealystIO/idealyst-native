# idealyst-native architecture

These docs explain the design of the framework — what each layer does, why
the seams are where they are, and how to extend the system without
reaching into other layers' internals.

The framework is built around one structural decision: **structure,
reactivity, styling, and rendering are four orthogonal concerns**, each
addressable on its own. A new front-end syntax, a new primitive, a new
backend, or a new style strategy can be added without modifying any of
the others.

## Reading order

If you're new to the codebase, read the docs in this order:

1. [`ui-layer.md`](./ui-layer.md) — the author-facing surface. Components,
   `ui!` / `jsx!`, `Primitive`, `Bound<H>`, refs, `stylesheet!`. Read this
   first to see what application code looks like.

2. [`primitives.md`](./primitives.md) — the framework's structural
   vocabulary. The fixed set of "things the renderer knows about,"
   what each one's contract is, and how to build a component suite
   on top. The entry point if you're designing your own widget kit.

3. [`reactivity.md`](./reactivity.md) — `Signal<T>`, `Effect`, `Scope`,
   the arena, fine-grained updates. The reactive substrate everything
   else assumes.

4. [`styling.md`](./styling.md) — themes, stylesheets, variants,
   overrides, interaction states. How application style declarations
   reach a backend as concrete `StyleRules`.

5. [`animation.md`](./animation.md) — the gesture/spring/decay-driven
   animation system. Value handles, animator factories, the
   per-thread clock, and how the `Backend::set_animated_*` family
   carries per-frame writes to native widgets. Complements styling's
   `Transition` (declarative) with imperative, interruptible motion.

6. [`backend.md`](./backend.md) — the `Backend` trait, the render walker,
   per-primitive lifecycle hooks, the rules a backend must follow.
   Read this last — it's where the seam between framework and platform
   lives, and it makes more sense after you've seen what gets handed
   across it.

## Crate map

| Crate | Role |
| --- | --- |
| `framework-core` | `Primitive`, `Backend` trait, render walker, reactivity, styles |
| `framework-macros` | `#[component]`, `ui!`, `jsx!`, `stylesheet!` proc-macros |
| `reactive-arena` | Arena allocator used by the reactivity system |
| `reactive-refs` | `Ref<H>` machinery |
| `backend-web` | WASM + DOM backend |
| `backend-android` | JNI + Android `View` hierarchy backend |
| `backend-ios` | UIKit / objc2 backend (compile-only spike) |

Application crates depend on `framework-core` and the macros. They do
**not** depend on any backend — the platform host crate is the only
place that names a concrete backend.

## One-screen summary

```
Application code
   │  declares a tree of `Primitive` values via `ui!` / `jsx!`
   │  + `Signal<T>` for reactive state
   │  + `StyleSheet` for styling
   ▼
Render walker  (framework-core)
   │  recurses Primitive → calls Backend trait methods
   │  + wires Effects so signal changes drive backend updates
   │  + resolves StyleSheets against active theme into StyleRules
   ▼
Backend  (your platform impl)
   │  creates / inserts / updates native widgets
   │  + applies StyleRules however suits the platform
   │  + (optionally) caches stylesheet state, exposes ref handles
   ▼
Native UI on screen
```

The framework controls **what** to render and **when** to update.
The backend controls **how** that happens on the target platform.
