# Proposal: the `Lazy` primitive — wasm code splitting for idealyst web

Web idealyst apps ship as one wasm bundle. Every backend trait impl,
every primitive's vtable, every SDK's extension handler is reachable
through `call_indirect` and impossible for `wasm-opt` to strip. The
marketing site's bundle is 13 MB uncompressed (~3 MB gzipped) and 74%
of those bytes are the function table alone — and that's a marketing
site with a single embedded GPU preview. Any real product (admin
panels, settings pages, editors, demos) will hit this wall harder.

The `Lazy` primitive lets authors declare boundaries in the UI tree
that compile into separate wasm chunks, loaded on demand. On native
platforms it's a no-op — the chunk crate is a normal cargo dep and
its content is mounted inline.

> Status: proposal. Not implemented.

---

## Motivation

Concrete from the marketing site:

| Component | Forces in deps | Bundle impact |
|---|---|---|
| Embedded `Simulator` on `/` | `host-web` + `ios-sim` + `android-sim` + `render-api` + `render-wgpu` + `welcome` | ~10 MB |
| Everything else | runtime, idea-ui, drawer-navigator, idea-codeblock, etc. | ~3 MB |

The Simulator is a visual asset shown on one route. With code
splitting, the main bundle drops to ~3 MB and the simulator wasm
loads in the background (or on first scroll into view) without
blocking initial paint.

This generalizes:

- Admin route loaded only when the user navigates to `/admin`
- Rich text editor loaded when the user opens "compose"
- Map view loaded when the "map" tab is selected
- Heavy data viz loaded when a dashboard widget mounts

The framework currently has no answer for any of these.

---

## Surface

### Author API

```rust
use runtime_core::primitives::lazy::{lazy, LazyState};
use runtime_core::ui;

// `simulator_chunk` is the chunk crate — an idealyst app crate
// declared in the project manifest under [package.metadata.idealyst.chunks].
// It exports `pub fn app(props: SimulatorProps) -> Primitive`.
use simulator_chunk::SimulatorProps;

ui! {
    Lazy::<SimulatorProps>("simulator", SimulatorProps {
        skin: skin_kind,
        device: device_profile,
    })
    .on_state(move |s| state_signal.set(s))
    .placeholder(|| ui! { Spinner {} })
}
```

The first type argument names the chunk's prop type — it must match
what the chunk crate's `app(props: T)` declares. The string
identifier (`"simulator"`) maps to a chunk URL the build pipeline
emits (`/pkg-simulator/simulator_chunk.js`).

### `LazyState` lifecycle

```rust
pub enum LazyState {
    /// Module fetch in flight. Web only — never observed on native.
    Loading,
    /// Module fetched + instantiated, mount call hasn't returned.
    /// Brief window; some authors won't distinguish from Loading.
    Loaded,
    /// Mount succeeded, UI is visible.
    Rendered,
    /// Fetch or mount failed. String is the underlying error.
    Error(String),
}
```

Author can react to `LazyState` via the `on_state` callback. The
state callback fires `Rendered` immediately on native — there's no
loading because the chunk was compiled in.

Typical placeholder pattern:

```rust
let state = signal(LazyState::Loading);
ui! {
    View {
        // Author owns the loading/error UI. The framework's
        // placeholder slot is only the immediate fallback before
        // the on_state callback fires.
        Switch(key = move || matches!(state.get(),
            LazyState::Loading | LazyState::Loaded)) {
            true => Spinner { },
            false => view!(empty),
        }
        Switch(key = move || matches!(state.get(), LazyState::Error(_))) {
            true => Text { "Failed to load simulator" },
            false => view!(empty),
        }
        Lazy::<SimulatorProps>("simulator", props)
            .on_state(move |s| state.set(s))
    }
}
```

### Constructor

```rust
pub fn lazy<T: serde::Serialize + 'static>(
    chunk: &'static str,
    props: T,
) -> LazyBuilder<T>;

pub struct LazyBuilder<T> { /* ... */ }

impl<T> LazyBuilder<T> {
    pub fn on_state(self, f: impl Fn(LazyState) + 'static) -> Self;
    pub fn placeholder(self, build: impl Fn() -> Primitive + 'static) -> Self;
    pub fn with_style(self, style: impl IntoStyleSource) -> Self;
}

impl<T> IntoPrimitive for LazyBuilder<T> { /* emits Primitive::Lazy */ }
```

Props bound: `serde::Serialize` because they cross a wasm boundary.
For native targets the bound is harmless — serde-able types are also
plain Rust types, the framework just calls the chunk crate directly.

---

## Per-platform semantics

### Web

1. Lazy primitive mounts. Backend creates a placeholder DOM node
   (a styled `<div>`). State callback fires `Loading`.
2. Backend looks up the chunk URL from a registry installed at
   bootstrap (the build pipeline injects this — see Build below).
3. JS dynamic `import("/pkg-simulator/simulator_chunk.js")`. State
   callback fires `Loaded` when the wasm-bindgen-generated module
   finishes instantiating its wasm.
4. Backend calls the chunk's exported
   `mount_chunk(elem_id: &str, props_json: JsValue) -> u32` with the
   placeholder's id and the serialized props. The chunk's wrapper
   creates its OWN `WebBackend` rooted at that elem and mounts
   `chunk_crate::app(props)`. Returns an integer handle.
5. State callback fires `Rendered`.
6. On unmount (parent's reactive scope drops the lazy node):
   backend calls the chunk's `unmount_chunk(handle)`, which drops
   the chunk's `Owner` (tearing down its reactive graph) and
   detaches the rooted backend.

Each chunk loads at most once per page lifetime (its wasm-bindgen
module is cached by the browser's module map). Two mount sites for
the same chunk share the loaded module but get separate
`mount_chunk` instances — separate `WebBackend`s rooted at separate
DOM nodes, separate reactive graphs.

### Native (iOS, Android, macOS, terminal, etc.)

The chunk crate is a target-conditional cargo dep on the parent
app crate:

```toml
# In the parent project's Cargo.toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
simulator-chunk = { path = "../simulator-chunk" }
```

The framework's native backend handlers for `Primitive::Lazy` call
`<chunk_crate>::app(props)` directly and render the resulting
primitive inline. State callback fires `Rendered` immediately. No
state machine, no loading, no error path (compile-time guarantees
the chunk crate is present).

A typed thunk needs to bridge the dynamic chunk name + props back
to the static cargo dep call. Two options:

- **A. Codegen via a build script.** The CLI generates a `chunks.rs`
  in the parent crate listing each chunk → `Fn(props) -> Primitive`.
  Backend dispatches by chunk name.
- **B. A static registry the author populates.** At bootstrap:
  `lazy_registry::register("simulator", |props| simulator_chunk::app(props))`.
  Backend dispatches via this registry on native.

B is simpler and explicit, matches existing `register_external`
patterns. A is more ergonomic but introduces codegen.

### Roku and other generator backends

Generator backends don't compose dyn dispatch through wasm at all;
they emit wire commands the device-side runtime replays. Lazy on
these targets is the same as native — chunks are normal deps,
rendered inline at snapshot time. The wire shape doesn't change.

---

## Cross-boundary state

This is the hard question and deserves its own section. The honest
answer: **two wasm modules cannot share state**. Each has its own
heap, its own globals, its own reactive arena. A `Signal<T>` is a
pointer into module A's arena; module B literally can't dereference
it. The boundary is a serialization seam, exactly like a network
call.

The protocol layer above that constraint can recover most of what
authors want. Three patterns, increasing capability:

### v1 — props snapshot + update re-entry (initial implementation)

The chunk's `app(props: T)` receives a serialized snapshot at
mount. To react to parent state changes:

```rust
// Parent calls this when its observed signals change.
// The build pipeline emits a typed wrapper around it.
chunk.update(new_props);
```

The chunk's wrapper exposes a `update_chunk(handle, props_json)`
JS-callable function. The parent's backend handler installs an
Effect that re-serializes `props` and calls `update_chunk` whenever
the closure's captured signals change. The chunk crate exposes a
re-entry function that re-runs its `app(props)` against the new
props — typically by storing a `Signal<Props>` at mount time and
calling `.set(new_props)` on update.

Honest, simple, no magic. Covers 90% of cases.

### v2 — bridged signals (later, additive)

For ambient state (theme, current user, current route), prop
drilling is annoying. v2 introduces `BridgedSignal<T>`:

```rust
// In the chunk crate, looks identical to a normal Signal:
let theme: BridgedSignal<Theme> = bridge::read_context("theme");
let current = theme.get();
```

Internally, `BridgedSignal::get` makes a JS call to the parent's
exported `bridge_get(context_key) -> JsValue` and deserializes.
`.set()` is similar. Subscription is done via a callback the parent
registers; when the parent's underlying signal changes, the
callback fires across the bridge, the chunk's local mirror updates,
and downstream effects re-run normally.

The parent declares contexts at boot:

```rust
// In the parent app's start():
bridge::register_context("theme", theme_signal);
bridge::register_context("user", current_user_signal);
```

JS bridge cost is microseconds per get/set — fine for low-frequency
state, terrible for per-frame animation. The latter shouldn't cross
boundaries anyway.

v2 is purely additive: v1 apps keep working, v2 makes ambient
context less verbose.

### v3 — bridged context registry (later, if needed)

React-Context-shaped ambient: `register_context(key, signal)` from
the parent, `read_context::<T>(key)` from the chunk. Built on top of
v2's bridge primitives. Sketched here but not part of this proposal.

### Why this discipline is good

It forces components to **not pretend** they share state. A
boundary crossing is visible in the type system (props are
`Serialize`, signals are `BridgedSignal`) and in the API surface
(`update()` is an explicit re-entry, not a hidden side effect of a
write). Pretending otherwise — making cross-module signal access
implicit — leads to subtle bugs where a write in one module
silently fails to propagate to readers in another.

---

## Build pipeline

### Chunk declaration

The parent project's Cargo manifest declares chunks under
`[package.metadata.idealyst]`:

```toml
[package.metadata.idealyst.chunks]
simulator = { path = "../simulator-chunk" }
admin = { path = "../admin-chunk" }
```

Each value is a path to a sibling idealyst app crate. The chunk
crate exposes the standard `pub fn app(props: T) -> Primitive`
signature; the framework's wrapper template knows how to wrap it.

### Wrapper template variants

`build-web` currently emits one wrapper per project — a `cdylib`
with `#[wasm_bindgen(start)]` that mounts `crate::app()` to `#app`.
For chunks, the wrapper template grows a second mode:

- **Main mode (today's behavior).** `start()` mounts `app()` to
  `#app` on page load.
- **Chunk mode (new).** No `start()`. Exports:
  - `mount_chunk(elem_id: &str, props_json: JsValue) -> u32` —
    creates a `WebBackend` rooted at `elem_id`, deserializes
    `props`, calls `chunk_crate::app(props)`, mounts, returns a
    handle.
  - `update_chunk(handle: u32, props_json: JsValue)` — re-runs the
    chunk's update path against new props.
  - `unmount_chunk(handle: u32)` — drops the `Owner` keyed by
    `handle`.

The wrapper template picks the mode based on a build flag the CLI
passes.

### CLI

`idealyst build --web` walks the chunks declared in the manifest
and runs `build-web::build` for each, writing into
`<bundle>/pkg-<name>/`. The main bundle gets a generated
`chunks.js` (or inlines a `__IDEALYST_CHUNKS__` map) listing each
chunk name → URL so the web backend can resolve at runtime.

```
dist/
├── index.html
├── pkg/                  # main bundle
│   ├── website.js
│   └── website_bg.wasm
├── pkg-simulator/        # chunk
│   ├── simulator_chunk.js
│   └── simulator_chunk_bg.wasm
└── pkg-admin/            # chunk
    ├── admin_chunk.js
    └── admin_chunk_bg.wasm
```

### Filename hashing

Chunks are content-addressable for cache busting. The build emits
hashed filenames (e.g. `simulator_chunk_a3b8.js`) and the runtime
chunk registry resolves the logical name to the hashed URL. This
is a cache-correctness requirement, not an optional feature —
without it a deploy of an updated chunk will be invisible to users
with the old URL cached.

For v1 we can emit a `chunk_manifest.json` next to the main bundle
that maps logical → hashed name; the runtime fetches it lazily on
first `Lazy` use.

---

## Implementation sketch

### Framework core

New variant on `Primitive` (in `crates/runtime/core/src/primitive.rs`):

```rust
Lazy {
    /// Logical chunk identifier. Maps to a URL via the per-backend
    /// chunk registry on web; on native, maps to a registered
    /// `Fn(payload) -> Primitive` thunk.
    chunk: &'static str,
    /// Type-erased props. The web backend serializes via
    /// `serde_json::to_string(payload.downcast_ref::<T>())`; the
    /// native backend hands `payload` to the registered thunk.
    type_id: std::any::TypeId,
    type_name: &'static str,
    payload: Rc<dyn Any>,
    /// Serializer + re-entry hook. Generated by the constructor
    /// macro so the framework doesn't need to know T at runtime.
    bridge: LazyBridge,
    /// Reactive state callback. Fires for each state transition.
    on_state: Option<Rc<dyn Fn(LazyState)>>,
    /// Placeholder primitive shown immediately on mount, before
    /// the chunk has a chance to fire `Rendered`. `None` is fine
    /// — backends render an empty div.
    placeholder: Option<Box<dyn Fn() -> Primitive>>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    accessibility: AccessibilityProps,
}

pub struct LazyBridge {
    /// JSON-serialize the type-erased payload for cross-boundary
    /// transport. Generated by `lazy::<T>(...)` from `T: Serialize`.
    serialize: Box<dyn Fn(&dyn Any) -> serde_json::Result<String>>,
}
```

The walker treats `Lazy` analogously to `External` / `Navigator`:
hand it to the backend, which dispatches via its own registry.

### Web backend handler

New trait method (or new entry in the existing `External`-style
registry):

```rust
impl WebBackend {
    fn handle_lazy(&mut self, props: Primitive::Lazy<…>) -> NodeId {
        // 1. Create placeholder div, mount it
        // 2. Fire on_state(Loading)
        // 3. dynamic import via wasm-bindgen-glue
        // 4. Fire on_state(Loaded) on resolve
        // 5. Call chunk's mount_chunk(elem_id, props_json)
        // 6. Fire on_state(Rendered)
        // 7. Store the chunk handle in the node's userdata for unmount
    }
}
```

### Native backend handlers

```rust
// One-time at app bootstrap, equivalent of register_external:
backend.register_lazy("simulator", |payload| {
    let props = payload.downcast::<SimulatorProps>().unwrap();
    simulator_chunk::app((*props).clone())
});
```

Handler is a simple thunk; no async, no state machine.

### Bridge primitives (v2, deferred)

Out of scope for the v1 implementation. Documented here to confirm
v1's design doesn't lock us out.

---

## Open questions

1. **Identifier type.** String name (`"simulator"`) is cheapest;
   compile-time-checked marker type prevents typos. Suggest string
   for v1, with a future macro that emits typed markers from the
   manifest.
2. **Sharing a chunk across multiple mount sites.** One wasm instance
   per chunk per page, multiple mount calls — confirmed feasible
   per wasm-bindgen module semantics. Verify before commit.
3. **Hot reload.** Dev mode currently rebuilds + reloads the whole
   bundle. Chunks should rebuild independently and the page should
   only reload the affected chunk. Reasonable but adds complexity
   to the dev loop; OK to ship v1 with "any chunk change reloads
   everything" and refine.
4. **Mobile bundle size impact.** On native, the chunk crate is a
   normal dep — the bundle includes everything. We should still
   measure: does compiling Simulator into the native app's binary
   inflate it meaningfully? Probably not (no wgpu on mobile native
   target — the chunk's heavy deps are wasm-gated), but worth a
   look.
5. **What if the chunk crate uses primitives the parent doesn't?**
   Each backend owns its own primitive registry (External /
   Navigator handlers). A chunk that uses, say, the `maps` SDK
   needs `maps::register(&mut backend)` called in the CHUNK's
   bootstrap, not the parent's. Each chunk's wrapper has its own
   `start_chunk_internals()` where it does this. Author code: the
   chunk crate exposes `pub fn register_extensions(&mut WebBackend)`
   same as a main crate does.
6. **`Send + Sync` bounds on the chunk thunk.** The native
   registry holds `Box<dyn Fn(Rc<dyn Any>) -> Primitive>`. The
   `Rc` makes it `!Send`. That's fine — the registry is
   thread-local. State this explicitly in the API doc.

---

## First user — migration of the website Simulator

Concrete plan once the primitive lands:

1. Create `examples/website-simulator/` as a new workspace member.
2. Move `host-web`, `ios-sim`, `android-sim`, `render-api`,
   `welcome` deps out of `examples/website/Cargo.toml` and into
   `examples/website-simulator/Cargo.toml`.
3. Move the current `Simulator` component's wgpu plumbing into
   `website_simulator::app(props: SimulatorProps) -> Primitive`.
4. Rewrite `examples/website/src/components/simulator.rs` to
   construct `lazy::<SimulatorProps>("simulator", props)` with a
   placeholder + on_state callback.
5. Declare the chunk in `examples/website/Cargo.toml`:
   ```toml
   [package.metadata.idealyst.chunks]
   simulator = { path = "../website-simulator" }
   ```
6. `idealyst build --web --release` produces both bundles.

Expected outcome: main bundle ~3 MB, simulator chunk ~10 MB,
loaded only when the home page renders. Time-to-interactive on
every non-home page drops by ~3x.

---

## Out of scope for this proposal

- **Implicit / automatic code splitting.** Author declares all
  boundaries explicitly. Third-party UI frameworks can layer
  implicit splitting on top later if the patterns emerge.
- **Inter-chunk communication.** A chunk can talk to the parent
  (props in, events out via callbacks); two sibling chunks do not
  talk directly. If two chunks need to coordinate, parent-mediated
  state is the answer.
- **Streaming / progressive chunk download.** Chunks are loaded
  whole. Smaller-than-chunk splitting is a wasm-bindgen
  `split-linked-modules` problem we defer to that toolchain
  maturing.
- **`Lazy` inside a `Lazy`.** Nested chunks (chunk A loads chunk
  B) should work transparently — chunk B's `Primitive::Lazy` goes
  through chunk A's backend. Not exercised in v1; if it breaks,
  fix when needed.
- **Server-side rendering.** Different problem; doesn't reduce
  bundle size, reduces time-to-paint.

---

## Decision points needed before implementation

1. **Identifier scheme** — string name or typed marker for v1?
   (Suggest string; lower friction.)
2. **Native dispatch** — `register_lazy(name, thunk)` registry or
   codegen-emitted `chunks.rs`?
   (Suggest registry; matches existing patterns.)
3. **Chunk manifest format** — `[package.metadata.idealyst.chunks]`
   inline, or a separate `chunks.toml`?
   (Suggest inline; one less file.)
4. **Where does `LazyState` live?** A new
   `crates/runtime/core/src/primitives/lazy.rs` module, sibling to
   `link.rs` / `portal.rs`.
5. **Confirm bundle filename hashing requirement** — is it OK to
   defer to v1.1 with a known cache-correctness bug, or must v1
   ship with hashed filenames?
   (Suggest deferring; document the gotcha.)
