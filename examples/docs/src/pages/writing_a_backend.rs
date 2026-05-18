//! Writing your own backend — built via the `docs!` macro.
//!
//! Demonstrates the macro end-to-end: one `docs!` invocation emits
//! `pub fn page() -> Primitive` and `pub static PAGE_META: PageMeta`.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "writing-a-backend",
    title = "Writing your own backend",
    category = Reference,
    description = "Translate the framework's Primitive tree into something a platform can put on screen.",
    related = ["backends", "cli", "primitives", "styles"],
    concepts = [Backend, RuntimeBackend, GeneratorBackend, LazySlotCapture, WireProtocol],

    section(heading = "Overview") {
        p("A backend is the piece of code that translates the framework's ",
          code("Primitive"), " tree into something a particular platform can \
           put on screen — DOM elements, UIViews, Android Views, BrightScript \
           SceneGraph nodes, or anything else you can drive from Rust."),
        p("You'd write one when the shipped backends don't cover your target: \
           a terminal renderer, an embedded display, a custom GPU canvas, a \
           server-side HTML renderer, a platform we haven't shipped yet. Most \
           of the framework — primitives, reactivity, styles, components, hot \
           reload, navigation — works the same against your backend as it \
           does against the built-in ones. The seam is small."),
        p("This page walks the trait, explains the two execution models \
           (runtime vs generator), and shows the shape of a minimum viable \
           implementation."),
    },

    section(heading = "The Backend trait") {
        p("A backend implements one trait — ", code("framework_core::Backend"),
          ". The declaration is short:"),

        code(rust, r##"
            use framework_core::{Backend, /* primitives, styles, etc. */};

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
        "##),

        p("The methods above are the required ones — no default \
           implementation. They cover the smallest set the framework needs to \
           put a tree on screen: create views, create text, create buttons, \
           attach children, update text content, clear a container's \
           children, apply styles, finalize the root mount."),
        p("Everything else has a default. Most are either no-ops (for things \
           that don't apply to your platform) or ", code("unimplemented!()"),
          " (for primitives you don't support yet, which causes a clear panic \
           if the app uses one). You can ship a backend that supports only \
           the above and progressively fill in the rest."),
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
        p("The framework treats ", code("Node"), " opaquely. It calls ",
          code("create_*"), " to mint one, holds onto it, hands it back to ",
          code("insert"), " / ", code("update_text"), " / ",
          code("clear_children"), " / ", code("apply_style"),
          ". The backend is free to put whatever it likes inside."),
    },

    section(heading = "The two execution models") {
        p("There are two distinct ways a backend can do its job. Pick the \
           one that fits the target platform."),
    },

    section(heading = "Runtime backends") {
        p("The default model. The backend manipulates native widgets \
           directly, in process, as the framework hands it operations."),
        list(
            [code("create_view()"), " immediately allocates a ", code("<div>"),
             " / ", code("UIView"), " / Android ", code("View"), "."],
            [code("insert(parent, child)"), " immediately calls ",
             code("appendChild"), " / ", code("addSubview"), " / ",
             code("addView"), "."],
            [code("update_text(node, content)"),
             " immediately mutates the widget's text property."],
            [code("apply_style(node, rules)"),
             " immediately writes CSS / view properties / drawable attributes."],
        ),
        p("The shipped web, iOS, and Android backends are all runtime \
           backends. They run in the same process as your ", code("app()"),
          " function; when a signal changes, the framework re-fires the \
           effect, the effect calls ", code("update_text(...)"),
          ", and the backend mutates the widget on the spot."),
        p("If you're writing a backend for any traditional GUI platform — \
           desktop, mobile, embedded — runtime is the model you want."),
    },

    section(heading = "Generator backends") {
        p("The unusual model. The backend doesn't have direct access to a \
           native widget tree. It exists because the real renderer lives \
           somewhere else — on a different device, in a different process, \
           behind a serialization boundary."),
        p("Instead of manipulating widgets, a generator backend emits a wire \
           stream of commands that a remote runtime replays. The framework \
           calls ", code("create_view()"),
          "; the backend mints a ", code("NodeId"), " and emits a ",
          code("Create(NodeId, View)"), " command. The framework calls ",
          code("insert(parent, child)"), "; the backend emits an ",
          code("Insert(parent_id, child_id)"), ". And so on."),
        p(code("backend-roku"),
          " is the only shipped generator backend. Roku devices don't run \
           Rust — the only language the runtime understands is BrightScript. \
           The backend runs on the developer's host machine; commands stream \
           to a thin client on the device, which replays them against \
           SceneGraph nodes."),
        p("The shape implies two extra constraints generator backends have \
           to handle."),
    },

    section(heading = "Closures don't ship") {
        p("A runtime backend can capture a ", code("Box<dyn Fn()>"),
          " from the framework and call it directly. A generator backend \
           can't — the closure exists in the host's memory, and the device \
           side has no way to invoke it."),
        p("For event handlers, this means the device sends an event-fired \
           message back to the host, which dispatches the closure in-process. \
           The wire protocol carries that round-trip."),
        p("For reactive expressions (a ", code("Text"),
          " whose content reads a signal, a ", code("When"),
          "'s condition), the closure can't be re-evaluated on the device — \
           so the framework provides a structured view of those expressions \
           through ", code("Derived<T>"), " and ", code("Action"),
          ". Each carries a ", code("method: &'static str"),
          " (a stable name the device runtime maps to a transpiled \
           BrightScript function) plus an ", code("inputs: Vec<u64>"),
          " (the signal ids the method reads). Generator backends consume \
           the structured form via the ", code("note_*_binding"), " hooks:"),

        code(rust, r##"
            fn note_text_binding(&mut self, node: &Self::Node,
                                 signal_ids: &[u64], method: &'static str) {
                // Emit a "this node's text is computed by `method` from these signals" command
            }

            fn note_when_binding(&mut self, anchor: &Self::Node,
                                 signal_ids: &[u64], cond_method: &'static str,
                                 then_node: &Self::Node, otherwise_node: &Self::Node) {
                // Emit a "this anchor toggles between these two subtrees based on `cond_method`"
            }
        "##),

        p("Runtime backends leave these defaults at no-op — they re-run the \
           closures locally on every signal change, no metadata needed."),
    },

    section(heading = "Inactive subtrees shouldn't materialize") {
        p("A runtime backend can afford to build both branches of a ",
          code("when()"),
          " up front and just hide the inactive one — it's a cheap local \
           operation. A generator backend can't — building means emitting \
           commands over a network, and shipping a subtree that's not on \
           screen wastes bandwidth and device memory."),
        p("Generator backends opt into lazy slot capture to handle this. The \
           pattern:"),

        code(rust, r##"
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
        "##),

        p("With lazy slot capture on, the framework builds each conditional \
           arm's subtree and the backend stores it as a template rather than \
           sending it. When the condition flips on the device, the \
           device-side runtime replays the relevant template's commands."),
        p("Runtime backends leave ", code("supports_lazy_slot_capture"),
          " at ", code("false"),
          " and the framework builds every branch into the live tree \
           directly — cheap on-platform, no capture needed."),
    },

    section(heading = "The full method tour") {
        p("Here's what's in the trait, grouped by purpose. Methods without \
           notes are \"create + update\" pairs for a specific primitive."),
    },

    section(heading = "Required (no default)") {
        list(
            [code("type Node: Clone")],
            [code("create_view"), ", ", code("create_text"), ", ",
             code("create_button"), ", ", code("insert")],
            [code("update_text")],
            [code("clear_children")],
            [code("apply_style")],
            [code("finish"),
             " — called once after the initial render walk to let the \
              backend do any final mount work (web's ", code("finish"),
             " triggers a layout flush, iOS's attaches the root view to the ",
             code("UIWindow"), ")."],
        ),
    },

    section(heading = "Container primitives (defaults: create_view)") {
        list(
            [code("create_pressable(on_click)"),
             " — a tappable container. The default falls back to ",
             code("create_view"),
             ", which means clicks won't fire but the subtree renders."],
            [code("create_reactive_anchor"), " — placeholder node for ",
             code("when"), " / ", code("switch"),
             " branches. Web overrides this to return a ",
             code("display: contents"),
             " element so the branch's children inherit flex context."],
        ),
    },

    section(heading = "Content primitives (defaults: unimplemented!())") {
        list(
            [code("create_image"), ", ", code("update_image_src")],
            [code("create_icon"), ", ", code("update_icon_color"), ", ",
             code("update_icon_stroke"), ", ", code("animate_icon_stroke")],
            [code("create_video"), ", ", code("update_video_src")],
            [code("create_web_view"), ", ", code("update_web_view_url")],
            [code("create_activity_indicator")],
        ),
        p("The walker only calls these if your app uses the corresponding \
           primitive. Leave them ", code("unimplemented!()"),
          " until you support the primitive."),
    },

    section(heading = "Inputs (defaults: unimplemented!())") {
        list(
            [code("create_text_input"), ", ", code("update_text_input_value")],
            [code("create_toggle"), ", ", code("update_toggle_value")],
            [code("create_slider"), ", ", code("update_slider_value")],
            [code("update_button_label")],
        ),
    },

    section(heading = "Navigation (defaults: unimplemented!())") {
        list(
            [code("create_navigator"), " — stack navigator"],
            [code("create_tab_navigator")],
            [code("create_drawer_navigator")],
            [code("create_link")],
        ),
        p("These take a callbacks bundle so the framework can ask the \
           backend to mount/release per-screen subtrees on demand. The shape \
           is large; the trait's source has annotated examples."),
    },

    section(heading = "Overlays (defaults: unimplemented!())") {
        list(
            [code("create_overlay"), ", ", code("create_anchored_overlay")],
        ),
        p("Backends decide how to route these — iOS could send ",
          code("Overlay"), " to a window-level ", code("UIView"), " and ",
          code("AnchoredOverlay"), " to ",
          code("UIContextMenuInteraction"), ". Web uses the ",
          code("popover"), " attribute with CSS anchor positioning."),
    },

    section(heading = "Styling") {
        list(
            [code("apply_style(node, &Rc<StyleRules>)"),
             " — required. The framework hands you concrete, theme-resolved values."],
            [code("apply_styled_states(node, base, overlays)"),
             " — optional. If your backend supports declarative state \
              styling (web's CSS pseudo-classes), implement this and return ",
             code("true"), " from ", code("handles_states_natively()"),
             ". The framework then hands you the base + per-state overlay \
              rules in one call. If you leave the default, the framework \
              drives states via signal flips and re-fires ",
             code("apply_style"), " per change."],
            [code("register_stylesheet(&[Rc<StyleRules>])"),
             " — optional. Backends that benefit from up-front rule emission \
              (web mints CSS classes here) override this. Defaults to a no-op."],
            [code("unregister_stylesheet(&[Rc<StyleRules>])"),
             " — paired teardown."],
            [code("install_theme_variables(&[TokenEntry])"),
             " — optional. Backends with a runtime variable system (web's \
              CSS custom properties) install tokens here. iOS and Android \
              leave the default no-op and read ", code("Tokenized::value()"),
             " at ", code("apply_style"), " time. See ",
             link("Styles", to = "styles"), " for the full story."],
        ),
    },

    section(heading = "Lazy-slot capture (generator backends only)") {
        list(
            [code("supports_lazy_slot_capture(&self) -> bool"), " — default ",
             code("false"), "."],
            [code("begin_slot_capture"), " / ", code("end_slot_capture"),
             " — pair the framework calls around each conditional arm's \
              subtree build."],
            [code("note_text_binding"), ", ", code("note_signal_initial"),
             ", ", code("note_when_binding"), ", ",
             code("note_switch_binding"), ", ", code("note_repeat_binding"),
             " — declarative metadata hooks. Generator backends record these \
              so the device runtime can re-evaluate reactive expressions \
              without closures."],
        ),
    },

    section(heading = "Refs and handles") {
        list(
            [code("ref_ops()"), " — returns a ", code("RefOps"),
             " bundle with the per-primitive trait objects the framework \
              uses to construct handles (", code("ButtonHandle"), ", ",
             code("ViewHandle"),
             ", etc.). The defaults are no-op ops, so refs work but the \
              handle methods don't do anything. Implement the relevant \
              traits and return them here when you want geometry queries, \
              programmatic clicks, etc., to work."],
            [code("make_*_handle"),
             " for each primitive — defaults construct no-op handles. \
              Override per-primitive to return real backend-aware handles."],
        ),
    },

    section(heading = "Virtualization") {
        list(
            [code("create_virtualizer(callbacks: VirtualizerCallbacks<Self::Node>)"),
             " — defaults to ", code("unimplemented!()"),
             ". The framework hands you a bundle of closures (",
             code("item_count"), ", ", code("mount_item"), ", ",
             code("release_item"),
             ", etc.) and you wire them into your platform's recycling \
              widget (", code("UICollectionView"), ", ",
             code("RecyclerView"), ", an ", code("IntersectionObserver"),
             "). See ", link("Lists", to = "lists"),
             " for what each callback carries."],
        ),
    },

    section(heading = "A skeleton backend") {
        p("The smallest plausible backend looks like this:"),

        code(rust, r##"
            use std::rc::Rc;
            use framework_core::{Backend, Action, StyleRules, IconData};

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
        "##),

        p("This compiles and produces a working app — as long as your app \
           uses only ", code("View"), ", ", code("Text"), ", and ",
          code("Button"), ". Trying to use anything else (an ", code("Image"),
          ", a ", code("ScrollView"),
          ", navigation) will panic with ", code("unimplemented!()"),
          " at the relevant call site, telling you exactly what to \
           implement next."),
        p("That progressive shape is deliberate. You can ship a backend for \
           an unusual target with the minimum surface working in a day, and \
           grow it as you need more primitives."),
    },

    section(heading = "Driving the render") {
        p("Once your backend is built, hand it to the framework:"),

        code(rust, r##"
            use framework_core::{render, Owner};

            fn main() {
                let mut backend = MyBackend::new(/* platform args */);
                let _owner = render(&mut backend, my_app::app());
                // ...platform-specific event loop here...
            }
        "##),

        p(code("render(backend, root)"),
          " walks the primitive tree, calling your backend's methods in the \
           right order. The returned ", code("Owner"),
          " holds the reactive scope; drop it to tear everything down."),
        p("What \"the event loop\" means is platform-specific:"),
        list(
            ["Native event loops (iOS's ", code("UIApplicationMain"),
             ", Android's ", code("ActivityThread"),
             ", a winit loop) — the platform runs the loop, your event \
              callbacks call ", code("signal.set(...)"),
             ", the framework cascades effects through the backend."],
            ["Reactive runtimes (web, where there's no explicit loop) — \
              events come in via JS callbacks the backend registered, those \
              call signals, those cascade."],
            ["Generator runtimes — your backend's loop is a network loop: \
              read inbound event messages from the device, dispatch the \
              matching closures, write outbound command updates."],
        ),
        p("The framework doesn't run a loop itself. It runs during the \
           walker pass and during signal cascades — both synchronous, both \
           driven by whatever event source you wired in."),
    },

    section(heading = "Where to read more") {
        list(
            [link("The shipped backends", to = "backends"),
             " — high-level overview of web, iOS, Android, Roku, and the AAS \
              dev backend. Useful for seeing how each model maps to a real \
              platform."],
            [link("Reactivity", to = "reactivity"),
             " — what's happening on the framework side when your ",
             code("update_text"), " or ", code("apply_style"),
             " gets called."],
            [link("Styles", to = "styles"), " — the ", code("StyleRules"),
             " you receive in ", code("apply_style"),
             " and the theme-token machinery you may want to implement."],
            [link("Lists", to = "lists"), " — the ",
             code("VirtualizerCallbacks"), " bundle in detail."],
            [link("Navigation", to = "navigation"),
             " — what the navigator ", code("create_*"),
             " methods are expected to do (and the per-screen mount/release \
              callbacks they receive)."],
            [link("Robot", to = "robot"),
             " — what test-id propagation looks like (your backend's \
              primitive creation can opt in by capturing the ",
             code("test_id"), " field)."],
            [link("Dev tools", to = "cli"),
             " — what AAS expects from the wire side if you're writing a \
              generator-style backend."],
        ),
    },
}
