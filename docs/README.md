# idealyst-native architecture

These docs explain the design of the framework: what each layer does, why
the seams are where they are, and how to extend the system without
reaching into other layers' internals.

The framework is built around one structural decision: **structure,
reactivity, styling, and rendering are four orthogonal concerns**, each
addressable on its own. A new front-end syntax, a new primitive, a new
backend, or a new style strategy can be added without modifying any of
the others.

## Reading order

If you're new to the codebase, read the docs in this order:

1. [`ui-layer.md`](./ui-layer.md). The author-facing surface. Components,
   `ui!` / `jsx!`, `Primitive`, `Bound<H>`, refs, `stylesheet!`. Read this
   first to see what application code looks like.

2. [`primitives.md`](./primitives.md). The framework's structural
   vocabulary. The fixed set of "things the renderer knows about,"
   what each one's contract is, and how to build a component suite
   on top. The entry point if you're designing your own widget kit.

3. [`reactivity.md`](./reactivity.md). `Signal<T>`, `Effect`, `Scope`,
   the arena, fine-grained updates. The reactive substrate everything
   else assumes.

4. [`styling.md`](./styling.md). Themes, stylesheets, variants,
   overrides, interaction states. How application style declarations
   reach a backend as concrete `StyleRules`.

5. [`animation.md`](./animation.md). The gesture/spring/decay-driven
   animation system. Value handles, animator factories, the
   per-thread clock, and how the `Backend::set_animated_*` family
   carries per-frame writes to native widgets. Complements styling's
   `Transition` (declarative) with imperative, interruptible motion.

6. [`fonts.md`](./fonts.md). Bundling custom typefaces with the
   `typeface!` + `face!` macros, and how each backend turns that
   declaration into a native font registration (CoreText on iOS,
   `Typeface.createFromFile` on Android, `@font-face` on web).
   Read this when you're adding a custom font or debugging why one
   isn't rendering the weight you expected.

7. [`backend.md`](./backend.md). The `Backend` trait, the render walker,
   per-primitive lifecycle hooks, the rules a backend must follow.
   Read this last; it's where the seam between framework and platform
   lives, and it makes more sense after you've seen what gets handed
   across it.

## Crate map

The repo is grouped by concern (`crates/framework/`, `crates/backend/`,
`crates/render/`, …). The crates these design docs refer to:

| Crate | Path | Role |
| --- | --- | --- |
| `runtime-core` | `crates/framework/core` | `Primitive`, `Backend` trait, render walker, reactivity, styles |
| `runtime-macros` | `crates/framework/macros` | `#[component]`, `ui!`, `jsx!`, `stylesheet!` proc-macros |
| `reactive-arena` | `crates/framework/reactive/arena` | Arena allocator used by the reactivity system |
| `reactive-refs` | `crates/framework/reactive/refs` | `Ref<H>` machinery |
| `runtime-layout` | `crates/framework/runtime-layout` | Taffy flex-layout helper used by native backends |
| `wire` | `crates/framework/wire` | Hot-reload + server-driven UI wire protocol |
| `backend-web` | `crates/backend/web` | WASM + DOM backend |
| `backend-android-mobile` | `crates/backend/android/mobile` | JNI + Android `View` hierarchy backend |
| `backend-ios-mobile` | `crates/backend/ios/mobile` | UIKit / objc2 backend |
| `backend-macos` | `crates/backend/macos` | AppKit / objc2 backend |
| `backend-roku` | `crates/backend/roku` | BrightScript / SceneGraph generator backend |
| `render-wgpu` | `crates/render/wgpu` | wgpu-backed renderer that implements `Backend` on a GPU pipeline |

Per-backend behaviour notes live in `README.md` files next to each backend
crate. Start there if you're investigating a platform-specific quirk.

Application crates depend on `runtime-core` and the macros. They do
**not** depend on any backend; the platform host crate is the only
place that names a concrete backend.

## One-screen summary

```
Application code
   │  declares a tree of `Primitive` values via `ui!` / `jsx!`
   │  + `Signal<T>` for reactive state
   │  + `StyleSheet` for styling
   ▼
Render walker  (runtime-core)
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
