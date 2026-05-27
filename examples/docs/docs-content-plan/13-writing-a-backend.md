# Writing your own backend

A backend is the piece of code that translates the framework's
`Primitive` tree into something a particular platform can put on
screen — DOM elements, UIViews, Android Views, BrightScript
SceneGraph nodes, or anything else you can drive from Rust.

You'd write one when the shipped backends don't cover your
target: a terminal renderer, an embedded display, a custom GPU
canvas, a server-side HTML renderer, a platform we haven't shipped
yet. Most of the framework — primitives, reactivity, styles,
components, hot reload, navigation — works the same against your
backend as it does against the built-in ones. The seam is small.

This page walks the trait, explains the two execution models
(**runtime** vs **generator**), and shows the shape of a minimum
viable implementation.

## The Backend trait

A backend implements one trait — `runtime_core::Backend`. The
declaration is short:

```rust
use runtime_core::{Backend, /* primitives, styles, etc. */};

pub struct MyBackend {
    // your platform-specific state
}

impl Backend for MyBackend {
    type Node = MyNodeHandle;     // the platform's "thing on screen"

    fn create_view(&mut self) -> Self::Node { /* ... */ }
    fn create_text(&mut self, content: &str) -> Self::Node { /* ... */ }
    fn create_button(&mut self, label: &str, on_click: &Action,
                     leading: Option<&IconData>, trailing: Option<&IconData>)
                     -> Self::Node { /* ... */ }
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) { /* ... */ }
    fn update_text(&mut self, node: &Self::Node, content: &str) { /* ... */ }
    fn clear_children(&mut self, node: &Self::Node) { /* ... */ }
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) { /* ... */ }
    fn finish(&mut self, root: Self::Node) { /* ... */ }

    // ... plus 30-ish more methods, almost all with sensible defaults
}
```

The methods above are the **required** ones — no default
implementation. They cover the smallest set the framework needs
to put a tree on screen: create views, create text, create
buttons, attach children, update text content, clear a
container's children, apply styles, finalize the root mount.

Everything else has a default. Most are either no-ops (for things
that don't apply to your platform) or `unimplemented!()` (for
primitives you don't support yet, which causes a clear panic if
the app uses one). You can ship a backend that supports only the
above and progressively fill in the rest.

### `type Node`

The associated type `Node: Clone` is whatever your platform uses
to represent a thing on screen. Pick the shape that's most useful
for your backend's internal state:

- **Web** uses `web_sys::Node` (with an `Rc` inside for cheap
  cloning).
- **iOS** uses a strong reference to a `UIView` subclass through
  `objc2`.
- **Android** uses a JNI global ref to a `View`.
- **Roku** uses a `NodeId` — a u64 the device-side runtime maps
  back to a SceneGraph node.

The framework treats `Node` opaquely. It calls `create_*` to mint
one, holds onto it, hands it back to `insert` / `update_text` /
`clear_children` / `apply_style`. The backend is free to put
whatever it likes inside.

## The two execution models

There are two distinct ways a backend can do its job. Pick the
one that fits the target platform.

### Runtime backends

The default model. The backend manipulates native widgets
**directly, in process**, as the framework hands it operations.

- `create_view()` immediately allocates a `<div>` / `UIView` /
  Android `View`.
- `insert(parent, child)` immediately calls `appendChild` /
  `addSubview` / `addView`.
- `update_text(node, content)` immediately mutates the
  widget's text property.
- `apply_style(node, rules)` immediately writes CSS / view
  properties / drawable attributes.

The shipped web, iOS, and Android backends are all runtime
backends. They run in the same process as your `app()` function;
when a signal changes, the framework re-fires the effect, the
effect calls `update_text(...)`, and the backend mutates the
widget on the spot.

If you're writing a backend for any traditional GUI platform —
desktop, mobile, embedded — runtime is the model you want.

### Generator backends

The unusual model. The backend doesn't have direct access to a
native widget tree. It exists because the *real* renderer lives
somewhere else — on a different device, in a different process,
behind a serialization boundary.

Instead of manipulating widgets, a generator backend **emits a
wire stream of commands** that a remote runtime replays. The
framework calls `create_view()`; the backend mints a `NodeId` and
emits a `Create(NodeId, View)` command. The framework calls
`insert(parent, child)`; the backend emits an `Insert(parent_id,
child_id)`. And so on.

`backend-roku` is the only shipped generator backend. Roku
devices don't run Rust — the only language the runtime
understands is BrightScript. The backend runs on the developer's
host machine; commands stream to a thin client on the device,
which replays them against SceneGraph nodes.

The shape implies two extra constraints generator backends have
to handle:

#### Closures don't ship

A runtime backend can capture a `Box<dyn Fn()>` from the
framework and call it directly. A generator backend can't — the
closure exists in the host's memory, and the device side has no
way to invoke it.

For event handlers, this means the device sends an event-fired
message back to the host, which dispatches the closure
in-process. The wire protocol carries that round-trip.

For reactive expressions (a `Text` whose content reads a signal,
a `When`'s condition), the closure can't be re-evaluated on the
device — so the framework provides a **structured** view of
those expressions through `Derived<T>` and `Action`. Each carries
a `method: &'static str` (a stable name the device runtime maps
to a transpiled BrightScript function) plus an `inputs: Vec<u64>`
(the signal ids the method reads). Generator backends consume
the structured form via the `note_*_binding` hooks:

```rust
fn note_text_binding(&mut self, node: &Self::Node,
                     signal_ids: &[u64], method: &'static str) {
    // Emit a "this node's text is computed by `method` from these signals" command
}

fn note_when_binding(&mut self, anchor: &Self::Node,
                     signal_ids: &[u64], cond_method: &'static str,
                     then_node: &Self::Node, otherwise_node: &Self::Node) {
    // Emit a "this anchor toggles between these two subtrees based on `cond_method`"
}
```

Runtime backends leave these defaults at no-op — they re-run the
closures locally on every signal change, no metadata needed.

#### Inactive subtrees shouldn't materialize

A runtime backend can afford to build both branches of a `when()`
up front and just hide the inactive one — it's a cheap local
operation. A generator backend can't — building means emitting
commands over a network, and shipping a subtree that's not on
screen wastes bandwidth and device memory.

Generator backends opt into **lazy slot capture** to handle
this. The pattern:

```rust
fn supports_lazy_slot_capture(&self) -> bool { true }

fn begin_slot_capture(&mut self) {
    // Redirect subsequent backend mutations from the main wire stream
    // into a capture buffer. The framework calls this around each
    // `when` / `switch` / `for` arm's subtree build.
}

fn end_slot_capture(&mut self, slot_root: &Self::Node) {
    // Close the capture region. The framework will then call one of
    // the `note_*_binding` methods so you can package the captured
    // commands as a stored, replayable subtree.
}
```

With lazy slot capture on, the framework builds each conditional
arm's subtree and the backend stores it as a *template* rather
than sending it. When the condition flips on the device, the
device-side runtime replays the relevant template's commands.

Runtime backends leave `supports_lazy_slot_capture` at `false`
and the framework builds every branch into the live tree
directly — cheap on-platform, no capture needed.

## The full method tour

Here's what's in the trait, grouped by purpose. Methods without
notes are "create + update" pairs for a specific primitive.

### Required (no default)

- `type Node: Clone`
- `create_view`, `create_text`, `create_button`, `insert`
- `update_text`
- `clear_children`
- `apply_style`
- `finish` — called once after the initial render walk to let
  the backend do any final mount work (web's `finish` triggers a
  layout flush, iOS's attaches the root view to the
  `UIWindow`).

### Container primitives (defaults: `create_view`)

- `create_pressable(on_click)` — a tappable container. The
  default falls back to `create_view`, which means clicks won't
  fire but the subtree renders.
- `create_reactive_anchor` — placeholder node for `when` /
  `switch` branches. Web overrides this to return a
  `display: contents` element so the branch's children inherit
  flex context.

### Content primitives (defaults: `unimplemented!()`)

- `create_image`, `update_image_src`
- `create_icon`, `update_icon_color`, `update_icon_stroke`,
  `animate_icon_stroke`
- `create_activity_indicator`

The walker only calls these if your app uses the corresponding
primitive. Leave them `unimplemented!()` until you support the
primitive.

### Inputs (defaults: `unimplemented!()`)

- `create_text_input`, `update_text_input_value`
- `create_toggle`, `update_toggle_value`
- `create_slider`, `update_slider_value`
- `update_button_label`

### Navigation (defaults: `unimplemented!()`)

- `create_navigator` — stack navigator
- `create_tab_navigator`
- `create_drawer_navigator`

These take a callbacks bundle so the framework can ask the
backend to mount/release per-screen subtrees on demand. The
shape is large; the trait's source has annotated examples.

`create_link` is also navigation, but its default falls
through to `create_view` — see the container section.

### Portals (defaults: `unimplemented!()` / no-op)

- `create_portal` — single entry point. Receives a
  `PortalTarget` (either `Viewport(ViewportPlacement)` or
  `Anchor { target, side, align }`), an `on_dismiss` callback,
  and a `trap_focus` flag. Default: `unimplemented!()`.
- `release_portal` — paired teardown. Defaults to no-op.
- `make_portal_handle` — returns a `PortalHandle` for
  imperative ops. Defaults to a no-op handle.

Backends decide how to route by branching on `PortalTarget`.
iOS could route `PortalTarget::Viewport(_)` to a window-level
`UIView` and `PortalTarget::Anchor { .. }` to
`UIContextMenuInteraction`, all inside one `create_portal`.
Web uses the `popover` attribute with CSS anchor positioning.

### Styling

- `apply_style(node, &Rc<StyleRules>)` — required. The
  framework hands you concrete, theme-resolved values.
- `apply_styled_states(node, base, overlays)` — optional. If
  your backend supports declarative state styling (web's CSS
  pseudo-classes), implement this and return `true` from
  `handles_states_natively()`. The framework then hands you
  the base + per-state overlay rules in one call. If you leave
  the default, the framework drives states via signal flips
  and re-fires `apply_style` per change.
- `register_stylesheet(&[Rc<StyleRules>])` — optional.
  Backends that benefit from up-front rule emission (web mints
  CSS classes here) override this. Defaults to a no-op.
- `unregister_stylesheet(&[Rc<StyleRules>])` — paired teardown.
- `install_theme_variables(&[TokenEntry])` — optional. Backends
  with a runtime variable system (web's CSS custom properties)
  install tokens here. iOS and Android leave the default no-op
  and read `Tokenized::value()` at `apply_style` time. See
  [Styles](#) for the full story.

### Lazy-slot capture (generator backends only)

- `supports_lazy_slot_capture(&self) -> bool` — default `false`.
- `begin_slot_capture` / `end_slot_capture` — pair the
  framework calls around each conditional arm's subtree build.
- `note_text_binding`, `note_signal_initial`, `note_when_binding`,
  `note_switch_binding`, `note_repeat_binding` — declarative
  metadata hooks. Generator backends record these so the device
  runtime can re-evaluate reactive expressions without closures.

### Refs and handles

- `ref_ops()` — returns a `RefOps` bundle with the per-primitive
  trait objects the framework uses to construct handles
  (`ButtonHandle`, `ViewHandle`, etc.). The defaults are no-op
  ops, so refs work but the handle methods don't do anything.
  Implement the relevant traits and return them here when you
  want geometry queries, programmatic clicks, etc., to work.
- `make_*_handle` for each primitive — defaults construct
  no-op handles. Override per-primitive to return real
  backend-aware handles.

### Virtualization

- `create_virtualizer(callbacks: VirtualizerCallbacks<Self::Node>)`
  — defaults to `unimplemented!()`. The framework hands you a
  bundle of closures (`item_count`, `mount_item`,
  `release_item`, etc.) and you wire them into your platform's
  recycling widget (`UICollectionView`, `RecyclerView`, an
  IntersectionObserver). See [Lists](#) for what each callback
  carries.

## A skeleton backend

The smallest plausible backend looks like this:

```rust
use std::rc::Rc;
use runtime_core::{Backend, Action, StyleRules, IconData};

#[derive(Clone)]
struct Node {
    // Whatever your platform uses
}

pub struct MyBackend {
    // Backend-level state (the root container, a cache, etc.)
}

impl Backend for MyBackend {
    type Node = Node;

    fn create_view(&mut self) -> Node { /* allocate a container */ }

    fn create_text(&mut self, content: &str) -> Node {
        /* allocate a text node, set initial content */
    }

    fn create_button(&mut self, label: &str, on_click: &Action,
                     _leading: Option<&IconData>, _trailing: Option<&IconData>)
                     -> Node {
        /* allocate a button, wire on_click into your platform's event system */
    }

    fn insert(&mut self, parent: &mut Node, child: Node) {
        /* attach child to parent in your scene */
    }

    fn update_text(&mut self, node: &Node, content: &str) {
        /* set node's text content */
    }

    fn clear_children(&mut self, node: &Node) {
        /* remove all children from node */
    }

    fn apply_style(&mut self, node: &Node, style: &Rc<StyleRules>) {
        /* translate StyleRules into your platform's styling */
    }

    fn finish(&mut self, root: Node) {
        /* attach root to your platform's surface (window, etc.) */
    }
}
```

This compiles and produces a working app — as long as your app
uses only `View`, `Text`, and `Button`. Trying to use anything
else (an `Image`, a `ScrollView`, navigation) will panic with
`unimplemented!()` at the relevant call site, telling you
exactly what to implement next.

That progressive shape is deliberate. You can ship a backend
for an unusual target with the minimum surface working in a
day, and grow it as you need more primitives.

## Driving the render

Once your backend is built, hand it to the framework:

```rust
use runtime_core::{render, Owner};

fn main() {
    let mut backend = MyBackend::new(/* platform args */);
    let _owner = render(&mut backend, my_app::app());
    // ...platform-specific event loop here...
}
```

`render(backend, root)` walks the primitive tree, calling your
backend's methods in the right order. The returned `Owner` holds
the reactive scope; drop it to tear everything down.

What "the event loop" means is platform-specific:

- **Native event loops** (iOS's `UIApplicationMain`, Android's
  `ActivityThread`, a winit loop) — the platform runs the loop,
  your event callbacks call `signal.set(...)`, the framework
  cascades effects through the backend.
- **Reactive runtimes** (web, where there's no explicit loop) —
  events come in via JS callbacks the backend registered, those
  call signals, those cascade.
- **Generator runtimes** — your backend's loop is a network
  loop: read inbound event messages from the device, dispatch
  the matching closures, write outbound command updates.

The framework doesn't run a loop itself. It runs *during* the
walker pass and *during* signal cascades — both synchronous,
both driven by whatever event source you wired in.

## Where to read more

- [The shipped backends](#) — high-level overview of web, iOS,
  Android, Roku, and the runtime-server dev backend. Useful for seeing how
  each model maps to a real platform.
- [Reactivity](#) — what's happening on the framework side when
  your `update_text` or `apply_style` gets called.
- [Styles](#) — the `StyleRules` you receive in `apply_style`
  and the theme-token machinery you may want to implement.
- [Lists](#) — the `VirtualizerCallbacks` bundle in detail.
- [Navigation](#) — what the navigator `create_*` methods are
  expected to do (and the per-screen mount/release callbacks
  they receive).
- [Robot](#) — what test-id propagation looks like (your
  backend's primitive creation can opt in by capturing the
  `test_id` field).
- [Dev tools](#) — what runtime-server expects from the wire side if you're
  writing a generator-style backend.
