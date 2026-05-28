# The backend layer

A backend is the platform-specific renderer. It implements the
`Backend` trait, the framework calls it through that trait, and the
framework knows nothing else about it. This is the seam that makes
the same application code run on the DOM, on `android.view.View`s,
on UIKit, or on anything else you can drive from Rust.

This doc covers the contract a backend implements, the framework
guarantees a backend can rely on, and the lifecycle rules that
keep things from blowing up at teardown.

Implementation: `runtime_core::backend` (the trait + default
ops), plus the render walker in `runtime_core::lib`.

---

## The trait at a glance

```rust
pub trait Backend {
    type Node: Clone;

    // — Construction
    fn create_view(&mut self) -> Self::Node;
    fn create_text(&mut self, content: &str) -> Self::Node;
    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node;
    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node { unimplemented!() }
    fn create_text_input(&mut self, …) -> Self::Node { unimplemented!() }
    fn create_toggle(&mut self, …) -> Self::Node { unimplemented!() }
    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node { unimplemented!() }
    fn create_slider(&mut self, …) -> Self::Node { unimplemented!() }
    fn create_activity_indicator(&mut self, …) -> Self::Node { unimplemented!() }
    fn create_virtualizer(&mut self, callbacks: VirtualizerCallbacks<Self::Node>, …) -> Self::Node { unimplemented!() }
    fn create_graphics(&mut self, …) -> Self::Node { unimplemented!() }
    fn create_navigator(&mut self, …) -> Self::Node { unimplemented!() }

    // — Tree manipulation
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);
    fn clear_children(&mut self, node: &Self::Node);

    // — Reactive update
    fn update_text(&mut self, node: &Self::Node, content: &str);
    fn update_button_label(&mut self, node: &Self::Node, label: &str) { self.update_text(node, label); }
    fn update_image_src(&mut self, node: &Self::Node, src: &str) { /* default no-op */ }
    fn update_text_input_value(&mut self, …) { /* default no-op */ }
    fn update_toggle_value(&mut self, …) { /* default no-op */ }
    fn update_slider_value(&mut self, …) { /* default no-op */ }
    fn update_web_view_url(&mut self, …) { /* default no-op */ }
    fn update_video_src(&mut self, …) { /* default no-op */ }
    fn virtualizer_data_changed(&mut self, node: &Self::Node) { /* default no-op */ }
    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) { /* default no-op */ }

    // — Styling
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>);
    fn apply_styled_states(&mut self, node, base, overlays) { self.apply_style(node, base); }
    fn handles_states_natively(&self) -> bool { false }
    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) { /* default no-op */ }
    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) { /* default no-op */ }
    fn on_node_unstyled(&mut self, node: &Self::Node) { /* default no-op */ }
    fn attach_states(&mut self, node, setter) { /* default no-op */ }

    // — Lifecycle cleanup
    fn release_virtualizer(&mut self, node: &Self::Node) { /* default no-op */ }
    fn release_graphics(&mut self, node: &Self::Node) { /* default no-op */ }
    fn release_navigator(&mut self, node: &Self::Node) { /* default no-op */ }

    // — Imperative handle factories (one per primitive)
    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle { /* no-op default */ }
    fn make_text_input_handle(&self, node: &Self::Node) -> TextInputHandle { /* no-op default */ }
    fn make_scroll_view_handle(&self, …) -> ScrollViewHandle { /* no-op default */ }
    // …one per primitive…

    // — Initial mount
    fn navigator_attach_initial(&mut self, navigator, screen, scope_id) { /* default no-op */ }
    fn finish(&mut self, root: Self::Node);
}
```

The shape: **one method per "thing the framework wants to do,"
defaulted as `unimplemented!()` or no-op so a backend can ship
incrementally.** A backend that only implements `View`, `Text`,
`Button`, `insert`, `update_text`, `apply_style`, and `finish` is
sufficient for a non-trivial app.

`Self::Node: Clone` is the only constraint on the node type. The
framework holds and clones `Self::Node` values freely; backends
typically use a cheap-to-clone wrapper (`web_sys::Node` on web is
already a refcount; `GlobalRef` on Android wraps a JVM ref-counted
handle).

---

## The render walker

`runtime_core::render(backend, primitive)` is the entry point.
The walker visits each `Element` and emits calls into the
backend trait.

```text
render(backend, tree)
   │
   ▼
with_scope(&mut owner.scope, || build(backend, tree))
   │
   ▼
build(backend, Element::View { children, style, ref_fill })
   ├─ backend.create_view()                          → node
   ├─ for child in children:
   │     child_node = build(backend, child)          (recurse)
   │     backend.insert(&mut node, child_node)
   ├─ attach_style(backend, &node, style)            // installs apply-style Effect
   ├─ if ref_fill is Some: fill the Ref<H> with backend.make_view_handle(&node)
   └─ return node
   │
   ▼
backend.finish(root_node)
```

Two crucial properties of the walker:

### Effects are created inside the scope

Every `Effect::new(...)` inside `build` registers with the active
scope. That includes the reactive-text effect, the apply-style
effect, the reactive-disabled effect, the per-primitive cleanup
effects (which hold `release_virtualizer` / `release_graphics`
hooks). When the scope drops — owner teardown, `when`/`switch`
rebuild — every effect inside it is freed together.

This is the [`reactivity.md` § effects-first-signals-second](./reactivity.md#drop-order-effects-first-signals-second)
invariant in action: backend cleanup hooks ride along with the
scope drop, and they fire while signals are still live.

### Updates flow through Effects, not re-render

When the walker sees a reactive prop, it doesn't store the closure
on the primitive for later. It wraps the closure in an `Effect`:

```rust
// Inside build_reactive_text:
let _e = Effect::new(move || {
    let value = compute();
    backend.borrow_mut().update_text(&node, &value);
});
```

The effect runs immediately (initial value), then re-runs whenever
signals inside `compute()` change. Each re-run is one
`Backend::update_text` call. **The native widget exists once and is
mutated in place.** There's no diff, no virtual DOM, no re-render
pass.

This is true of every "reactive prop" across every primitive:
labels, image src, input value, toggle value, slider value, video
src, web-view url, disabled flag, style. Each gets its own
dedicated `Effect`, so changes to one don't invalidate the others.

---

## The contract: what the framework guarantees a backend

A backend can assume:

1. **`create_*` is called exactly once per primitive in a tree, in
   construction order.** Children are constructed before their
   parent's `insert(parent, child)` call. The backend's
   `Self::Node` returned from a `create_*` call is a fresh node;
   it has no parent yet (and on most platforms, no children).

2. **`insert(parent, child)` happens after both nodes are
   constructed.** The walker holds a `&mut Self::Node` to the
   parent; backends can rely on the parent existing.

3. **`update_*` is only called for nodes the backend created via the
   matching `create_*`, while those nodes are still alive.** No
   "update a node we already tore down" race — that's prevented by
   the scope lifecycle.

4. **`apply_style(node, rules)` may be called multiple times.** Each
   call is a fresh authoritative style application; the backend
   should overwrite, not accumulate. The same node may move through
   many `apply_style` calls as variants and overrides change.

5. **`clear_children(node)` may leave the node itself in place.**
   Used by reactive conditionals when a `Switch`/`When` branch flips
   — the placeholder `View` survives, only its children change.

6. **`release_*` hooks fire while the corresponding `Self::Node` is
   still alive** (the framework hasn't dropped its handle yet).
   Backends can call back into platform code via the still-live
   node. After `release_*` returns, the framework drops its handle.

7. **`on_node_unstyled(node)` fires when a styled node is going
   away.** A backend can drop per-node bookkeeping (CSS class
   slots, animator state) here.

8. **`finish(root)` is called once, at the end of the initial
   render.** Most backends attach the root to their mount point
   here. After `finish`, updates only flow through `update_*` /
   `apply_style` / lifecycle hooks.

The framework does **not** call backend methods in parallel — it's
all on one thread, and the `Rc<RefCell<B>>` it holds means
re-entrant calls panic at the borrow check (see "Re-entrancy
hazards" below).

---

## The contract: what a backend must hold up

In return, a backend must:

1. **Make `Self::Node` cheap to clone.** Cloning a node should not
   deep-copy underlying widget state — typically just bump a refcount.

2. **Run `on_click` / `on_change` callbacks on the framework's
   thread.** Platform events that arrive on background threads must
   be posted back to the main thread before invoking the callback;
   reactivity isn't `Send`.

3. **For controlled widgets, no-op when set to the current value.**
   `update_text_input_value(node, "abc")` when the input already
   shows "abc" must not re-fire `on_change`. Otherwise the
   round-trip (signal → `update` → native event → `on_change` →
   signal) becomes a cycle. Most platform APIs already no-op on
   identical input — the requirement is to not get clever.

4. **Honor the `release_*` lifecycle for primitives with
   listeners.** If the backend creates a `Element::Virtualizer`,
   `Element::Graphics`, or `Element::Navigator`, it almost
   certainly registers native event listeners that capture
   wasm-bindgen / JNI closures that hold framework state. When the
   primitive's enclosing scope drops, those listeners need to be
   detached and the closures dropped before the captured framework
   state is freed. See "Release lifecycle" below.

5. **Don't synchronously call framework code from `create_*`/`apply_*`
   if it would re-enter `Backend`.** The walker holds `borrow_mut`
   on the backend's `RefCell` during the call; re-entry panics.
   See "Re-entrancy hazards."

---

## The walker holds `Rc<RefCell<B>>`

The walker passes `&Rc<RefCell<B>>` to `build` recursively. Each
backend call does:

```rust
backend.borrow_mut().create_view()
```

This is a runtime-checked borrow, single-threaded. The backend's
methods take `&mut self`, so re-entry through `borrow_mut` panics
with "RefCell already borrowed."

This is intentional. It surfaces "I called back into the framework
synchronously from a backend method" as a hard error instead of
silent state corruption. The backend impls are expected to be
linear: do the platform work, return.

But — primitives like `Navigator`, `Graphics`, and `Virtualizer`
fundamentally need to call back into the framework (mount a screen,
mount a list item, fire `on_resize`). Those callbacks can run
**after** the synchronous backend call returns, when the borrow
has released. The framework provides the [`Backend::navigator_attach_initial`](#navigator)
seam exactly so the backend doesn't have to call `mount_screen`
inside `create_navigator`.

For callbacks that arrive **synchronously** from inside platform
code (a JS callback fired during `release()`, an Android listener
that re-enters during `removeAllViews`), the backend is responsible
for deferring the work — typically with `runtime_core::schedule_microtask`
— so the re-entrant call lands after the outer borrow has been
released.

---

## Lifecycle cleanup hooks

Three lifecycle hooks tear down platform-held resources before the
captured framework state is freed:

```rust
fn release_virtualizer(&mut self, node: &Self::Node) { /* default no-op */ }
fn release_graphics(&mut self, node: &Self::Node) { /* default no-op */ }
fn release_navigator(&mut self, node: &Self::Node) { /* default no-op */ }
```

For each of these primitives, the walker installs a cleanup
`Effect` whose drop captures the node and calls the corresponding
release method. When the surrounding scope drops, the effect drops,
the release method fires, the backend detaches listeners and drops
its retained closures.

The [`reactivity.md` § drop order](./reactivity.md#drop-order-effects-first-signals-second)
invariant guarantees signals are still alive at this point —
crucial because the release method may invoke platform code that
synchronously fires queued events, and those events may reach Rust
callbacks that read user signals.

### The two-phase teardown pattern

Sometimes the platform's release call **synchronously re-enters**
Rust before it can return. E.g. the web `Virtualizer`'s
`release()` call unmounts every visible cell, each unmount calls
the framework's `release_item(scope_id)`, which drops a per-item
`Scope`, which fires `StyleHandle::drop`, which calls
`backend.borrow_mut().on_node_unstyled(...)` — and panics, because
the outer `release_virtualizer` already holds `borrow_mut`.

The pattern that resolves this:

```rust
fn release_virtualizer(b: &mut WebBackend, node: &Node) {
    let id = virtualizer_id_of(node).unwrap();
    let instance = b.virtualizer_instances.remove(&id).unwrap();

    // Step 1: synchronously flip the JS `_released` flag so no further
    // platform callbacks try to enter Rust.
    set_released_now(&instance.js);

    // Step 2: defer the heavy release call to a microtask, so the
    // outer borrow_mut() has been released by the time we re-enter.
    runtime_core::schedule_microtask(move || {
        let release_fn: js_sys::Function = /* … */;
        let _ = release_fn.call0(&instance.js);
        drop(instance);                       // drops the closures JS held
    });
}
```

The synchronous step prevents further re-entry; the deferred step
does the actual platform teardown after the borrow has been
released. **If you find yourself adding a release hook that calls
back into platform code that may synchronously invoke Rust, this
pattern is your default.**

---

## Virtualizer

The framework handles **what** to mount; the backend handles
**when** and **where**. The framework hands the backend a callback
bundle:

```rust
pub struct VirtualizerCallbacks<N: Clone + 'static> {
    pub item_count:        Rc<dyn Fn() -> usize>,
    pub item_key:          Rc<dyn Fn(usize) -> ItemKey>,
    pub item_size:         Rc<dyn Fn(usize) -> f32>,
    pub measure_sizes:     bool,
    pub mount_item:        Rc<dyn Fn(usize) -> (N, u64)>,         // returns (node, scope_id)
    pub release_item:      Rc<dyn Fn(u64)>,
    pub set_measured_size: Rc<dyn Fn(u64, f32)>,
}
```

The backend owns the visible-window math, the scroll handler, and
(on native) cell recycling. It calls:

- `item_count()` to size the scroll content.
- `item_key(idx)` for stable identity (so keyed diffs work across
  data changes).
- `item_size(idx)` for an initial size (used for layout before any
  measurement happens). `measure_sizes == true` means this is an
  estimate the backend should refine by measuring the mounted
  node.
- `mount_item(idx)` when an index needs to become visible. Returns
  the freshly-built subtree node plus the per-item scope id.
- `release_item(scope_id)` when an index leaves the visible window
  (web: scrolled out; native: cell recycled). The framework drops
  the matching scope, freeing every signal/effect/ref inside the
  item's subtree.
- `set_measured_size(scope_id, size)` to push a measured size back
  to the framework for layout.

The framework also fires `virtualizer_data_changed(node)` from an
`Effect` that reads the data signal, so the backend can re-query
counts/keys/sizes and diff against its mounted set.

Authors write `runtime_core::primitives::flat_list::flat_list(...)`
or `ui! { FlatList(...) }` — both produce this primitive with the
data-side closures pre-wired.

---

## Navigator

A `Element::Navigator` is the framework's screen-stack container.
The backend owns the platform-native stack
(`UINavigationController`, `FragmentManager`, an inline subtree on
web). The framework owns the route table, the imperative
`NavigatorHandle`, and per-screen scope bookkeeping.

```rust
fn create_navigator(
    &mut self,
    callbacks: NavigatorCallbacks<Self::Node>,
    control: Rc<NavigatorControl>,
) -> Self::Node;

fn navigator_attach_initial(&mut self, navigator: &Self::Node, screen: Self::Node, scope_id: u64);

fn release_navigator(&mut self, node: &Self::Node);
```

The backend's `create_navigator` should:

1. Build the native stack container and return its node.
2. Call `control.install(Box::new(|cmd| { /* execute cmd */ }))` —
   this is the dispatcher the user-facing `NavigatorHandle` invokes.
3. **Not** call `callbacks.mount_screen` inside this method.
   `create_navigator` is invoked while the walker holds the
   backend's `borrow_mut`; `mount_screen` re-enters the build
   walker (which itself `borrow_mut`s) → double-borrow panic.

The framework then calls `navigator_attach_initial(node, screen,
scope_id)` from outside the borrow window. The backend mounts the
initial screen there.

For subsequent navigations:

- The user calls `nav.push(...)` / `nav.pop()` / `nav.replace(...)` /
  `nav.reset(...)` on the handle.
- The handle dispatches a `NavCommand` through `NavigatorControl`.
- The dispatcher closure (installed in step 2 above) runs. It calls
  `callbacks.mount_screen(name, params)` to get the new screen's
  subtree, then commits the native transaction.
- After each commit, the backend calls `callbacks.depth_changed(d)`
  so `handle.depth()` stays in sync.
- When a screen leaves the stack, the backend calls
  `callbacks.release_screen(scope_id)` — drops the per-screen
  scope.

---

## Graphics

`Element::Graphics` is an authored GPU surface. The author owns
the rendering — the framework just provides the platform-native
drawable widget (`<canvas>` on web, `SurfaceView` on Android,
`UIView` + `CAMetalLayer` on iOS) and the lifecycle callbacks.

```rust
fn create_graphics(
    &mut self,
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
) -> Self::Node;

fn release_graphics(&mut self, node: &Self::Node);
```

The backend exposes the drawable as a `raw_window_handle`-compatible
object. The author's GPU library of choice (wgpu, glow, …) wires up
its own pipeline against that. The framework doesn't link any GPU
crate — wgpu types appear only in `runtime_core::primitives::graphics`'s
*author-facing* API, behind a feature flag in downstream code.

---

## Imperative handles

Each primitive has a typed handle: `ButtonHandle`, `TextInputHandle`,
`ScrollViewHandle`, etc. Backends produce handles via the
`make_*_handle` trait methods. The handle wraps a `Rc<dyn Any>`
that the backend owns; an `Ops` trait per primitive defines what
methods can be invoked.

```rust
// Web backend's text-input handle:
fn make_text_input_handle(&self, node: &Node) -> TextInputHandle {
    TextInputHandle::new(Rc::new(node.clone()), &WebTextInputOps)
}

struct WebTextInputOps;
impl TextInputOps for WebTextInputOps {
    fn focus(&self, node: &dyn Any) {
        let node = node.downcast_ref::<Node>().unwrap();
        // …call .focus() on the DOM element…
    }
    fn blur(&self, node: &dyn Any) { /* … */ }
    fn select_all(&self, node: &dyn Any) { /* … */ }
}
```

Backends that don't implement an imperative API for a given
primitive leave the trait default — which returns a no-op handle
(backed by `Rc::new(())`). The author's `handle.focus()` call
silently does nothing on that platform.

User components declared with `#[component] + methods! { … }` get a
parallel system: the macro generates a handle struct and the
component's parent can drive it through `Ref<MyHandle>` the same way.

---

## Stylesheet pre-generation

Some backends (notably web) want to **mint platform style state
ahead of time** rather than per-`apply_style`-call. The framework
exposes this through two hooks:

```rust
fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]);
fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]);
```

`rules` is the result of `runtime_core::style::pregenerate_for_theme`
— one entry for base, one per single-axis variant, one per declared
compound variant. The backend can mint a CSS class per entry, or
build a `Drawable` per entry, or whatever its caching strategy is.

The framework calls `register_stylesheet` exactly once per
`(sheet, theme)` pair (the first time a stylesheet is resolved
against a theme it hasn't seen). On `set_theme`, every active
registration is queued for `unregister_stylesheet` — the backend
hears about them on the next style-effect run, when it's in scope.

Backends without ahead-of-time caching needs (most native backends)
leave both default no-ops and just handle each `apply_style` call
directly.

---

## Batched `Repeat` fast path

When the walker expands a `Element::Repeat`, it inspects the row
shape and — if every row is a static `View`/`Text` tree with static
styles — accumulates the whole expansion into a `BackendBatch`
instead of issuing per-row backend calls. The batch ships in one
FFI round-trip via `execute_batch`.

Backends opt in by overriding two trait methods:

```rust
fn supports_batched_repeat(&self) -> bool { true }
fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node>;
```

The default `supports_batched_repeat` is `false` — the walker then
expands the Repeat the slow way (per-row `create_*` + `apply_style`
+ `insert`). Backends that have a cheap batched path (the web
backend ships the whole op stream as a single `Uint32Array`
through one wasm→JS call) flip the flag on and implement
`execute_batch`.

### One call to rule them all: `execute_batch_with_attach`

`Repeat` doesn't just create its rows — it parents them under the
surrounding container. Originally that was a separate
`insert_many(parent, rows)` follow-up call to the backend; for the
web backend that meant N additional `appendChild` FFI hops, which
dominated at large N.

The walker now calls a single combined method:

```rust
fn execute_batch_with_attach(
    &mut self,
    batch: BackendBatch,
    parent: &mut Self::Node,
    attach_locals: &[u32],
) -> Vec<Self::Node>;
```

`attach_locals` is the list of batch-local ids (typically the
row-top ids) the backend should parent under `parent` once the
batch's structural ops have executed. The default impl is
literally:

```rust
let nodes = self.execute_batch(batch);
if !attach_locals.is_empty() {
    let rows: Vec<_> = attach_locals.iter()
        .map(|&id| nodes[id as usize].clone()).collect();
    self.insert_many(parent, rows);
}
nodes
```

so backends that don't override get the old two-step behaviour.
Backends that DO override can fold the attach into the same
round-trip — on web that's a `Uint32Array` of the row-top
`local_id`s passed alongside the existing batch buffers, and the
JS shim does N pure-JS `appendChild` calls inside a single
`DocumentFragment`. Measured savings: ~10 ms per 100 k-row
rebuild transition.

### What's batchable

The walker bails on the batched path (and falls back to per-call
expansion of the whole Repeat) the moment a row contains anything
the batch shape doesn't model:

- non-`View`/`Text` primitives
- reactive styles (`StyleSource::Reactive`, `SignalClass`)
- state overlays (per-node dynamic CSS class per row)
- `ref_fill`, `on_touch`, `safe_area_sides`, reactive text sources

Backends don't need to know about any of this — the walker only
invokes `execute_batch_with_attach` when the entire row shape is
batchable. If your backend opts in, you can assume the incoming
`BackendBatch` is consistent with `BatchOp::{CreateView,
CreateText, ApplyStyleStatic, Insert}`.

---

## Interaction states

Two paths, picked by the backend's `handles_states_natively()` flag.

### Native (`true`)

The backend receives `apply_styled_states(base, overlays)` and
emits its own state-tracking. The web backend mints CSS pseudo-class
rules (`:hover`, `:active`, `:focus`, `[disabled]`) — the browser
handles state activation natively. No Rust-side bookkeeping.

### Event-driven (`false`)

The framework calls `attach_states(node, setter)` with a closure
that flips per-node state bits. The backend installs native event
listeners (touch / focus / press) that call the setter. When the
setter fires, the framework's state signal flips, the apply-style
effect re-runs with the new bits merged into the variant set, and
the backend receives a regular `apply_style(node, &resolved)` call.

Mobile backends use this path. The two paths produce the same
observable behavior; the choice is just about where state tracking
lives.

---

## Re-entrancy hazards

The framework holds the backend behind `Rc<RefCell<B>>`. Inside a
trait method, the backend already has `borrow_mut`. Any synchronous
path that re-enters a trait method will panic:

- **Walker → backend → user closure → user code → walker.** If a
  primitive's `on_click` is invoked from inside `create_button`
  (it's not, but consider it as an example), the user's closure
  could call `signal.set` → effect re-runs → effect calls
  `backend.borrow_mut().update_text` → panic.
- **Backend → platform → platform callback (sync) → Rust closure
  → backend.** This is the common case for `release_virtualizer`,
  `release_navigator`, etc. — the platform call invokes a callback
  the backend handed it earlier, which lands in framework Rust,
  which tries to `borrow_mut` the backend.

Two tools to handle this:

1. **`runtime_core::schedule_microtask(f)`** — defers `f` to run
   after the current synchronous chain returns. Use this in
   release hooks that synchronously trigger platform teardown
   which fires callbacks. Web `release_virtualizer` and `build_switch`'s
   teardown both use this pattern.

2. **Split methods to avoid re-entry.** `create_navigator` +
   `navigator_attach_initial` is the canonical example: the
   framework splits "make the container" from "mount the initial
   screen" so the first runs under the borrow and the second runs
   after.

If you see "RefCell already borrowed" in a new backend, the
question to ask is "what synchronous chain re-entered the backend
inside a trait method." Either the chain shouldn't be synchronous
(microtask-defer) or the trait method should be split.

---

## A minimal new backend

If you wanted to add a backend for some new target — say, a TUI —
here's the smallest set of methods to implement and the order to
add them:

```rust
struct TuiBackend { /* widget tree state */ }

impl Backend for TuiBackend {
    type Node = TuiNodeRef;

    // Required (no defaults):
    fn create_view(&mut self)   -> TuiNodeRef { /* … */ }
    fn create_text(&mut self, content: &str) -> TuiNodeRef { /* … */ }
    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> TuiNodeRef { /* … */ }
    fn insert(&mut self, parent: &mut TuiNodeRef, child: TuiNodeRef) { /* … */ }
    fn update_text(&mut self, node: &TuiNodeRef, content: &str) { /* … */ }
    fn clear_children(&mut self, node: &TuiNodeRef) { /* … */ }
    fn apply_style(&mut self, node: &TuiNodeRef, style: &Rc<StyleRules>) { /* … */ }
    fn finish(&mut self, root: TuiNodeRef) { /* attach to screen */ }
}
```

That's enough for any application that uses only `View`, `Text`,
`Button`, and `When`/`Switch` conditionals. Every other primitive
returns `unimplemented!()` from its default, and the framework will
panic *only if the user actually constructs that primitive* — so
the surface area you owe the application is exactly what it uses.

You can then incrementally add:

- `create_text_input` / `update_text_input_value` for forms.
- `create_scroll_view` for scrolling.
- `make_*_handle` overrides for imperative APIs your platform
  supports.
- Pre-generation hooks if your platform benefits.
- `release_virtualizer` / `release_graphics` / `release_navigator`
  for the primitives that need cleanup hooks.

Each is independent. No backend is required to implement everything.

---

## What the framework deliberately doesn't do

Worth knowing because it shapes the contract.

- **No layout system.** The framework hands `StyleRules` to the
  backend; the backend lays out. Web uses CSS flex; Android uses
  `LinearLayout`; iOS uses Auto Layout or whatever you wire up.
  There's a flex-like style vocabulary (`flex`, `align_items`,
  `justify_content`, `padding`, `margin`, `width`, `height`) that
  every backend interprets in its native idiom.

- **No diff pass.** Updates flow through dedicated `Effect`s into
  `update_*` methods. The framework doesn't compare old to new;
  the reactive system invalidates exactly what changed.

- **No event-system.** Events are `Rc<dyn Fn()>` callbacks passed
  to `create_*`. The backend wires them to platform events and
  invokes them. There's no synthetic event bubbling, capturing,
  or normalization — the framework gets out of the way.

- **No render scheduler.** Effects run synchronously on signal
  change. Backends that need batching (web's CSS `apply_style`
  rapid succession) do their own coalescing internally; the
  framework doesn't try to be clever about when updates land.

These omissions are deliberate. Each one is a place where building
a generic abstraction would have constrained backends to a model
that doesn't fit their platform. By staying out of these concerns,
the framework can target genuinely different rendering substrates
without forcing one to pretend to be another.
