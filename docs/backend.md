# The backend layer

A backend is the platform-specific renderer. It implements one small
structural trait plus the capability traits for the primitives it
supports, and the framework knows nothing else about it. This is the
seam that makes the same application code run on the DOM, on
`android.view.View`s, on UIKit, on AppKit, on a wgpu pipeline, or on
anything else you can drive from Rust.

This doc covers the contract a backend implements, the guarantees it can
rely on, and the lifecycle rules that keep things from blowing up at
teardown.

Implementation:

- `runtime_vocabulary::backend` — **the single import a backend needs**
  (`crates/runtime/vocabulary/src/backend.rs`); everything below is
  re-exported from it.
- `runtime_scene::Host` — the structural seam
  (`crates/runtime/scene/src/host.rs`).
- `runtime_vocabulary::caps::*` — 30 per-primitive capability traits
  (`crates/runtime/vocabulary/src/caps/`), with
  `crates/runtime/vocabulary/COVERAGE.md` as the trait-by-trait map.
- `runtime_scene::Registry` + `realize` — the mount path
  (`crates/runtime/scene/src/{registry,realize}.rs`).
- The backend's own `newcore` module — boot entry + flush driver (e.g.
  `crates/backend/web/src/newcore.rs`).

---

## The seam, in three parts

### 1. `Host` — seven structural operations

```rust
pub trait Host: 'static {
    type Node: Clone + 'static;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);
    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) { /* per-child default */ }
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize);
    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node);
    fn clear_children(&mut self, node: &Self::Node);
    fn create_anchor(&mut self) -> Self::Node;
    fn supports_splice(&self) -> bool;
}
```

That is the *entire* structural interface. The scene's drivers — reactive
holes, keyed lists, fragments — emit nothing else. Two of the seven are
worth calling out:

- **`create_anchor`** returns a layout-transparent container the anchored
  drivers swap subtrees under (`display: contents` on web; a plain view
  elsewhere).
- **`supports_splice`** answers "can this host splice children directly
  into a real parent (`remove_child` + `insert_at`)?" `true` → style-less
  reactive regions go anchorless; `false` → every reactive region nests
  under a `create_anchor` node. SSR, web hydration, and splice-incapable
  native hosts return `false`.

`Self::Node: Clone` is the only constraint on the node type, and `Clone`
is required because structural regions retain node handles across effect
fires — a spliced region must `remove_child` the exact nodes it inserted.
Backends use a cheap-to-clone wrapper (`web_sys::Node` is already a
refcount; `GlobalRef` wraps a JVM ref-counted handle).

### 2. Capability traits — one per primitive family

Everything else a platform does — creating primitives, setting props,
styling, wiring events, minting imperative handles — lives in the
`runtime_vocabulary::caps` traits. Each is a subtrait of `Host`, so they
share the `Node` type and the structural ops:

| Trait group | Traits |
|---|---|
| Environment / lifecycle | `AppEnvOps`, `LifecycleOps` |
| Containers + input | `ViewOps`, `InputOps`, `PressableOps`, `ScrollOps`, `SafeAreaOps` |
| Text | `TextOps`, `ButtonOps` |
| Media | `ImageOps`, `IconOps`, `LinkOps` |
| Widgets | `TextInputOps`, `ToggleOps`, `SliderOps`, `ActivityIndicatorOps` |
| Structure-bearing prims | `VirtualizerOps`, `GraphicsOps`, `PortalOps`, `PresenceOps`, `NavigatorOps` |
| Styling + assets | `StyleOps`, `AssetOps`, `DocumentOps` |
| Cross-cutting | `A11yOps`, `AnimationOps`, `IntrospectionOps`, `BatchOps`, `WireBindingOps`, `ExternalOps` |

`caps::AllCaps` is the umbrella: a blanket impl makes any type
implementing all 30 traits `AllCaps` automatically, and
`register_builtins::<H: AllCaps>()` bounds on it
(`crates/runtime/vocabulary/src/caps/mod.rs`).

**A backend can ship incrementally**, because the traits carry the same
frozen defaults the old mega-trait did, and the default target is
declared as a supertrait so the delegation is visible in the type system
(`COVERAGE.md` § "Judgment calls on defaults"):

- **Placeholder defaults** — `create_image`, `create_icon`,
  `create_text_input`, `create_text_area`, `create_toggle`,
  `create_slider`, `create_activity_indicator`, `create_virtualizer`,
  `create_graphics`, `create_portal`, `create_navigator` all default to
  `ExternalOps::missing_primitive_placeholder`, which is why those traits
  declare `: ExternalOps`.
- **Container degradation** — `create_pressable`, `create_link`,
  `create_presence_placeholder`, `create_element` default to a plain
  container, hence `: ViewOps`.
- **Lowering defaults** — `create_styled_text` / `update_styled_text`
  concatenate runs into plain text; `update_button_label` lowers to
  `update_text` (hence `ButtonOps: TextOps`);
  `apply_styled_variants` → `apply_styled_states` → `apply_style`;
  `apply_scroll_view_safe_area_inset` → `apply_safe_area_padding`;
  `execute_batch_with_attach` → `execute_batch` + `Host::insert_many`.
- **Opt-in fast paths** report `false` / `None` by default:
  `create_text_with_id`, `supports_js_text_bindings`,
  `supports_js_class_bindings`, `supports_preminted_styles`,
  `supports_batched_repeat`, `supports_native_introspection`,
  `supports_screenshot`, `handles_states_natively`.

So a backend that implements `Host`, `create_view`, `create_text`,
`update_text`, `create_button`, `apply_style`, and `finish` runs a
non-trivial app; every other primitive degrades to a placeholder or a
container rather than failing to compile.

### 3. `Registry` — the mount path

The scene never interprets a primitive. It carries type-erased payloads
and dispatches each one to a handler registered by `TypeId`
(`crates/runtime/scene/src/registry.rs`):

```rust
registry.register::<MyPayload, _>(|cx, payload: &Rc<MyPayload>, children| -> H::Node {
    // create → bind props → realize children → attach handlers
});
```

Three consequences:

- **First-party and third-party primitives use the same contract.** The
  old core's `Element::External` concept is gone; a "third-party
  primitive" is just a payload type whose handler ships outside the
  framework. See [`external-export.md`](external-export.md).
- **TypeId keying is collision-free by construction** — two crates' own
  `MapViewProps` types have distinct TypeIds, where a string-keyed
  registry would conflict.
- **An unregistered payload panics at realize**:
  `"runtime-scene: no handler registered for item payload (…) — register
  one on this backend's Registry before realizing"`
  (`crates/runtime/scene/src/realize.rs::mount_item`). A missed
  registration fails loud; the old core rendered a placeholder box.

The framework's own handlers install through one call —
`runtime_vocabulary::handlers::register_builtins(&mut registry)`: the
leaf primitives (`view`, `text`, `button`, `pressable`, `image`, `icon`,
`toggle`, `slider`, `activity_indicator`, `link`, `scroll_view`,
`text_input`, `text_area`) plus `virtualizer`, `graphics`, `portal`
(which also serves `overlay` / `anchored_overlay`), `presence`, the
navigators, `repeat`, and `lazy`
(`crates/runtime/vocabulary/src/handlers/mod.rs`).

---

## The mount walk

`runtime_scene::realize(backend, registry, element)` is the entry point.
It walks the `Element` tree once, delegates every `Item` to its handler,
and spawns one driver effect per structural hole. It must run with the
owning `World` ambient, and the walk itself runs **untracked** — a mount
is a snapshot; reactive regions subscribe through their own driver
effects.

A handler receives a `MountCx` and drives the sequence itself
(`crates/runtime/vocabulary/src/handlers/view.rs::mount_view` is the
reference shape):

```rust
let backend = cx.backend().clone();                    // Rc<RefCell<H>>
let mut node = backend.borrow_mut().create_view(&prim.a11y);
if prim.is_container { backend.borrow_mut().mark_container(&node); }
cx.realize_children_into(&mut node, children);          // recurse
if let Some(style) = prim.style { attach_style(&backend, &node, style); }
if let Some(h) = prim.on_touch { backend.borrow_mut().install_touch_handler(&node, h); }
// …ref-fill…
node
```

`MountCx` exposes exactly what a handler needs: `backend()`,
`registry()`, `realize_children_into(parent, children)` (children land in
this handler's frame and become the item's live children),
`realized_children()`, `realize_detached(element)` (a navigator screen or
a keyed row root), and `realize_in_place(element)`.

Two structural properties carry over from the old walker:

### Everything created during a realize is owned by that subtree

Every realization — initial mount, driver rebuild, keyed row — runs
inside one `collect_owned` scope, and the element **producer** (a branch
builder, a row `render`) runs inside it too
(`crates/runtime/scene/src/realize.rs::build_realized`). Dropping the
resulting `Realized` IS unmount: cleanups fire, effects retire, slots
free. There is no separate dispose call, and no way for a driver rebuild
to leak an effect into the world root.

### Updates flow through binding effects, not re-render

A reactive prop becomes a binding effect that calls the matching
capability method:

```rust
// The shape of `bind_value` / `bind_dyn` (crate-internal helpers in
// crates/runtime/vocabulary/src/handlers/mod.rs):
match value {
    Value::Const(v) => apply(&v),                    // no effect at all
    Value::Dyn(f)   => { let _ = effect(move || apply(&f())); }
}
```

The effect runs once for the initial value, then re-runs when the
signals it read commit a change. Each re-run is one capability call.
**The native widget exists once and is mutated in place** — no diff, no
virtual DOM, no re-render pass. A *constant* prop creates no effect at
all (`runtime_world`'s `Value::Const` arm;
`tests.rs::const_values_bind_once_with_no_effect`).

---

## Booting a backend

Each backend ships a `newcore` module whose entry has the shape
*(register, build)*: `register` runs after `register_builtins` and lets
the app add its own primitive handlers; `build` is the root component
call. Variants without `register` exist for the common case. The full
per-platform table lives in
[`migrating-to-runtime-v2.md` § Booting](migrating-to-runtime-v2.md#booting);
web is the reference:

```rust
fn main() {
    backend_web::newcore::start(|| app());
}
```

Under the hood `start_in` creates the backend and `Registry`, calls
`register_builtins` + the app seam, creates a `World`, realizes inside
`world.enter(…)`, hands the root node to `finish`, installs the flush
driver and the viewport source, and retains all of it in a thread-local
for the page's lifetime (`crates/backend/web/src/newcore.rs`). The
retained struct's field order is its drop order: the `Realized` unmounts
before the `World` that owns its slots dies.

### `runtime_vocabulary::backend` — the one import a backend needs

Everything above is reachable from a single module. `runtime_vocabulary::backend`
gathers the three layers a backend spans — `Host` / `Registry` / `realize`
from `runtime-scene`, the `caps` traits and the `BuiltinSet` lever from
`runtime-vocabulary`, and the boot-time installers from
`runtime_shared::backend` — plus a `runtime_shared` re-export for the value
types in the capability signatures (`StyleRules`, `AccessibilityProps`,
`Action`, `IconData`, …). A backend crate needs no other framework
dependency, in-tree or out.

A backend must **not** depend on `runtime-core`. That root is the *author*
surface, and its `glue` re-export deliberately shadows several substrate
names with authoring wrappers.

`crates/runtime/vocabulary/tests/third_party_backend.rs` is a complete
worked backend written under exactly the constraints an out-of-tree crate
has, and it scans its own source to prove it never reaches past the public
surface.

### Environment services: `install_env_services`

Five author-facing free functions read thread-local slots rather than
taking a backend reference, so author code can call them from any
component body, effect, or event handler:

| Author call | Backend capability |
|---|---|
| `platform()` | `AppEnvOps::platform` |
| `color_scheme()` | `AppEnvOps::color_scheme` |
| `open_url(url)` | `AppEnvOps::url_opener` |
| `set_fullscreen(on)` | `AppEnvOps::fullscreen_setter` |
| `announce(msg, priority)` | `A11yOps::announce_for_accessibility` |

Filling those slots is the boot entry's job — there is no central
`mount()` to hang it on, because the backend's boot entry *is* the mount
path. One call does all five:

```rust
runtime_vocabulary::backend::install_env_services(&backend);
```

**It must precede the root build**, since a component body may read
`platform()` or `color_scheme()` while constructing — theme selection at
the app root is the common case. A backend that leaves a capability at
its trait default gets the documented no-op for that one service and
working behavior for the rest.

The announcer is the only one that captures the backend, because
`announce_for_accessibility` takes `&mut self`. It is captured **weakly**:
the thread-local is never cleared on teardown, so a strong `Rc` would pin
the backend and its whole view tree for the life of the thread.
`install_env_services` handles that; a hand-rolled `install_announcer`
call must do the same.

Forgetting the call is silent — the app renders correctly and those five
APIs just do nothing. `boot_seam_surface.rs` scans every backend boot file
for it, and `backend_env_seam.rs` pins the behavior.

### The flush driver is part of the backend's job

The kernel stages writes; nothing is observable until someone calls
`World::flush`. Every backend installs a **flush driver** made of two
pieces:

1. **Author-callback wrapping** in the capability impls: call the author
   callback, then `schedule_flush()`. Covers press/click, input/change,
   toggle, slider, scroll, hover, wheel, touch, key, focus/blur,
   file-drop, image load/error, link activation, portal dismiss,
   graphics lifecycle, virtualizer row mount/release, and the app-level
   key handler.
2. **A post-dispatch hook** for author code that runs from non-event
   surfaces — `after_ms` timers, `after_animation_frame` one-shots,
   `raf_loop` iterations, executor future polls. The scheduler and async
   executor fire `dispatch_hook` after each; the boot entry installs
   `schedule_flush` into it.

Web schedules one deduped microtask; the native backends mirror the
design (`crates/backend/{macos,ios/mobile,android/mobile,terminal,cpu,linux,windows}/src/newcore.rs`
each expose `schedule_flush` + a `dispatch_hook`). Hosts that own the
cadence also expose `flush_sync()` for synchronous settle; Roku spells
the same contract `settle()`. Full model:
[`automatic-batching.md`](automatic-batching.md).

**If a backend forgets the driver, author writes never commit** — the UI
appears frozen with no error. That is the single most important thing to
get right in a new backend after `Host`.

---

## The contract: what the framework guarantees a backend

1. **`create_*` is called once per primitive, in construction order.**
   Children are constructed before their parent's `insert` call. A
   freshly returned node has no parent.
2. **`insert(parent, child)` happens after both nodes exist.**
3. **`update_*` is only called for nodes the backend created via the
   matching `create_*`, while those nodes are alive.** The
   subtree-ownership rule above is what prevents "update a node we tore
   down."
4. **`apply_style(node, rules)` may be called many times.** Each call is
   a fresh authoritative application — overwrite, don't accumulate.
5. **`clear_children(node)` leaves the node itself in place.**
6. **`release_*` hooks fire while the node is still alive.** The backend
   can call into platform code through it; the framework drops its handle
   afterwards.
7. **`on_node_unstyled(node)` fires when a styled node goes away**, so
   per-node bookkeeping (class slots, animator state) can be dropped.
8. **`finish(root)` is called once, at the end of the initial mount.**
9. **Calls are single-threaded and never re-entrant by the framework's
   own choice** — the mount path holds `Rc<RefCell<H>>` and borrows it
   per call.

## The contract: what a backend must hold up

1. **Make `Self::Node` cheap to clone.** Bump a refcount; don't deep-copy
   widget state.
2. **Invoke author callbacks on the framework's thread.** Reactivity
   isn't `Send`; platform events arriving on other threads must be posted
   back first.
3. **Install the flush driver** (above), including wrapping any callback
   the backend hands to the platform itself.
4. **For controlled widgets, no-op when set to the current value.**
   `update_text_input_value(node, "abc")` on an input already showing
   "abc" must not re-fire `on_change`, or the round trip
   (signal → update → native event → `on_change` → signal) becomes a
   cycle.
5. **Honor the `release_*` lifecycle** for primitives that register
   native listeners capturing framework state — virtualizer, graphics,
   navigator, portal.
6. **Don't synchronously re-enter a capability method** from inside one.
   See below.

---

## `Rc<RefCell<H>>` and re-entrancy

Handlers and binding effects reach the backend through
`Rc<RefCell<H>>` and call `borrow_mut()` per operation. Re-entry panics
with "RefCell already borrowed."

That is intentional: it surfaces "I called back into the framework
synchronously from inside a backend method" as a hard error instead of
silent corruption. Backend methods are expected to be linear — do the
platform work, return.

Two shapes that need care:

- **Backend → platform → synchronous platform callback → Rust →
  backend.** The common case for release hooks. Fix by deferring with
  `runtime_core::scheduling::schedule_microtask`.
- **A capability method that would mount a subtree.** The framework
  splits those instead: `create_navigator` builds the container only, and
  `navigator_attach_initial(node, screen, scope_id)` mounts the initial
  screen from outside the borrow window.

### The two-phase teardown pattern

When the platform's release call synchronously re-enters Rust — the web
virtualizer's `release()` unmounts every visible cell, each unmount drops
a per-row scope, which fires `on_node_unstyled` back into the backend —
split it:

```rust
fn release_virtualizer(b: &mut WebBackend, node: &Node) {
    let instance = b.virtualizer_instances.remove(&id_of(node)).unwrap();

    // 1. Synchronously flip the JS `_released` flag: no further platform
    //    callbacks enter Rust.
    set_released_now(&instance.js);

    // 2. Defer the heavy release, so the outer borrow_mut has been
    //    released by the time we re-enter.
    runtime_core::scheduling::schedule_microtask(move || {
        call_release(&instance.js);
        drop(instance);                 // drops the closures JS held
    });
}
```

**If you add a release hook that calls platform code which may
synchronously invoke Rust, this pattern is the default.**

---

## Structure-bearing primitives

Four primitives hand the backend framework callbacks instead of just
props. Their capability traits are where a new backend's real work is.

### Virtualizer

The framework decides **what** to mount; the backend decides **when** and
**where**. It receives a callback bundle
(`VirtualizerCallbacks<Self::Node>`) with `item_count`, `item_key`,
`item_size`, `measure_sizes`, `mount_item`, `release_item`,
`set_measured_size`, and owns the visible-window math, the scroll
handler, and (on native) cell recycling. `virtualizer_data_changed(node)`
fires from a binding effect on the data signal so the backend can
re-query and diff. `release_virtualizer` tears down listeners.

Rows mount through `MountCx::realize_detached`, so each row's scope is
freed when the backend calls `release_item`.

### Navigator

The backend owns the platform stack (`UINavigationController`,
`FragmentManager`, an inline subtree on web); the framework owns the
route table and per-screen scope bookkeeping. `create_navigator` builds
the container and installs the command dispatcher —
**not** the initial screen; `navigator_attach_initial` does that from
outside the borrow. Author-facing navigation is already flush-safe:
`on_select` / `pop` / `NavHandle` stage a command that a driver effect
commits on the flush, so one navigation is one logical update
(`crates/runtime/vocabulary/src/handlers/navigator.rs`).

### Graphics

An authored GPU surface. The framework provides the platform drawable
(`<canvas>`, `SurfaceView`, `UIView` + `CAMetalLayer`) and the
`on_ready` / `on_resize` / `on_lost` lifecycle callbacks; the author owns
the rendering. No GPU crate is linked by the framework itself.

### Presence / Portal

`create_presence_placeholder` + `apply_presence` drive
enter/exit animation for a mounting/unmounting child;
`create_portal` / `set_portal_hidden` / `release_portal` back
`overlay` and `anchored_overlay`. Both degrade to containers by default.

---

## Imperative handles

Each primitive has a typed handle (`ButtonHandle`, `TextInputHandle`,
`ScrollViewHandle`, …). Backends mint them in the `make_*_handle`
methods; the handle wraps an `Rc<dyn Any>` the backend owns, and a
per-primitive `Ops` trait defines the callable methods.

```rust
fn make_text_input_handle(&self, node: &Node) -> TextInputHandle {
    TextInputHandle::new(Rc::new(node.clone()), &WebTextInputOps)
}
```

Backends that don't implement an imperative API for a primitive leave the
default, which returns a no-op handle (`caps::noop`). The author's
`handle.focus()` silently does nothing there. The handler fills the
author's `Ref<H>` through the payload's `ref_fill` slot — see
[`reactivity.md` § `Ref<H>`](./reactivity.md#refh--the-imperative-handle-slot)
for the lifetime caveat.

---

## Styling hooks

### Stylesheet pre-generation

```rust
fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]);
fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]);
```

`rules` is the pregenerated set — one entry for base, one per single-axis
variant, one per declared compound variant. A backend can mint a CSS
class per entry, a `Drawable` per entry, or nothing. Registration happens
once per `(sheet, theme)` pair; on a theme install every active
registration is queued for unregistration. Backends without
ahead-of-time caching leave both as no-ops and handle each `apply_style`
directly.

### Interaction states — two paths

Picked by `handles_states_natively()`:

- **`true`** — the backend receives `apply_styled_states(base, overlays)`
  and emits its own state tracking. Web mints CSS pseudo-class rules
  (`:hover`, `:active`, `:focus`, `[disabled]`) and lets the browser
  activate them; no Rust-side bookkeeping.
- **`false`** — the framework calls `attach_states(node, setter)` with a
  closure that flips per-node state bits. The backend installs native
  touch/focus/press listeners that call the setter; the state signal
  flips, the style binding effect re-runs with the new bits merged into
  the variant set, and the backend gets an ordinary `apply_style`.

Mobile backends use the second path. Both produce the same observable
behavior.

---

## Batched repeat fast path

A static `for i in 0..n` lowering becomes `Element::Many`: one payload
that mounts N sibling nodes directly into the enclosing parent through a
`Registry::register_many` handler, which receives parent access so a
batching backend can collapse the whole expansion into one FFI round
trip (`crates/runtime/scene/src/element.rs`, the `Many` variant).

Backends opt in through `BatchOps`:

```rust
fn supports_batched_repeat(&self) -> bool { true }
fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node>;
fn execute_batch_with_attach(&mut self, batch, parent, attach_locals) -> Vec<Self::Node>;
```

`attach_locals` is the list of batch-local ids (the row tops) to parent
under `parent` once the batch's structural ops have run, so creation and
attachment share one round trip. The default lowers to `execute_batch` +
`Host::insert_many`, so a backend that doesn't override gets the
two-step behavior. Web ships the op stream as a single `Uint32Array`
through one wasm→JS call and does the `appendChild`es inside a
`DocumentFragment`.

`Many` is a children-list-only variant: it stands for N siblings, never a
subtree root, so realizing one as a detached root (navigator screen,
keyed row root, spliced hole root) panics.

---

## A minimal new backend

One dependency (`runtime-vocabulary`) and one import
(`runtime_vocabulary::backend`). The order to build it in:

1. **`Host`** — the seven structural ops and `type Node`. Decide
   `supports_splice` honestly; `false` is always correct and costs an
   anchor node per reactive region.
2. **The six required capability methods.** Everything else has a
   default, but these six do not, so `AllCaps` is unsatisfiable without
   them: `ViewOps::create_view`, `TextOps::create_text`,
   `TextOps::update_text`, `ButtonOps::create_button`,
   `StyleOps::apply_style`, `LifecycleOps::finish`. That is enough for a
   real tree.
3. **A `newcore::start_with::<S>(backend, register, build)` entry**,
   generic over `BuiltinSet` — see the boot-order contract in
   `runtime_shared::backend`. It installs the scheduler and clock, calls
   [`install_env_services`](#environment-services-install_env_services),
   creates the `Registry`, calls `register_builtins_with::<H, S>`, creates
   the `World`, realizes under `enter`, and calls `finish`. Keep it
   generic: an entry that pins `AllBuiltins` re-anchors the whole
   vocabulary and costs every app on the platform ~65 KB.
4. **The flush driver** — wrap author callbacks, install the
   `dispatch_hook`, expose `schedule_flush` (and `flush_sync` if the host
   owns the cadence). This is the one omission that compiles, mounts, and
   then silently never commits an author write.
5. **The remaining caps traits**, in the order your app needs them. Each
   is an empty `impl Trait for MyBackend {}` until you want it; every one
   you skip degrades to a placeholder or a container.

```rust
use runtime_vocabulary::backend::{caps, Host};

impl Host for TuiBackend {
    type Node = TuiNodeRef;
    fn insert(&mut self, parent: &mut TuiNodeRef, child: TuiNodeRef) { /* … */ }
    fn insert_at(&mut self, parent: &mut TuiNodeRef, child: TuiNodeRef, index: usize) { /* … */ }
    fn remove_child(&mut self, parent: &TuiNodeRef, child: &TuiNodeRef) { /* … */ }
    fn clear_children(&mut self, node: &TuiNodeRef) { /* … */ }
    fn create_anchor(&mut self) -> TuiNodeRef { /* a plain container */ }
    fn supports_splice(&self) -> bool { false }
}

impl caps::ViewOps for TuiBackend { /* create_view */ }
impl caps::TextOps for TuiBackend { /* create_text, update_text */ }
impl caps::ButtonOps for TuiBackend { /* create_button */ }
impl caps::StyleOps for TuiBackend { /* apply_style */ }
impl caps::LifecycleOps for TuiBackend { /* finish */ }

// The other 25 are empty until you want them — every method defaults.
impl caps::InputOps for TuiBackend {}
impl caps::ScrollOps for TuiBackend {}
// …
```

`crates/runtime/vocabulary/tests/third_party_backend.rs` is the complete
version of that sketch, written under out-of-tree constraints and
compiled on every test run.
`crates/backend/terminal/src/newcore.rs` and
`crates/backend/cpu/src/newcore.rs` are the smallest in-tree worked
examples; `crates/backend/web/src/newcore.rs` is the reference with every
fast path turned on.

---

## Selecting a backend at build time

Today the CLI generates a per-platform wrapper crate into
`target/idealyst/<app>/<platform>/wrapper/` whose `Cargo.toml` names
exactly one backend and whose `src/lib.rs` carries the platform entry
symbol (`#[wasm_bindgen(start)]`, `ios_main`, `Java_…_attach`, `fn main`).
The app crate itself stays backend-free (`crate-type = ["rlib"]`), so the
same source ships to every backend. The generators live in
`crates/tools/build/*`.

That means a third-party backend cannot currently go through
`idealyst build`: `crates/tools/cli/src/platform.rs` is a closed clap
`ValueEnum`, so a custom backend has to be booted from a hand-written
wrapper or `main.rs`.

**The intended replacement** is a Cargo dependency alias plus a
per-backend entry macro, which keeps backend choice entirely in the app's
manifest and out of the framework:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
idealyst-backend = { package = "backend-web", version = "…" }

[target.'cfg(target_os = "ios")'.dependencies]
idealyst-backend = { package = "my-company-uikit-backend", path = "../mine" }
```

```rust
idealyst::entry!(app);   // expands to ::idealyst_backend::idealyst_entry! { … }
```

A custom backend is then just a different `package =`. Nothing in the
framework enumerates backends, and the cfg-selection is Cargo's job.

Two constraints any such macro contract has to respect — both discovered
by walking the existing generators, and both the reason the entry macro
belongs to the *backend* rather than to a central `entry!`:

- **The entry symbol shape is platform contract, not detail.** A
  `#[wasm_bindgen(start)]` fn, a `#[no_mangle] extern "C"` fn, a JNI
  symbol whose name embeds the app's Java package, and a plain `fn main`
  cannot be papered over by one signature. Backends also differ in their
  host-attachment argument (`selector`, root-view pointer, JNI context,
  window options), so no single `trait Boot` covers them.
- **A macro that emits `#[wasm_bindgen(start)]` expands in the *app's*
  crate**, so either the app declares `wasm-bindgen` directly or the
  backend re-exports the attribute for its expansion to name. Likewise
  Android's JNI symbol name cannot be built from a string literal by
  `macro_rules!` (no stable `concat_idents!`) — that backend's entry macro
  must be a proc-macro, or the JNI binding must move to
  `JNI_OnLoad` + `RegisterNatives`. The contract is the *invocation
  shape*, not the macro kind, so both are allowed.

Whatever the entry macro does, it must keep forwarding the `BuiltinSet`
type parameter: an entry that pins `AllBuiltins` silently re-anchors the
whole vocabulary (`boot_seam_surface.rs`).

---

## What the framework deliberately doesn't do

- **No layout system in the seam.** The framework hands `StyleRules` to
  the backend; the backend lays out. Web uses CSS flex; the native
  backends drive Taffy through `runtime-layout`; the GPU backend does its
  own pass. There is a flex-like style vocabulary every backend
  interprets in its native idiom.
- **No diff pass.** Updates flow through binding effects into capability
  calls. The framework never compares old to new tree shapes; the
  reactive kernel invalidates exactly what changed.
- **No synthetic event system.** Events are callbacks handed to
  `create_*` / `install_*`. There is no synthetic bubbling, capturing, or
  normalization layer.
- **No render scheduler above the flush.** The flush is the only
  coalescing boundary the framework imposes; a backend that wants
  additional batching (web's rapid-`apply_style` coalescing) does it
  internally.

These omissions are deliberate. Each is a place where a generic
abstraction would have constrained backends to a model that doesn't fit
their platform.
