//! Backends page — built via the `docs!` macro.
//!
//! Map of the backends Idealyst ships: web, iOS, Android, Roku, and
//! the runtime-server dev-mode shell client.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "backends",
    title = "Backends",
    category = Reference,
    description = "The piece of code that puts an Idealyst app on a specific platform's screen.",
    related = ["writing-a-backend", "dev-tools", "overview", "primitives"],
    concepts = [Backend, RuntimeBackend, GeneratorBackend, RuntimeServer],

    section(heading = "Intro") {
        p("A backend is the piece of code that knows how to put an Idealyst \
           app on a specific platform's screen. The framework hands it a tree \
           of ", code("Primitive"), "s and a stream of updates; the backend \
           translates those into native widgets, layout, and input events."),
        p("The ", code("Backend"), " trait is the seam. Anything that \
           implements it can run an app. This page is the map of the backends \
           Idealyst ships — what each one targets, what makes it interesting, \
           what it can and can't do. For the trait's full surface and what \
           writing one looks like, see ",
          link("Writing your own backend", to = "writing-a-backend"), "."),
    },

    section(heading = "The shipped backends") {
        p("Five backend families come in the box:"),
        list(
            [code("backend-web"), " — Browser (WASM + DOM); production-ready"],
            [code("backend-ios-*"), " family (", code("-core"), ", ",
             code("-mobile"), ", ", code("-tv"),
             ") — iOS / iPadOS / tvOS (UIKit); production-ready"],
            [code("backend-android-*"), " family (", code("-core"), ", ",
             code("-mobile"), ", ", code("-tv"),
             ") — Android phones / tablets / TV (Views); production-ready"],
            [code("backend-roku"), " — Roku devices (SceneGraph + BrightScript); experimental"],
            [code("runtime-server-shell-native"), " — Dev-mode app-as-server client; dev only"],
        ),
        p("Each one lives in ", code("crates/backend/<name>"), " and gets \
           pulled in by the CLI when you target the matching platform."),
    },

    section(heading = "Web") {
        p(code("backend-web"), " drives the DOM via ", code("web-sys"), " and ",
          code("wasm-bindgen"), ". Your app compiles to WebAssembly, the \
           backend creates DOM elements as the walker visits primitives, and \
           the browser handles layout, input, and rendering."),
        p("A few things worth knowing:"),
        list(
            ["Layout is the browser's. No Taffy here — the backend just sets \
              CSS properties and lets the browser do the work. Flex layout \
              maps cleanly because the framework's flex model is a subset of \
              CSS flexbox."],
            ["Stylesheets become CSS classes. Each ",
             code("(stylesheet, variants)"), " combination mints one class \
              lazily; ", code("apply_style"), " just sets ", code("className"),
             ". Theme swaps are CSS variable writes — see ",
             link("Styles", to = "styles"), " for the full mechanism."],
            ["The dev pipeline uses ", code("wasm-pack"), ". ",
             code("idealyst dev --web"), " watches your source, rebuilds the \
              WASM module, and serves it from a local HTTP server. Live \
              updates apply via hot reload — not full page reloads."],
        ),
    },

    section(heading = "iOS") {
        p(code("backend-ios"), " renders to UIKit via the ", code("objc2"),
          " crate. The backend is split across three sub-crates:"),
        list(
            [code("backend-ios-core"), " — the shared substrate. Style \
              application, color/flex conversion, and the ", code("NSTimer"),
             "-based render loop driver."],
            [code("backend-ios-mobile"), " — touch input semantics for iPhone \
              and iPad. Gesture recognizers, focus on first responder, \
              hardware-keyboard handling."],
            [code("backend-ios-tv"), " — Apple TV / tvOS focus-engine \
              semantics. D-pad navigation, focus visualizers, the \
              focus-on-press model."],
        ),
        p("The split exists because the input model genuinely differs: \
           touch-based UIs and focus-engine-based UIs need different \
           gesture-recognizer plumbing and different selection chrome. They \
           share everything below input — the same flex layout, the same \
           style application, the same primitive vocabulary."),
        p("A few specifics:"),
        list(
            ["Layout is Taffy. The backend hands flex constraints to Taffy \
              and applies the resulting frames to ", code("UIView"),
             " instances. Same engine the Dioxus ecosystem uses; one of the \
              few external dependencies Idealyst pulls in."],
            ["Safe-area insets propagate reactively. The native safe-area \
              changes (rotation, dynamic island, software keyboard) drive a \
              framework signal; primitives that opt into safe-area padding \
              re-apply automatically."],
            ["The build pipeline uses ", code("xcodebuild"), ". ",
             code("idealyst dev --ios"), " generates a small Xcode wrapper \
              project in a build cache, invokes ", code("cargo build"),
             " for the static library, then ", code("xcodebuild"),
             " to launch on a simulator."],
        ),
    },

    section(heading = "Android") {
        p(code("backend-android"), " mirrors the iOS split: a shared core, \
           plus a mobile leaf and a TV leaf."),
        list(
            [code("backend-android-core"), " — JNI helpers, the render-thread ",
             code("RenderLoopDriver"), ", and the shared View hierarchy \
              primitives."],
            [code("backend-android-mobile"), " — touch input for phones and \
              tablets."],
            [code("backend-android-tv"), " — Leanback / D-pad focus model for \
              Android TV."],
        ),
        p("Notes:"),
        list(
            ["Layout is Taffy (same as iOS). The Android ", code("View"),
             " system has its own layout machinery, but the framework drives \
              layout itself and pushes computed frames into the View \
              hierarchy."],
            ["The build pipeline uses Gradle. ", code("idealyst dev --android"),
             " generates a Gradle wrapper around your Rust ", code("cdylib"),
             ", then builds and installs to an emulator or attached device."],
        ),
    },

    section(heading = "Roku") {
        p(code("backend-roku"), " is the experimental one. Roku devices don't \
           run Rust — the only language the device runtime supports is \
           BrightScript, plus the SceneGraph XML markup format."),
        p("The backend works by emitting a wire stream of commands that a \
           small BrightScript thin client running on the device replays \
           against SceneGraph nodes. Every primitive create / update / event \
           goes over the network."),
        p("Implications:"),
        list(
            ["Performance is bounded by the network round-trip. This backend \
              isn't viable for shipping consumer apps. It's primarily a \
              demonstration that the framework's seams can target a platform \
              with no native Rust runtime."],
            ["Closures don't ship. Reactive expressions have to round-trip \
              through the host. The ", code("Derived<T>"), " and ",
             code("Action"), " types (mentioned in ",
             link("Reactivity", to = "reactivity"), ") carry a structured \
              form specifically for this case — generator backends can \
              serialize the reactive intent without serializing a closure."],
            ["Some primitives have no Roku analog. GPU ", code("Graphics"),
             " are not supported; certain layout features are approximated."],
        ),
        p("Read ", code("backend-roku"), "'s source comments for the exact \
           constraints if you're curious. For now: cool to see, not what \
           you'd build a production TV app on."),
    },

    section(heading = "runtime-server — the dev-mode backend") {
        p(code("runtime-server-shell-native"), " is the app-as-server client. \
           It's unusual because it doesn't render anything itself — it \
           forwards backend operations to whatever real backend is on the \
           other end."),
        p("The shape:"),
        code(text, r##"
            ┌────────────────────┐  WebSocket  ┌────────────────────┐
            │  runtime-server dev-host      │ ──────────► │  runtime-server shell client  │
            │  (your app, on     │             │  (browser / phone, │
            │   the dev machine) │ ◄────────── │   thin client)     │
            └────────────────────┘             └────────────────────┘
               Runs the reactive                  Receives wire
               runtime; emits wire                commands and
               commands to clients                renders them
        "##),
        p("Why have this? Because it gives you one running instance of your \
           app's reactive runtime, with arbitrary platforms connecting as \
           thin clients. Edit code on the dev machine, every connected client \
           updates. Navigate on one client, the navigation state syncs to \
           the others."),
        p("runtime-server is its own concept worth a page — see ",
          link("Dev tools", to = "dev-tools"), " for the full story, \
           including how the wire protocol works and what \"app-as-server\" \
           actually buys you in day-to-day development."),
    },

    section(heading = "Picking a backend") {
        p("You pick a backend by picking a platform target. Inside ",
          code("Cargo.toml"), ":"),
        code(toml, r##"
            [package.metadata.idealyst.app]
            targets = ["web", "ios", "android", "roku"]
        "##),
        p("…and the CLI selects the matching backend per target when you \
           build. There's no code change to switch."),
        p("For one-off runs:"),
        code(bash, r##"
            idealyst dev --web       # only the web backend
            idealyst dev --ios       # only the iOS backend
            idealyst dev --aas       # runtime-server dev-host with whatever clients connect
        "##),
    },

    section(heading = "Writing your own backend") {
        p("The shipped backends cover the major platforms, but there's no \
           reason to stop there. The ", code("Backend"), " trait is small \
           (~30 methods), and a working backend lives in one Rust crate that \
           depends only on ", code("runtime-core"), " and whatever native \
           bindings it needs."),
        p("Things people could plug in:"),
        list(
            ["A terminal renderer for command-line apps"],
            ["A custom GPU renderer with ", code("wgpu"),
             " (Idealyst already exposes the ", code("Graphics"),
             " primitive — a backend can take it further and render the \
              whole tree on GPU)"],
            ["An embedded display driver (e-paper, OLED)"],
            ["A server-side renderer that emits HTML for SSR"],
            ["A backend for a platform the framework doesn't ship yet \
              (macOS, Windows, Linux desktop via winit, KaiOS, anything)"],
        ),
        p("The dedicated ",
          link("Writing your own backend", to = "writing-a-backend"),
          " page walks through the trait's full surface, the lifecycle of a \
           backend node, the relationship between the walker and ",
          code("Backend"), " methods, and what gets called when. This page \
           is intentionally just the map."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Writing your own backend", to = "writing-a-backend"),
             " — the full trait, lifecycle, and a worked example."],
            [link("Dev tools", to = "dev-tools"),
             " — runtime-server in depth, the wire protocol, hot reload."],
            [link("Architecture in more depth", to = "overview"),
             " (on the Overview) — where backends sit relative to ",
             code("framework-wire"), ", ", code("dev-hot"), ", and ",
             code("framework-runtime-layout"), "."],
        ),
    },
}
