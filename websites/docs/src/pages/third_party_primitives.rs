//! Third-party primitives page — built via the `docs!` macro.
//!
//! Covers the scene `Registry` as the extension seam, the per-host
//! handler pattern, and the umbrella-crate convention third-party SDKs
//! use to ship a primitive across multiple platforms.
//!
//! The worked reference for everything on this page is the in-tree
//! `maps` SDK (`crates/sdk/client/maps`): shared props crate, umbrella
//! facade, per-backend leaves, one `register` per host.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "third-party-primitives",
    title = "Third-party primitives",
    category = Advanced,
    description = "Ship a new primitive (with its own native FFI) without forking the framework. One seam — the scene Registry — plus a per-host handler and a small umbrella-crate convention.",
    related = ["primitives", "backends", "writing-a-backend"],
    concepts = [External],

    section(heading = "What this is for") {
        p("The framework ships a set of primitives (view, text, button, \
           overlay, image, …) as handlers on a registry, and each host \
           implements a set of narrow capability traits those handlers call \
           through. Nothing about that arrangement is closed: a primitive is \
           just a payload type plus a mount handler."),
        p("So when you want a primitive the framework doesn't ship — a ",
          code("MapView"), " that wraps MapKit on iOS and an OpenStreetMap \
           embed on web, a camera viewfinder, a Stripe card-element, an AR \
           scene — you register it the same way the built-ins are \
           registered. These are real platform things with no business \
           living in the framework, but they behave like primitives at the \
           call site: styles, refs, scope-tied cleanup, the works."),
        p("There is no separate \"external\" concept to learn. The scene ",
          code("Registry"),
          " treats first-party primitives and third-party ones uniformly."),
    },

    section(heading = "The shape, at a glance") {
        p("An SDK is a payload type, one mount handler per host it supports, \
           and a small three-crate convention:"),
        list(
            ["The runtime — defines ", code("runtime_scene::Registry<H>"),
             ", ", code("item(payload, children)"),
             " for building a scene node from a payload, and the ",
             code("runtime_vocabulary::caps"),
             " capability traits handlers are generic over. It knows nothing \
              about specific SDKs."],
            ["A shared types crate (e.g. ", code("maps-core"),
             ") — the props type. Pure data, zero platform deps, so the \
              per-backend leaves and the umbrella can all depend on it \
              without a cycle."],
            ["Per-backend leaf crates (", code("maps-web"), ", ",
             code("maps-ios"), ", …) — build the native node from the props. \
              Pure platform code; nothing about the runtime leaks in."],
            ["The umbrella crate (", code("maps"),
             ") — defines the payload, the author-facing constructor, and \
              one ", code("register"),
             " per host, cfg-routed so exactly one is active per build."],
            ["User app — passes ", code("maps::register"),
             " to the boot entry's registration seam. One line."],
        ),
        p("An UNREGISTERED payload panics at realize. That is deliberate: a \
           missed ", code("register"),
          " fails loud instead of silently rendering a placeholder box."),
    },

    section(heading = "Authoring a third-party primitive") {
        p("Concrete example: the in-tree ", code("maps"),
          " SDK — an OpenStreetMap iframe on web, a native ",
          code("MKMapView"),
          " on iOS. The pattern generalizes to camera, AR, video pickers, \
           anything platform-native."),

        p("First the shared types crate. Pure data, zero platform deps:"),
        code(rust, r##"
            // crates/sdk/client/maps/core/src/lib.rs

            #[derive(Clone, Debug)]
            pub struct MapViewProps {
                pub lat: f64,
                pub lon: f64,
                pub zoom: f32,
            }
        "##),

        p("Then the per-backend leaf. It only knows how to build a native \
           node from the props — no runtime types at all:"),
        code(rust, r##"
            // crates/sdk/client/maps/web/src/lib.rs

            use maps_core::MapViewProps;

            pub fn build_map_iframe(props: &MapViewProps) -> web_sys::Element {
                let doc = web_sys::window().unwrap().document().unwrap();
                let iframe = doc.create_element("iframe").unwrap();
                let src = format!(
                    "https://www.openstreetmap.org/export/embed.html?marker={},{}",
                    props.lat, props.lon,
                );
                let _ = iframe.set_attribute("src", &src);
                iframe
            }
        "##),

        p("Now the umbrella. Three pieces: the PAYLOAD (the type the \
           registry keys on), the author-facing BUILDER, and the ",
          code("IntoElement"), " impl that wraps the payload in an ",
          code("item"), " node:"),
        code(rust, r##"
            // crates/sdk/client/maps/src/lib.rs

            use std::cell::RefCell;
            use std::rc::Rc;

            use runtime_scene::{item, Element, MountCx, Registry};
            use runtime_vocabulary::glue::IntoElement;
            use runtime_vocabulary::style_attach::{
                attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
            };

            pub use maps_core::MapViewProps;

            /// The scene payload. Single-take style slot: the scene hands
            /// the handler a shared `&Rc<Self>`, but `StyleProp` has to
            /// MOVE at mount — hence the `RefCell<Option<_>>`.
            struct MapsPrim {
                props: Rc<MapViewProps>,
                style: RefCell<Option<StyleProp>>,
            }

            /// Author-side builder returned by `MapView`.
            pub struct MapViewBound {
                props: Rc<MapViewProps>,
                style: Option<StyleProp>,
            }

            /// PascalCase intentionally — it matches the visual cadence of
            /// first-party primitives inside a `ui!` block.
            #[allow(non_snake_case)]
            pub fn MapView(props: MapViewProps) -> MapViewBound {
                MapViewBound { props: Rc::new(props), style: None }
            }

            impl MapViewBound {
                pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
                    self.style = Some(style.into_style_prop());
                    self
                }
            }

            impl IntoElement for MapViewBound {
                fn into_element(self) -> Element {
                    item(
                        MapsPrim { props: self.props, style: RefCell::new(self.style) },
                        Vec::new(),
                    )
                }
            }
        "##),

        p("Then the handler + registration, one pair per host. The handler \
           receives a ", code("MountCx"),
          " (which carries the backend handle), the payload, and the child \
           elements; it returns the host's node type:"),
        code(rust, r##"
            /// Shared mount tail: author style, then scope-tied teardown.
            fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &MapsPrim)
            where
                H: ExternalOps + StyleServices,
            {
                if let Some(style) = prim.style.borrow_mut().take() {
                    attach_style(backend, node, style);
                }
                let backend = backend.clone();
                let node = node.clone();
                on_teardown(move || backend.borrow_mut().release_external(&node));
            }

            #[cfg(target_arch = "wasm32")]
            pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
                registry.register::<MapsPrim, _>(mount_map_web);
            }

            #[cfg(target_arch = "wasm32")]
            fn mount_map_web(
                cx: &mut MountCx<'_, backend_web::WebBackend>,
                prim: &Rc<MapsPrim>,
                _children: Vec<Element>,
            ) -> web_sys::Node {
                let backend = cx.backend().clone();
                let node: web_sys::Node = maps_web::build_map_iframe(&prim.props).into();
                finish_mount(&backend, &node, prim);
                node
            }
        "##),

        p("The iOS arm is the same three lines with a different leaf and a \
           different node type (", code("MKMapView"),
          " built by ", code("maps_ios::build_map_view"),
          "). Hosts with no leaf get a generic degradation handler — see \
           below."),

        p("And the umbrella's ", code("Cargo.toml"),
          " uses target-specific dependencies so non-web targets don't \
           even pull the web leaf into the dep graph:"),
        code(toml, r##"
            [dependencies]
            runtime-scene = { workspace = true }
            runtime-vocabulary = { workspace = true }
            maps-core = { workspace = true }

            [target.'cfg(target_arch = "wasm32")'.dependencies]
            maps-web = { workspace = true }
            backend-web = { workspace = true }

            [target.'cfg(target_os = "ios")'.dependencies]
            maps-ios = { workspace = true }
            backend-ios-mobile = { workspace = true }
        "##),
    },

    section(heading = "Generic handlers vs per-host handlers") {
        p("A handler generic over the capability traits it needs serves \
           EVERY host at once. That is the right shape whenever the \
           primitive can be expressed through the capability surface — the ",
          code("codeblock"), " and ", code("markdown"),
          " SDKs are both one generic handler for all hosts:"),
        code(rust, r##"
            pub fn register<H>(registry: &mut Registry<H>)
            where
                H: StyleServices + TextOps + 'static,
            {
                registry.register::<CodeBlockPrim, _>(mount_code_block::<H>);
            }
        "##),
        p("Reach for a host-CONCRETE handler (", code("Registry<WebBackend>"),
          ") only when the implementation genuinely needs the platform: \
           building a ", code("web_sys::Element"), " directly, or an ",
          code("MKMapView"),
          ". A concrete handler is a deliberate narrowing, not the default."),
    },

    section(heading = "Using it") {
        p("From the user app's perspective the SDK is one registration and \
           one call site. The boot entry's ", code("register"),
          " argument IS the seam:"),
        code(rust, r##"
            // App bootstrap. Compose several SDKs in one closure if needed.
            backend_web::newcore::start_in("#app", maps::register, app);

            // …or, with more than one SDK:
            backend_web::newcore::start_in(
                "#app",
                |r| { maps::register(r); svg::register(r); },
                app,
            );

            // Inside any component, anywhere in the UI tree:
            use maps::{MapView, MapViewProps};

            ui! {
                view {
                    text { "Find me on a map" }
                    { MapView(MapViewProps {
                        lat: 37.7749,
                        lon: -122.4194,
                        zoom: 12.0,
                    }) }
                }
            }
        "##),
        p("The ", code("{ ... }"),
          " interpolation tells the macro \"this is an expression, not a \
           tag\" — third-party primitives don't plumb into ", code("ui!"),
          " block syntax, because a PascalCase tag routes to ",
          code("BuildElement"),
          " component dispatch. An SDK that wants a real tag ships the tag \
           contract itself: a props struct plus ", code("type WebView = "),
          code("WebViewProps"), ", exactly like the ", code("webview"),
          " SDK does, and then ", code("ui! { WebView(url = …) }"),
          " is ordinary component dispatch."),
        p("The generated per-platform wrappers pass the app's own \
           registration fn — conventionally ",
          code("register_scene_extensions"),
          " — so an app composes its SDK registers there once and every \
           target (web, SSR, iOS, macOS, Android, GPU) picks it up."),
    },

    section(heading = "Refs, handles, and author callbacks") {
        p("An SDK that exposes imperative operations gives its payload a \
           ref slot and binds the handle at mount, the same way a \
           first-party primitive does. ", code("maps"),
          " deliberately doesn't: its props are plain data and its leaves \
           expose no imperative ops, so there is nothing for a ref to \
           carry."),
        p("If your handler DOES run author callbacks from a raw platform \
           event source outside the framework's wrapped dispatch sites — a ",
          code("<form>"), " submit listener, an iframe ",
          code("message"),
          " event, a native toolbar action — call the backend's ",
          code("newcore::schedule_flush()"),
          " after the callback returns. Otherwise the writes that callback \
           staged sit uncommitted until something else triggers a flush."),
        code(rust, r##"
            el.add_event_listener(&closure_that(move |ev| {
                (author_on_submit)(ev);
                backend_web::newcore::schedule_flush();   // commit the staged writes
            }));
        "##),
    },

    section(heading = "What happens on platforms without a leaf") {
        p("An SDK that supports some hosts and not others registers a \
           DEGRADATION handler on the rest, so the payload is always \
           registered and realize never panics:"),
        code(rust, r##"
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
            pub fn register<H>(registry: &mut Registry<H>)
            where
                H: ExternalOps + StyleServices + 'static,
            {
                registry.register::<MapsPrim, _>(mount_placeholder::<H>);
            }
        "##),
        p("The placeholder handler routes through ",
          code("ExternalOps::create_external"),
          ", which renders each host's \"not supported\" box (a bare ",
          code("<div>"),
          " on SSR) with author style and teardown still flowing. User code \
           compiles and runs identically on every target; only the rendered \
           node differs."),
        p("For graceful in-app degradation — \"if maps don't work here, show \
           a static image instead\" — branch in author code on the platform \
           identity rather than probing the registry:"),
        code(rust, r##"
            if matches!(platform(), Platform::Web | Platform::Ios) {
                MapView(MapViewProps { lat, lon, zoom }).into()
            } else {
                image_asset(static_map_png).into()
            }
        "##),
        p("Tree-shaking works automatically — Cargo's target-specific deps \
           keep the iOS leaf out of the web build's dep graph, so the iOS \
           FFI bindings aren't compiled or linked on web. You only pay for \
           the leaves your current target actually uses."),
    },

    section(heading = "Why registration instead of an open enum") {
        p("A natural question: the scene has a small structural ",
          code("Element"),
          " enum. Why do primitives live in a registry rather than as enum \
           variants third-party crates could add?"),
        p("Because registration is what makes the primitive set OPEN while \
           keeping the structural set CLOSED. The five structural variants \
           (item, dyn hole, keyed list, fragment, and the navigator outlet) \
           are what the mount drivers reason about — those must be closed and \
           exhaustively handled. What a given item DOES is a handler lookup, \
           which costs one dispatch and lets a third-party crate participate \
           without touching the runtime."),
        p("The design payoff: a custom host implements the capability traits \
           it can support and inherits the entire third-party ecosystem for \
           free. Every SDK whose handler is capability-generic works on it \
           on day one; the ones that need platform specifics register a \
           degradation handler instead. See ",
          link("writing your own backend", to = "writing-a-backend"), "."),
    },

    section(heading = "When NOT to reach for a new primitive") {
        p("If your primitive is implementable purely in terms of existing \
           framework primitives — views, styles, gestures, animation — \
           write a regular ", link("Component", to = "components"),
          " instead. Components compose, refs work, the ", code("ui!"),
          " macro understands them, no registration needed."),
        p("A registered payload is the right tool only when you genuinely \
           need a native platform widget the framework doesn't ship: system \
           camera, MapKit-style native map, Stripe element, WKWebView with \
           custom message channels, ARKit scene. If you can build it with a \
           styled ", code("view"),
          " and a few reactive props, do that."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Primitives", to = "primitives"),
             " — the first-party set your SDK sits alongside."],
            [link("Backends", to = "backends"),
             " — what each shipped backend supports today."],
            [link("Writing your own backend", to = "writing-a-backend"),
             " — implementing the host contract, and which capability \
              traits a handler can rely on."],
        ),
    },
}
