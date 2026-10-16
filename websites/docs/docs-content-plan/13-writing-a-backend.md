# Writing your own backend

A backend is the piece of code that turns the framework's scene tree into something a particular platform can put on screen — DOM elements, UIViews, Android Views, BrightScript SceneGraph nodes, terminal cells, GPU draw calls, or anything else you can drive from Rust.

You'd write one when the shipped backends don't cover your target: an embedded display, a custom canvas, a server-side renderer, a platform nobody has shipped yet. Most of the framework — primitives, reactivity, styles, components, hot reload, navigation — works the same against your backend as against the built-in ones. The seam is small, and it is deliberately layered so you can ship something working long before you support everything.

This page walks the two layers of that seam, explains the two execution models (runtime vs generator), and shows the shape of a minimum viable implementation.

## Two layers: Host, then capabilities

The seam is split in two, and the split is the thing to internalize.

The FIRST layer is `runtime_scene::Host`: the structural contract. It owns the `Node` associated type and the handful of operations the mount drivers themselves perform — attaching children, splicing, clearing, and minting a reactive anchor. Nothing about it mentions a primitive:

```rust
use runtime_scene::Host;

pub struct MyBackend { /* your platform-specific state */ }

impl Host for MyBackend {
    type Node = MyNodeHandle;      // the platform's "thing on screen"

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) { /* … */ }
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, i: usize) { /* … */ }
    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) { /* … */ }
    fn clear_children(&mut self, node: &Self::Node) { /* … */ }
    fn create_anchor(&mut self) -> Self::Node { /* … */ }
    fn supports_splice(&self) -> bool { /* … */ }

    // `insert_many` defaults to per-child `insert`; override it if
    // your platform has a batched insertion path (a
    // `DocumentFragment` on web).
}
```

The SECOND layer is `runtime_vocabulary::caps`: a family of narrow capability traits, each a subtrait of `Host`, each covering one primitive family. A primitive's mount handler is generic over exactly the capabilities it uses, so a backend supports a primitive precisely when it implements that primitive's trait.

That is what makes the surface progressive. There is no mega-trait with thirty `unimplemented!()` methods to stare at: you implement `Host`, then add capability traits one at a time, and each one immediately unlocks its primitives.

## type Node

The associated type `Node: Clone` is whatever your platform uses to represent a thing on screen. Pick the shape that's most useful for your backend's internal state:

- Web uses `web_glue::dom::Node` — a cheap-to-clone handle to the DOM
  node, from the framework's own JS bindings.
- iOS uses a strong reference to a `UIView` subclass through `objc2`.
- Android uses a JNI global ref to a `View`.
- Roku uses a `NodeId` — a u64 the device-side runtime maps back to a SceneGraph node.

The framework treats `Node` opaquely: handlers mint one, the drivers hold onto it and hand it back to `insert` / `insert_at` / `remove_child` / `clear_children`. The backend is free to put whatever it likes inside.

`Clone` is required because structural regions retain node handles across effect fires — a spliced region has to remove the exact nodes it inserted.

## supports_splice — the one behavioral switch

Most of `Host` is mechanical. One method is a real decision: `supports_splice` answers "can this host insert and remove children at an index inside a real parent?"

- `true` — style-less reactive regions go ANCHORLESS. A reactive `if` or `for` splices its nodes directly into the surrounding parent, so there is no wrapper in the tree. This is what web client-side rendering and splice-capable native backends do.
- `false` — every reactive region nests under a `create_anchor()` node the drivers swap subtrees under. Server-side rendering and hydration take this path deliberately: the client's adopt walk needs a stable node to claim.

Get this right before anything else — it decides the SHAPE of the tree your other methods will see.

## The capability traits

Every trait below is a subtrait of `Host` and groups the operations one primitive family needs. The authoritative per-method contract is each trait's own rustdoc in `crates/runtime/vocabulary/src/caps/`; `COVERAGE.md` next to it maps every method to the handler that calls it.

- Structure + input — `ViewOps`, `InputOps`, `PressableOps`.
- Text — `TextOps`, `ButtonOps`.
- Media — `ImageOps`, `IconOps`, `LinkOps`.
- Widgets — `TextInputOps`, `ToggleOps`, `SliderOps`, `ActivityIndicatorOps`.
- Scrolling + lists — `ScrollOps`, `SafeAreaOps`, `VirtualizerOps`.
- Layers + navigation — `PortalOps`, `PresenceOps`, `NavigatorOps`, `GraphicsOps`.
- Styling + assets — `StyleOps`, `AssetOps`.
- Payload escape hatch — `ExternalOps`, `DocumentOps` (the "render something opaque" and document-level surfaces).
- App shell — `AppEnvOps`, `LifecycleOps`, `BatchOps`.
- Tooling — `A11yOps`, `AnimationOps`, `IntrospectionOps`, `WireBindingOps`.

There is one umbrella: `AllCaps`, a blanket-implemented supertrait of every trait above. `runtime_vocabulary::register_builtins::<B: AllCaps>()` bounds on it, so a backend that implements the whole family gets the full built-in primitive vocabulary registered in one call.

Until then, register the subset you support. A handler generic over `H: StyleServices + TextOps` compiles against any backend that has those two — which is also why third-party SDKs written that way work on your backend the day you implement the traits they name. See [third-party primitives](#).

## The two execution models

There are two distinct ways a backend can do its job. Pick the one that fits the target platform.

## Runtime backends

The default model. The backend manipulates native widgets directly, in process, as the drivers hand it operations.

- A view handler immediately allocates a `<div>` / `UIView` / Android `View`.
- `insert(parent, child)` immediately calls `appendChild` / `addSubview` / `addView`.
- A text update immediately mutates the widget's text property.
- `StyleOps::apply_style(node, rules)` immediately writes CSS / view properties / drawable attributes.

The shipped web, iOS, Android, macOS, terminal, CPU and GPU backends are all runtime backends. They run in the same process as your `app()` function; when a signal changes, the world's flush re-runs the effect, the effect calls into the backend, and the backend mutates the widget on the spot.

## Generator backends

The unusual model. The backend has no direct access to a native widget tree, because the real renderer lives somewhere else — on a different device, in a different process, behind a serialization boundary.

Instead of manipulating widgets, a generator backend emits a wire stream of commands that a remote runtime replays. A view handler mints a `NodeId` and emits a `Create(NodeId, View)`; `insert(parent, child)` emits an `Insert(parent_id, child_id)`. And so on.

`backend-roku` is the shipped generator backend. Roku devices don't run Rust — the only language the runtime understands is BrightScript. The backend runs on the developer's host machine; commands stream to a thin client on the device, which replays them against SceneGraph nodes.

A generator backend has one extra obligation, and it is the interesting part of this model.

## Closures don't ship

A runtime backend can capture a Rust closure from the framework and call it directly. A generator backend can't — the closure lives in the host's memory, and the device side has no way to invoke it.

For event handlers that's a round-trip: the device sends an event-fired message back to the host, the host dispatches the closure in-process, the resulting writes stage, and the `settle()` step (drain microtasks + flush) commits them before the outbound command queue is drained. That boundary — "event → staged writes → flush → emitted commands" — is the embedder contract.

For reactive expressions (a text node whose content reads a signal, a conditional's predicate) the closure can't be re-evaluated on the device at all. The framework therefore also publishes a STRUCTURED view of those expressions: a stable method name the device runtime maps to a transpiled function, plus the signal ids that method reads. `WireBindingOps` is the capability trait that receives them:

```rust
impl WireBindingOps for MyBackend {
    fn note_text_binding(&mut self, node: &Self::Node,
                         signal_ids: &[u64], method: &'static str) {
        // Emit "this node's text is computed by `method` from these signals".
    }
    // …plus the conditional / switch / repeat siblings.
}
```

Runtime backends leave every `WireBindingOps` method at its default no-op — they re-run the closures locally on each flush, so no metadata is needed.

## A skeleton backend

The smallest plausible backend is `Host` plus the two or three capability traits your app's primitives actually use:

```rust
use std::rc::Rc;
use runtime_scene::Host;
use runtime_vocabulary::caps::{StyleOps, TextOps, ViewOps};
use runtime_shared::StyleRules;

#[derive(Clone)]
struct Node { /* whatever your platform uses */ }

pub struct MyBackend { /* root container, caches, … */ }

impl Host for MyBackend {
    type Node = Node;

    fn insert(&mut self, parent: &mut Node, child: Node) { /* attach */ }
    fn insert_at(&mut self, parent: &mut Node, child: Node, i: usize) { /* … */ }
    fn remove_child(&mut self, parent: &Node, child: &Node) { /* detach */ }
    fn clear_children(&mut self, node: &Node) { /* detach all */ }
    fn create_anchor(&mut self) -> Node { /* layout-transparent container */ }
    fn supports_splice(&self) -> bool { true }
}

impl ViewOps for MyBackend { /* create_view, … */ }
impl TextOps for MyBackend { /* create_text, update_text, … */ }
impl StyleOps for MyBackend {
    fn apply_style(&mut self, node: &Node, style: &Rc<StyleRules>) {
        // Translate StyleRules into your platform's styling.
    }
}
```

That compiles and produces a working app for any tree built from views, text, and styles. Adding an image means adding `ImageOps`; adding a scroll region means `ScrollOps`. You never have to write a method you don't intend to implement, and a primitive whose capability you haven't implemented is a COMPILE error at the registration site rather than a runtime panic in the field.

That progressive shape is deliberate. You can ship a backend for an unusual target with the minimum surface working in a day, and grow it as you need more primitives.

## Driving the render

A boot entry does six things in order: forward your environment capabilities into the ambient thread-locals, build a `Registry` and register the primitive handlers on it, create the `World`, run the app's root component inside `World::enter`, realize the returned scene tree against your backend, and install the flush driver. Everything you need comes from one module — `runtime_vocabulary::backend`:

```rust
use std::cell::RefCell;
use std::rc::Rc;

use runtime_vocabulary::backend::{
    install_env_services, realize, register_builtins_with,
    BuiltinSet, Registry, World,
};

// Generic over `S` on purpose: an entry that pins `AllBuiltins`
// re-anchors the whole primitive vocabulary, so an app that
// selected a smaller set pays for handlers it never mounts.
fn boot<S: BuiltinSet>() {
    let backend = Rc::new(RefCell::new(MyBackend::new(/* platform args */)));

    // `platform()`, `color_scheme()`, `open_url()`,
    // `set_fullscreen()` and `announce()` read thread-local
    // slots; this fills them from your `AppEnvOps` / `A11yOps`
    // impls. It must precede the build — a component body may
    // read `platform()` while constructing.
    install_env_services(&backend);

    let mut registry = Registry::new();
    register_builtins_with::<MyBackend, S>(&mut registry);
    // …plus any third-party SDK registers the app hands you.
    let registry = Rc::new(registry);

    let world = World::new();
    let realized = world.enter(|| {
        let element = my_app::app();
        realize(&backend, &registry, element)
    });

    // Hold `world` + `realized` for the app's lifetime; dropping
    // them tears the tree down, and `realized` must drop FIRST —
    // it unmounts before the `World` that owns the slots its
    // effects read. Then install the flush driver so staged
    // writes commit after each dispatch, and run the platform's
    // event loop.
}
```

In practice you don't hand-roll this: each shipped backend exposes a `newcore` boot module that does it for you (`backend_web::``newcore::start_in`, `host_appkit::newcore::run`, `host_winit::newcore::run_with`, …), and a new backend should ship the same shape. Every one of them takes a `register` argument that runs after `register_builtins`, which is the app's SDK-registration seam.

The flush driver is the piece that is easy to forget. Wrap every author-code entry point your backend owns — event callbacks, timers, animation frames — so the world flushes after the callback returns. Without it, an author's `set` stages and never commits, and the UI simply doesn't update. See [Reactivity](#) for what the flush does.

Also wire a viewport source where the platform has one: push the platform's resize / rotation / configuration-change report into the world's viewport context, so breakpoint-reactive author code re-fires. Backends with no resize surface at all (a command stream, a fixed-size display) simply seed once and document that.

What "the event loop" means is platform-specific:

- Native event loops (iOS's `UIApplicationMain`, Android's `ActivityThread`, a winit loop) — the platform runs the loop, your event callbacks call `signal.set(...)`, your flush driver commits, the effects run through the backend.
- Reactive runtimes (web, where there's no explicit loop) — events arrive via JS callbacks the backend registered, those write signals, the microtask flush driver commits.
- Generator runtimes — your loop is a network loop: read inbound event messages from the device, dispatch the matching closures, `settle()`, then drain the outbound command queue.

The framework doesn't run a loop itself. It runs during realize and during flushes — both synchronous, both driven by whatever event source you wired in.

## Where to read more

- [The shipped backends](#) — high-level overview of web, iOS, Android, macOS, terminal, GPU, Roku, and the runtime-server dev backend. Useful for seeing how each model maps to a real platform.
- [Reactivity](#) — what's happening on the framework side between a signal write and your `apply_style` call.
- [Styles](#) — the `StyleRules` you receive in `apply_style` and the token-resolution machinery you may want to implement.
- [Lists](#) — what `VirtualizerOps` is expected to do.
- [Navigation](#) — what `NavigatorOps` is expected to do, and the per-screen mount/release callbacks the navigator host bundle carries.
- [Third-party primitives](#) — how a handler declares the capabilities it needs, which is why SDKs work on a new backend for free.
- [Robot](#) — what test-id propagation looks like, and the introspection surface `IntrospectionOps` exposes.
- [Dev tools](#) — what runtime-server expects from the wire side if you're writing a generator-style backend.

