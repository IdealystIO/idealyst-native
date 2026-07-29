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
   `ui!` / `jsx!`, `Element`, `Bound<H>`, refs, `stylesheet!`. Read this
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

8. [`accessibility.md`](./accessibility.md). The author-facing
   accessibility guide — default roles, the `AccessibilityProps` model
   (roles, traits, live regions, actions), and how it maps to each
   platform's native AX system.
   [`accessibility-design.md`](./accessibility-design.md) has the
   internals: per-platform mapping tables, the Backend-trait surface,
   and the GPU-backend semantics tree.

9. [`server-functions.md`](./server-functions.md). The full-stack layer:
   `#[server]` fns (one function, two compilations), the `server` cargo
   feature split and what does/doesn't need cfg-gating, extractors
   (`State`/`Auth`/`Cookies`), typed `ServerError<E>`, the `DispatchHook`
   policy seam + the `server-kit` middleware/guard layer built on it, the
   cookie/bearer auth patterns, streaming (`#[subscription]` /
   `#[channel]` / `#[sse]`), schema versioning, batching, and serving.
   Read this when your app grows a backend.

## Tooling

- [`devcontainer.md`](./devcontainer.md). `idealyst configure devcontainer` —
  initialize or update a project's Dev Container and toggle idealyst-managed
  sidecar services (Postgres/MySQL, Redis, MinIO). The two-file ownership model
  (we own `docker-compose.idealyst.yml`, never your `docker-compose.yml`), the
  interactive wizard vs. the non-interactive flags, the shared `configure`
  engine behind the MCP `configure_devcontainer` tool, and how to add a service.
- [`vscode.md`](./vscode.md). `idealyst configure vscode` — set up a project's
  `.vscode/` workspace: recommend the editor extensions and wire the idealyst
  linter into rust-analyzer (inline squiggles via a generated `ra-check.sh`).
  The surgical settings/extensions merge, the aspect model, the non-interactive
  flags, and the shared MCP `configure_vscode` tool.

## Migrating

- [`migrating-to-runtime-v2.md`](./migrating-to-runtime-v2.md). The `new-core`
  runtime: staged-commit reactivity (writes commit at the driver's flush,
  `update` composes, `batch` removed), drop-as-teardown scopes and the
  tightened `on_cleanup` placement, handlers running outside the world,
  per-platform `newcore` boot entries, the not-yet-ported surface list,
  and the golden-parity guarantees.
- [`migration-0.2-to-0.3.md`](./migration-0.2-to-0.3.md). The reactive-surface
  unification: `signal!` / `memo!` macros removed (plain `signal(v)` /
  `memo(move || …)` fns), `memo` returns read-only `ReadSignal<T>`, the
  `ReadSignal` / `WriteSignal` capability halves + `.split()`, and inline
  component props. Two `sed`s cover most of it. Also catalogs the
  additive 0.3 tooling: snapshot-trap guardrails, `ui!` IDE recovery,
  `idealyst catalog-json`, the VS Code extension, and the proven
  rust-analyzer wiring.
- [`migration-0.1-to-0.2.md`](./migration-0.1-to-0.2.md). Moving navigation
  to the 0.2 model — the `swap` / `stack` primitives, the outlet + author
  layout, and the `idea-ui-nav` chrome (`TabBar` / `Drawer` / `StackHeader`).
  `tab` / `drawer` / `stack` → `swap-navigator` / `stack-navigator-v2`.

## Crate map

The repo is grouped by concern (`crates/framework/`, `crates/backend/`,
`crates/render/`, …). The crates these design docs refer to:

| Crate | Path | Role |
| --- | --- | --- |
| `runtime-core` | `crates/framework/core` | `Element`, `Backend` trait, render walker, reactivity, styles |
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
   │  declares a tree of `Element` values via `ui!` / `jsx!`
   │  + `Signal<T>` for reactive state
   │  + `StyleSheet` for styling
   ▼
Render walker  (runtime-core)
   │  recurses Element → calls Backend trait methods
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
