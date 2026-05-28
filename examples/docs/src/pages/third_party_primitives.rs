//! Third-party primitives page — built via the `docs!` macro.
//!
//! Covers `Primitive::External` as the framework's single extension
//! hatch, the per-backend registry pattern, and the umbrella crate
//! convention third-party SDKs use to ship a primitive across
//! multiple platforms.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "third-party-primitives",
    title = "Third-party primitives",
    category = Advanced,
    description = "Ship a new primitive (with its own native FFI) without forking runtime-core. One escape hatch — Primitive::External — plus a per-backend registry pattern and a small umbrella-crate convention.",
    related = ["primitives", "backends", "writing-a-backend"],
    concepts = [External],

    section(heading = "What this is for") {
        p("The framework ships a closed set of primitives (View, Text, Button, \
           Portal, Image, …) and a closed ", code("Backend"), " trait whose \
           job is to render that set. Closed-on-both-sides is what gives the \
           framework its type-safety guarantee: every backend handles every \
           primitive, checked at compile time."),
        p("But sometimes you want a primitive the framework doesn't ship. A ",
          code("MapView"), " that wraps MapKit on iOS and Google Maps on \
           Android and a Leaflet iframe on web. A camera viewfinder. A \
           Stripe card-element. An AR scene. These are real platform things \
           with no business living in runtime-core, but they need to look \
           and behave like primitives at the call site — they need styles, \
           refs, scope-tied cleanup, the works."),
        p(code("Primitive::External"), " is the one extension hatch the \
           framework provides for this. It lets you ship a primitive in your \
           own crate, register a handler per backend you care about, and \
           call it like any other primitive from user code."),
    },

    section(heading = "The shape, at a glance") {
        p("Everything below is one variant on the ", code("Primitive"),
          " enum, one inherent method on each backend, and a small \
           three-crate convention for SDK authors:"),
        list(
            [code("runtime-core"),
             " — defines ", code("Primitive::External { type_id, type_name, payload, .. }"),
             " and a per-backend ", code("ExternalRegistry<B>"),
             " helper. Knows nothing about specific external kinds."],
            ["Each backend (", code("backend-web"), ", ",
             code("backend-ios-mobile"), ", …) — holds an ",
             code("ExternalRegistry<Self>"), " field and exposes an inherent ",
             code("register_external::<T>(handler)"), " method. Looks the \
              handler up by ", code("TypeId"), " in ", code("create_external"),
             "; falls through to a platform-native \"not supported\" \
              placeholder on a miss."],
            ["Third-party SDK crates (e.g. ", code("maps"),
             ", ", code("maps-web"), ", ", code("maps-core"),
             ") — define their props type, expose a constructor, and ship \
              one per-backend leaf for each platform they support. An \
              umbrella crate cfg-routes the right leaf in per build."],
            ["User app — calls ", code("maps::register(&mut backend)"),
             " once at bootstrap. Done."],
        ),
        p("Closed-enum invariants stay intact for the first-party set; \
           type erasure is paid at exactly one line per backend; user code \
           stays fully typed."),
    },

    section(heading = "Authoring a third-party primitive") {
        p("Concrete example: a ", code("MapView"), " SDK with a web \
           implementation. The pattern generalizes to camera, AR, video \
           pickers, anything platform-native."),

        p("First the shared types crate. Pure data, zero platform deps. \
           Lives in its own crate so per-backend leaves and the umbrella \
           crate can both depend on it without forming a cycle:"),
        code(rust, r##"
            // crates/sdk/maps-core/src/lib.rs

            #[derive(Clone, Debug)]
            pub struct MapViewProps {
                pub lat: f64,
                pub lon: f64,
                pub zoom: f32,
            }
        "##),

        p("Then the per-backend leaf. Imports the shared props + the \
           specific backend type, calls ", code("register_external"),
          " with a handler that builds a native node:"),
        code(rust, r##"
            // crates/sdk/maps-web/src/lib.rs

            use backend_web::WebBackend;
            use maps_core::MapViewProps;

            pub fn register(backend: &mut WebBackend) {
                backend.register_external::<MapViewProps, _>(|props, _backend| {
                    // Build a web_sys::Element however you like.
                    // (Real code would bind to Leaflet via wasm-bindgen;
                    // an OSM iframe is a fine POC.)
                    let doc = web_sys::window().unwrap().document().unwrap();
                    let iframe = doc.create_element("iframe").unwrap();
                    let src = format!(
                        "https://www.openstreetmap.org/export/embed.html\
                         ?marker={},{}",
                        props.lat, props.lon,
                    );
                    let _ = iframe.set_attribute("src", &src);
                    iframe
                });
            }
        "##),

        p("Finally the umbrella crate. This is what user apps import. It \
           re-exports the props, exposes a constructor, and cfg-routes the \
           per-backend ", code("register"), " function to the right leaf at \
           compile time:"),
        code(rust, r##"
            // crates/sdk/maps/src/lib.rs

            use runtime_core::{external, Bound, ExternalHandle};
            pub use maps_core::MapViewProps;

            /// Public constructor. PascalCase so it reads as a primitive
            /// at call sites — `{ MapView(...) }` inside a `ui!` block
            /// has the visual cadence of `Overlay { }` or `View { }`.
            /// Returns a typed `Bound<...>` so `.bind(ref)` is
            /// type-checked against `Ref<ExternalHandle<MapViewProps>>`.
            #[allow(non_snake_case)]
            pub fn MapView(props: MapViewProps) -> Bound<ExternalHandle<MapViewProps>> {
                external(props)
            }

            // Platform-routed `register`. Exactly one of these is active
            // per build, picked by cfg.
            #[cfg(target_arch = "wasm32")]
            pub use maps_web::register;

            #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
            pub use maps_ios::register;

            // Fallback for platforms with no leaf. User code compiles
            // identically on every target; the framework renders its
            // "not supported" placeholder at runtime.
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
            pub fn register<B>(_backend: &mut B) {}
        "##),

        p("And the umbrella's ", code("Cargo.toml"),
          " uses target-specific dependencies so non-web targets don't \
           even pull the web leaf into the dep graph:"),
        code(toml, r##"
            [dependencies]
            runtime-core = { workspace = true }
            maps-core = { workspace = true }

            [target.'cfg(target_arch = "wasm32")'.dependencies]
            maps-web = { workspace = true }

            [target.'cfg(target_os = "ios")'.dependencies]
            maps-ios = { workspace = true }
        "##),
    },

    section(heading = "Using it") {
        p("From the user app's perspective the SDK is one line of \
           bootstrap and one call site for the primitive:"),
        code(rust, r##"
            // App bootstrap (per target, but identical Rust)
            let mut backend = WebBackend::new("#app");
            maps::register(&mut backend);  // routes to maps-web on web,
                                           // no-op on platforms with no leaf

            // Inside any component, anywhere in the UI tree:
            use maps::{MapView, MapViewProps};

            ui! {
                View {
                    Text { "Find me on a map" }
                    { MapView(MapViewProps {
                        lat: 37.7749,
                        lon: -122.4194,
                        zoom: 12.0,
                    }) }
                }
            }
        "##),
        p("The ", code("MapView(...)"), " call returns a ",
          code("Bound<ExternalHandle<MapViewProps>>"),
          ", which slots into the children list the same way ",
          code("View(...)"), " or ", code("Button(...)"),
          " does. ", code(".with_style(...)"), ", ", code(".bind(r)"),
          " and the rest of the standard builder surface all apply."),
        p("The PascalCase name is intentional — it matches the visual \
           cadence of first-party primitives (", code("Overlay { }"),
          ", ", code("View { }"), ") inside a ", code("ui!"),
          " block. The ", code("{ ... }"),
          " interpolation around it tells the macro \"this is an \
           expression, not a tag\" — third-party primitives don't \
           plumb into native ", code("ui!"), " block syntax because \
           the macro only recognizes the first-party primitive set."),
    },

    section(heading = "Refs and handles") {
        p("Refs are typed against the props type, so different SDKs can't \
           accidentally collide on a single ", code("Ref<H>"), " slot:"),
        code(rust, r##"
            use runtime_core::{Ref, ExternalHandle};
            use maps::{MapView, MapViewProps};

            let map_ref: Ref<ExternalHandle<MapViewProps>> = Ref::new();

            MapView(MapViewProps { lat, lon, zoom })
                .bind(map_ref.clone())
        "##),
        p("The ", code("ExternalHandle<T>"), " carries the backend's \
           native node behind an ", code("Rc<dyn Any>"), " — the SDK \
           author exposes typed accessors (under ", code("#[cfg]"),
          ") if they want call sites to reach into the native object:"),
        code(rust, r##"
            // In maps (umbrella):
            impl ExternalHandle<MapViewProps> {
                #[cfg(target_arch = "wasm32")]
                pub fn element(&self) -> Option<&web_sys::Element> {
                    self.node().downcast_ref::<web_sys::Element>()
                }
            }
        "##),
        p("Cross-platform code that doesn't reach into native types just \
           uses the ", code("Ref"), " for lifecycle tracking and skips the \
           accessor entirely."),
    },

    section(heading = "What happens on platforms without a leaf") {
        p("Two stages of \"not supported\" fall out automatically:"),
        list(
            ["Compile-time: the umbrella's fallback ",
             code("register<B>(_: &mut B) {}"), " is generic over any \
              backend, so user code that calls ",
             code("maps::register(&mut backend)"),
             " compiles on every target — even ones the SDK author hasn't \
              shipped a leaf for. The function just does nothing on those \
              targets."],
            ["Runtime: when the user actually mounts ",
             code("MapView(...)"), " on a target with no registered \
              handler, the backend's ", code("create_external"),
             " looks up its registry, finds nothing for ",
             code("TypeId::of::<MapViewProps>()"),
             ", and renders a platform-native \"External MapViewProps not \
              supported\" placeholder instead of panicking."],
        ),
        p("For graceful in-app degradation (\"if maps don't work here, \
           show a static image instead\"), each backend exposes a ",
          code("has_external::<T>()"), " discovery method:"),
        code(rust, r##"
            if backend.has_external::<MapViewProps>() {
                MapView(MapViewProps { lat, lon, zoom }).into()
            } else {
                image_asset(static_map_png).into()
            }
        "##),
        p("Tree-shake works automatically — Cargo's target-specific deps \
           keep the iOS leaf out of the web build's dep graph, so the iOS \
           FFI bindings aren't compiled or linked on web. You only pay for \
           the leaves your current target actually uses."),
    },

    section(heading = "Why the closed enum + escape hatch") {
        p("A natural question: why not just make the ", code("Primitive"),
          " enum open, so third-party crates can add cases directly?"),
        p("Two reasons. The first is a Rust language constraint: closed \
           enums are the only way the framework can prove at compile time \
           that every backend handles every primitive. Open the enum and \
           that guarantee evaporates — backends would have to runtime-check \
           every dispatch, with no way to know what primitives exist until \
           the whole program is linked."),
        p("The second is design discipline: first-party primitives are \
           obligations on every backend, externals are opt-in capabilities. \
           That split is what lets a custom backend (someone implementing ",
          link("their own Backend", to = "writing-a-backend"),
          ") inherit the entire third-party ecosystem for free — they \
           implement the closed first-party trait and either choose to \
           support some externals via their own registry, or leave the \
           default placeholder behavior in place. Either way they're a \
           working backend on day one."),
        p("So: closed enum stays closed for the things the framework \
           guarantees; one ", code("External"), " variant carries the long \
           tail of platform-native primitives nobody wants to centralize. \
           Type-erasure happens at exactly one line per backend, user code \
           stays fully typed, and the third-party crate convention is just \
           standard Rust dep-graph routing — no magic, no plugin loader, \
           no link-time discovery."),
    },

    section(heading = "When NOT to reach for External") {
        p("If your primitive is implementable purely in terms of existing \
           framework primitives — Views, styles, gestures, animation — \
           write a regular ", link("Component", to = "components"),
          " instead. Components compose, refs work, the ", code("ui!"),
          " macro understands them, no extension machinery needed."),
        p(code("External"), " is the right tool only when you genuinely \
           need a native platform widget the framework doesn't ship: \
           system camera, MapKit-style native map, Stripe element, \
           WKWebView with custom message channels, ARKit scene. If you \
           can build it with a styled ", code("View"), " and a few \
           reactive props, do that."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Primitives", to = "primitives"),
             " — the closed first-party set that ", code("External"),
             " complements."],
            [link("Backends", to = "backends"),
             " — what each shipped backend supports today, including which \
              ones have registered the placeholder behavior vs panic on \
              externals."],
            [link("Writing your own backend", to = "writing-a-backend"),
             " — implementing the ", code("Backend"), " trait, including ",
             code("create_external"), " and (optionally) a registry of \
              your own."],
        ),
    },
}
