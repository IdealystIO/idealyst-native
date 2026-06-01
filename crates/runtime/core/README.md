# runtime-core

The framework's structural foundation: primitive vocabulary, the `Backend`
trait, the render walker, reactivity, and the styling model.

Everything else in the workspace is either layered on top of this crate
(`runtime-macros`, `idea-ui`, the backends) or routed through it (host
crates, `dev-client`, `wire`). Application code depends only on
`runtime-core` plus the macros. Backends are named in the host crate, not
in app code.

## What this crate owns

- **`Element` enum**. The closed set of "things the render walker knows
  how to walk." `View`, `Text`, `Button`, `Image`, `TextInput`, `ScrollView`,
  `Slider`, `Toggle`, `Icon`, `ActivityIndicator`, `Virtualizer`,
  `Graphics`, `Link`, `When`, `Portal`, `Presence`, `External`, navigators.
  Adding a new primitive means a new variant plus the `Backend` hooks that
  go with it.
- **`Backend` trait** (`crates/framework/core/src/backend.rs`). The seam
  every platform implements. The trait has one required method per primitive
  (`create_view`, `create_text`, `create_button`, `insert`, `apply_style`,
  `finish`, …) plus a long tail of optional hooks (image src, scroll-view
  variants, navigator chrome, `set_animated_*`, etc.). Optional hooks
  default to `unimplemented!()` so a backend that hasn't wired a primitive
  panics loudly rather than silently no-oping. See `docs/backend.md` for the
  long version.
- **Render walker** (`walker.rs`). Recurses the `Element` tree, calls the
  backend trait, and wires per-node `Effect`s so signal changes drive the
  smallest possible backend update. No virtual DOM.
- **Reactivity** (`reactive.rs` + `sources.rs`). `Signal<T>`, `Effect`,
  `Scope`, `Memo`, `Computed`. Fine-grained: a signal change re-runs only
  effects that read it. The arena that backs the scope graph lives in
  [`reactive-arena`](../reactive/arena); the `Ref<H>` machinery lives in
  [`reactive-refs`](../reactive/refs).
- **Styles + theming** (`style.rs`). `StyleRules`, the `stylesheet!`
  representation, theme tokens (`install_tokens` / `Tokenized<T>` / token
  signals). The author-facing macro is in `runtime-macros`; this crate
  owns the data shapes the backend sees.
- **Animation core** (`animation/`). `AnimatedValue<T>`, spring/decay
  drivers, the per-thread clock. Backends implement `set_animated_f32` /
  `set_animated_color` to receive per-frame writes; backends that skip
  those overrides silently drop animated writes (see
  `project_wgpu_viewops_animated` in memory).
- **Mount entry point**: `runtime_core::mount(backend, app)`. **Host
  crates must call `mount`, not `render`.** `render` builds the tree first
  and cancels effects/timers before they can fire. See
  `project_mount_vs_render` in memory.

## Cargo features

- **`async-driver`**. Pulls in the cross-platform per-frame driver +
  `resource()` async-data primitive. Off by default.
- **`debug-stats`**. Enables the `debug` module's thread-local phase
  counters. Used by backend `PhaseTimer` calls (see project CLAUDE.md §6).
- **`robot`**. Enables the [`robot::`](./src/robot) module: the
  introspection registry that exposes every mounted primitive (`test_id`,
  label, kind, control handle) plus the component-method registry that
  `#[component] methods!` populates. The `idealyst mcp` command
  ([`mcp-server`](../../mcp/server)) turns this into MCP tools agents can
  drive. Off by default; production builds shouldn't pay for the per-node
  registry overhead.
- **`hot-reload`**. Wires the `dev-hot` substrate into the walker
  (catches `HotFnPanic` at the render boundary).

## Known gaps in the trait surface

- **Accessibility.** Image carries an `accessibilityLabel`. Link declares an
  accessibility role on native. The identity layer mints stable IDs intended
  for `aria-labelledby`. There is **no** generalised `AccessibilityProps`,
  no `set_accessibility_label` / `set_role` / `announce_for_accessibility`,
  no focus-order plumbing. Production apps will need this. See the root
  README's roadmap.
- **Optional vs required Backend methods.** Optional trait methods default
  to `unimplemented!()`. This silently expands the backend surface area
  without the type system tracking which capabilities a backend has actually
  implemented. A `RequiredBackend` + `OptionalBackend` split (or
  capability-flag types associated to method clusters) would make
  completeness checkable at compile time rather than panicking at runtime.
  See [TODO.md](../../../TODO.md).

## Cross-crate contracts

- A backend depending on this crate **must** implement every required trait
  method. Optional methods default to `unimplemented!()` so missing
  primitives panic instead of silently no-oping.
- Animated property writes (`set_animated_f32`, `set_animated_color`) are
  delivered via `Ops` traits on handle types. Backends that need animation
  to work **must** override the corresponding `ViewOps`/`TextOps` methods;
  the trait defaults are silent no-ops.
- `Element::External` carries a `kind: &'static str` and a `props: Vec<…>`.
  Per-backend `ExternalRegistry` instances resolve the kind to a renderer.
  Third-party SDKs (Maps, WebView) follow this pattern; see
  `project_third_party_extension` in memory.

## Where to read more

- `docs/primitives.md`: the primitive vocabulary as a design surface.
- `docs/backend.md`: the `Backend` trait contract and render-walker rules.
- `docs/reactivity.md`: the signal / effect / scope model.
- `docs/styling.md`: themes, stylesheets, variants, overrides.
- `docs/animation.md`: value handles, animator factories, the per-thread
  clock.

The tests under `crates/framework/core/tests/` exercise the walker against
a synthetic `Backend` impl. Read those to see end-to-end behaviour without
a real platform in the picture.
