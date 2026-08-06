//! Writing your own backend — built via the `docs!` macro.
//!
//! Demonstrates the macro end-to-end: one `docs!` invocation emits
//! `pub fn page() -> Element` and `pub static PAGE_META: PageMeta`.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "writing-a-backend",
    title = "Writing your own backend",
    category = Reference,
    description = "Translate the framework's scene tree into something a platform can put on screen.",
    related = ["backends", "cli", "primitives", "styles"],
    concepts = [Backend, RuntimeBackend, GeneratorBackend, LazySlotCapture, WireProtocol],

    section(heading = "Overview") {
        p("A backend is the piece of code that turns the framework's scene \
           tree into something a particular platform can put on screen — DOM \
           elements, UIViews, Android Views, BrightScript SceneGraph nodes, \
           terminal cells, GPU draw calls, or anything else you can drive \
           from Rust."),
        p("You'd write one when the shipped backends don't cover your target: \
           an embedded display, a custom canvas, a server-side renderer, a \
           platform nobody has shipped yet. Most of the framework — \
           primitives, reactivity, styles, components, hot reload, navigation \
           — works the same against your backend as against the built-in \
           ones. The seam is small, and it is deliberately layered so you can \
           ship something working long before you support everything."),
        p("This page walks the two layers of that seam, explains the two \
           execution models (runtime vs generator), and shows the shape of a \
           minimum viable implementation."),
    },

    section(heading = "Two layers: Host, then capabilities") {
        p("The seam is split in two, and the split is the thing to \
           internalize."),
        p("The FIRST layer is ", code("runtime_scene::Host"),
          ": the structural contract. It owns the ", code("Node"),
          " associated type and the handful of operations the mount drivers \
           themselves perform — attaching children, splicing, clearing, and \
           minting a reactive anchor. Nothing about it mentions a primitive:"),

        code(rust, r##"
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
        "##),

        p("The SECOND layer is ", code("runtime_vocabulary::caps"),
          ": a family of narrow capability traits, each a subtrait of ",
          code("Host"),
          ", each covering one primitive family. A primitive's mount handler \
           is generic over exactly the capabilities it uses, so a backend \
           supports a primitive precisely when it implements that \
           primitive's trait."),
        p("That is what makes the surface progressive. There is no mega-trait \
           with thirty ", code("unimplemented!()"),
          " methods to stare at: you implement ", code("Host"),
          ", then add capability traits one at a time, and each one \
           immediately unlocks its primitives."),
    },

    section(heading = "type Node") {
        p("The associated type ", code("Node: Clone"),
          " is whatever your platform uses to represent a thing on screen. \
           Pick the shape that's most useful for your backend's internal state:"),
        list(
            ["Web uses ", code("web_sys::Node"), " (with an ", code("Rc"),
             " inside for cheap cloning)."],
            ["iOS uses a strong reference to a ", code("UIView"),
             " subclass through ", code("objc2"), "."],
            ["Android uses a JNI global ref to a ", code("View"), "."],
            ["Roku uses a ", code("NodeId"), " — a u64 the device-side \
             runtime maps back to a SceneGraph node."],
        ),
        p("The framework treats ", code("Node"),
          " opaquely: handlers mint one, the drivers hold onto it and hand \
           it back to ", code("insert"), " / ", code("insert_at"), " / ",
          code("remove_child"), " / ", code("clear_children"),
          ". The backend is free to put whatever it likes inside."),
        p(code("Clone"),
          " is required because structural regions retain node handles \
           across effect fires — a spliced region has to remove the exact \
           nodes it inserted."),
    },

    section(heading = "supports_splice — the one behavioral switch") {
        p("Most of ", code("Host"),
          " is mechanical. One method is a real decision: ",
          code("supports_splice"),
          " answers \"can this host insert and remove children at an index \
           inside a real parent?\""),
        list(
            [code("true"),
             " — style-less reactive regions go ANCHORLESS. A reactive ",
             code("if"), " or ", code("for"),
             " splices its nodes directly into the surrounding parent, so \
              there is no wrapper in the tree. This is what web client-side \
              rendering and splice-capable native backends do."],
            [code("false"),
             " — every reactive region nests under a ",
             code("create_anchor()"),
             " node the drivers swap subtrees under. Server-side rendering \
              and hydration take this path deliberately: the client's adopt \
              walk needs a stable node to claim."],
        ),
        p("Get this right before anything else — it decides the SHAPE of the \
           tree your other methods will see."),
    },

    section(heading = "The capability traits") {
        p("Every trait below is a subtrait of ", code("Host"),
          " and groups the operations one primitive family needs. The \
           authoritative per-method contract is each trait's own rustdoc in ",
          code("crates/runtime/vocabulary/src/caps/"), "; ",
          code("COVERAGE.md"),
          " next to it maps every method to the handler that calls it."),
        list(
            ["Structure + input — ", code("ViewOps"), ", ", code("InputOps"),
             ", ", code("PressableOps"), "."],
            ["Text — ", code("TextOps"), ", ", code("ButtonOps"), "."],
            ["Media — ", code("ImageOps"), ", ", code("IconOps"), ", ",
             code("LinkOps"), "."],
            ["Widgets — ", code("TextInputOps"), ", ", code("ToggleOps"),
             ", ", code("SliderOps"), ", ", code("ActivityIndicatorOps"), "."],
            ["Scrolling + lists — ", code("ScrollOps"), ", ",
             code("SafeAreaOps"), ", ", code("VirtualizerOps"), "."],
            ["Layers + navigation — ", code("PortalOps"), ", ",
             code("PresenceOps"), ", ", code("NavigatorOps"), ", ",
             code("GraphicsOps"), "."],
            ["Styling + assets — ", code("StyleOps"), ", ",
             code("AssetOps"), "."],
            ["Payload escape hatch — ", code("ExternalOps"), ", ",
             code("DocumentOps"), " (the \"render something opaque\" and \
              document-level surfaces)."],
            ["App shell — ", code("AppEnvOps"), ", ", code("LifecycleOps"),
             ", ", code("BatchOps"), "."],
            ["Tooling — ", code("A11yOps"), ", ", code("AnimationOps"), ", ",
             code("IntrospectionOps"), ", ", code("WireBindingOps"), "."],
        ),
        p("There is one umbrella: ", code("AllCaps"),
          ", a blanket-implemented supertrait of every trait above. ",
          code("runtime_vocabulary::register_builtins::<B: AllCaps>()"),
          " bounds on it, so a backend that implements the whole family \
           gets the full built-in primitive vocabulary registered in one \
           call."),
        p("Until then, register the subset you support. A handler generic \
           over ", code("H: StyleServices + TextOps"),
          " compiles against any backend that has those two — which is also \
           why third-party SDKs written that way work on your backend the \
           day you implement the traits they name. See ",
          link("third-party primitives", to = "third-party-primitives"), "."),
    },

    section(heading = "The two execution models") {
        p("There are two distinct ways a backend can do its job. Pick the \
           one that fits the target platform."),
    },

    section(heading = "Runtime backends") {
        p("The default model. The backend manipulates native widgets \
           directly, in process, as the drivers hand it operations."),
        list(
            ["A view handler immediately allocates a ", code("<div>"),
             " / ", code("UIView"), " / Android ", code("View"), "."],
            [code("insert(parent, child)"), " immediately calls ",
             code("appendChild"), " / ", code("addSubview"), " / ",
             code("addView"), "."],
            ["A text update immediately mutates the widget's text property."],
            [code("StyleOps::apply_style(node, rules)"),
             " immediately writes CSS / view properties / drawable attributes."],
        ),
        p("The shipped web, iOS, Android, macOS, terminal, CPU and GPU \
           backends are all runtime backends. They run in the same process \
           as your ", code("app()"),
          " function; when a signal changes, the world's flush re-runs the \
           effect, the effect calls into the backend, and the backend \
           mutates the widget on the spot."),
    },

    section(heading = "Generator backends") {
        p("The unusual model. The backend has no direct access to a native \
           widget tree, because the real renderer lives somewhere else — on \
           a different device, in a different process, behind a \
           serialization boundary."),
        p("Instead of manipulating widgets, a generator backend emits a wire \
           stream of commands that a remote runtime replays. A view handler \
           mints a ", code("NodeId"), " and emits a ",
          code("Create(NodeId, View)"), "; ", code("insert(parent, child)"),
          " emits an ", code("Insert(parent_id, child_id)"), ". And so on."),
        p(code("backend-roku"),
          " is the shipped generator backend. Roku devices don't run Rust — \
           the only language the runtime understands is BrightScript. The \
           backend runs on the developer's host machine; commands stream to \
           a thin client on the device, which replays them against \
           SceneGraph nodes."),
        p("A generator backend has one extra obligation, and it is the \
           interesting part of this model."),
    },

    section(heading = "Closures don't ship") {
        p("A runtime backend can capture a Rust closure from the framework \
           and call it directly. A generator backend can't — the closure \
           lives in the host's memory, and the device side has no way to \
           invoke it."),
        p("For event handlers that's a round-trip: the device sends an \
           event-fired message back to the host, the host dispatches the \
           closure in-process, the resulting writes stage, and the ",
          code("settle()"),
          " step (drain microtasks + flush) commits them before the outbound \
           command queue is drained. That boundary — \"event → staged writes \
           → flush → emitted commands\" — is the embedder contract."),
        p("For reactive expressions (a text node whose content reads a \
           signal, a conditional's predicate) the closure can't be \
           re-evaluated on the device at all. The framework therefore also \
           publishes a STRUCTURED view of those expressions: a stable method \
           name the device runtime maps to a transpiled function, plus the \
           signal ids that method reads. ", code("WireBindingOps"),
          " is the capability trait that receives them:"),

        code(rust, r##"
            impl WireBindingOps for MyBackend {
                fn note_text_binding(&mut self, node: &Self::Node,
                                     signal_ids: &[u64], method: &'static str) {
                    // Emit "this node's text is computed by `method` from these signals".
                }
                // …plus the conditional / switch / repeat siblings.
            }
        "##),

        p("Runtime backends leave every ", code("WireBindingOps"),
          " method at its default no-op — they re-run the closures locally \
           on each flush, so no metadata is needed."),
    },

    section(heading = "A skeleton backend") {
        p("The smallest plausible backend is ", code("Host"),
          " plus the two or three capability traits your app's primitives \
           actually use:"),

        code(rust, r##"
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
        "##),

        p("That compiles and produces a working app for any tree built from \
           views, text, and styles. Adding an image means adding ",
          code("ImageOps"), "; adding a scroll region means ",
          code("ScrollOps"),
          ". You never have to write a method you don't intend to \
           implement, and a primitive whose capability you haven't \
           implemented is a COMPILE error at the registration site rather \
           than a runtime panic in the field."),
        p("That progressive shape is deliberate. You can ship a backend for \
           an unusual target with the minimum surface working in a day, and \
           grow it as you need more primitives."),
    },

    section(heading = "Driving the render") {
        p("A boot entry does six things in order: forward your \
           environment capabilities into the ambient thread-locals, build \
           a ", code("Registry"),
          " and register the primitive handlers on it, create the ",
          code("World"), ", run the app's root component inside ",
          code("World::enter"),
          ", realize the returned scene tree against your backend, and \
           install the flush driver. Everything you need comes from one \
           module — ", code("runtime_vocabulary::backend"), ":"),

        code(rust, r##"
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
        "##),

        p("In practice you don't hand-roll this: each shipped backend \
           exposes a ", code("newcore"),
          " boot module that does it for you (", code("backend_web::"),
          code("newcore::start_in"), ", ", code("host_appkit::newcore::run"),
          ", ", code("host_winit::newcore::run_with"),
          ", …), and a new backend should ship the same shape. Every one of \
           them takes a ", code("register"),
          " argument that runs after ", code("register_builtins"),
          ", which is the app's SDK-registration seam."),
        p("The flush driver is the piece that is easy to forget. Wrap every \
           author-code entry point your backend owns — event callbacks, \
           timers, animation frames — so the world flushes after the \
           callback returns. Without it, an author's ", code("set"),
          " stages and never commits, and the UI simply doesn't update. See ",
          link("Reactivity", to = "reactivity"),
          " for what the flush does."),
        p("Also wire a viewport source where the platform has one: push the \
           platform's resize / rotation / configuration-change report into \
           the world's viewport context, so breakpoint-reactive author code \
           re-fires. Backends with no resize surface at all (a command \
           stream, a fixed-size display) simply seed once and document that."),
        p("What \"the event loop\" means is platform-specific:"),
        list(
            ["Native event loops (iOS's ", code("UIApplicationMain"),
             ", Android's ", code("ActivityThread"),
             ", a winit loop) — the platform runs the loop, your event \
              callbacks call ", code("signal.set(...)"),
             ", your flush driver commits, the effects run through the \
              backend."],
            ["Reactive runtimes (web, where there's no explicit loop) — \
              events arrive via JS callbacks the backend registered, those \
              write signals, the microtask flush driver commits."],
            ["Generator runtimes — your loop is a network loop: read inbound \
              event messages from the device, dispatch the matching \
              closures, ", code("settle()"),
             ", then drain the outbound command queue."],
        ),
        p("The framework doesn't run a loop itself. It runs during realize \
           and during flushes — both synchronous, both driven by whatever \
           event source you wired in."),
    },

    section(heading = "Where to read more") {
        list(
            [link("The shipped backends", to = "backends"),
             " — high-level overview of web, iOS, Android, macOS, terminal, \
              GPU, Roku, and the runtime-server dev backend. Useful for \
              seeing how each model maps to a real platform."],
            [link("Reactivity", to = "reactivity"),
             " — what's happening on the framework side between a signal \
              write and your ", code("apply_style"), " call."],
            [link("Styles", to = "styles"), " — the ", code("StyleRules"),
             " you receive in ", code("apply_style"),
             " and the token-resolution machinery you may want to implement."],
            [link("Lists", to = "lists"), " — what ",
             code("VirtualizerOps"), " is expected to do."],
            [link("Navigation", to = "navigation"),
             " — what ", code("NavigatorOps"),
             " is expected to do, and the per-screen mount/release \
              callbacks the navigator host bundle carries."],
            [link("Third-party primitives", to = "third-party-primitives"),
             " — how a handler declares the capabilities it needs, which is \
              why SDKs work on a new backend for free."],
            [link("Robot", to = "robot"),
             " — what test-id propagation looks like, and the \
              introspection surface ", code("IntrospectionOps"),
             " exposes."],
            [link("Dev tools", to = "cli"),
             " — what runtime-server expects from the wire side if you're \
              writing a generator-style backend."],
        ),
    },
}
