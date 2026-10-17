//! Backends page — built via the `docs!` macro.
//!
//! Map of the backends Idealyst ships, plus the runtime-server
//! dev-mode shell client.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

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
           of ", code("Element"), "s and a stream of updates; the backend \
           translates those into native widgets, layout, and input events."),
        p("The backend contract — ", code("runtime_scene::Host"),
          " plus the capability traits — is the seam. Anything that \
           implements it can run an app. This page is the map of the backends \
           Idealyst ships — what each one targets, what makes it interesting, \
           what it can and can't do. For the full surface and what \
           writing one looks like, see ",
          link("Writing your own backend", to = "writing-a-backend"), "."),
    },

    section(heading = "The shipped backends") {
        p("These backends come in the box:"),
        list(
            [code("backend-web"), " — Browser (WASM + DOM)"],
            [code("backend-ios-mobile"), " (on ", code("backend-ios-core"),
             ") — iOS / iPadOS (UIKit)"],
            [code("backend-android-mobile"), " (on ", code("backend-android-core"),
             ") — Android phones / tablets (Views)"],
            [code("backend-macos"), " — macOS (AppKit)"],
            [code("backend-linux"), " — Linux (GTK4)"],
            [code("backend-windows"), " — Windows (Win32)"],
            [code("backend-roku"), " — Roku devices (SceneGraph + BrightScript); experimental"],
            [code("backend-ssr"), " — Server-side rendering and static generation (HTML)"],
            [code("backend-email"), " — Email-safe HTML"],
            [code("backend-terminal"), " — A terminal character grid"],
            [code("backend-cpu"), " — A pixel framebuffer (software rasterizer)"],
            [code("render-wgpu"), " — A GPU renderer that paints its own pixels"],
        ),
        p("Each one lives under ", code("crates/backend/"), " (the GPU renderer \
           under ", code("crates/gpu-backend/"), ") and gets pulled in by the \
           CLI when you target the matching platform. ", code("backend-ios-tv"),
          " and ", code("backend-android-tv"), " exist as empty skeletons; \
           tvOS and Android TV are not supported."),
        p("Dev mode adds one more piece: ", code("runtime-server-shell-native"),
          ", the thin client that replays a dev session on a device. See \
           runtime-server below."),
    },

    section(heading = "Web") {
        p(code("backend-web"), " drives the DOM through ", code("web-glue"),
          ", the framework's own JS bindings. Your app compiles to WebAssembly, the \
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
            ["The CLI builds the app crate for ", code("wasm32-unknown-unknown"),
             " with ", code("cargo build"), " and packages the module itself; \
              the ", code("wasm-bindgen"), " CLI runs only for apps that link \
              wgpu or their own ", code("web-sys"), ". ",
             code("idealyst dev --web"), " watches your source and serves the \
              app from a local HTTP server. Most saves apply in place, \
              without a page reload."],
        ),
    },

    section(heading = "iOS") {
        p("The iOS backend renders to UIKit via the ", code("objc2"),
          " crate. It is split across three sub-crates:"),
        list(
            [code("backend-ios-core"), " — the shared substrate. Style \
              application, color/flex conversion, and the ", code("NSTimer"),
             "-based render loop driver."],
            [code("backend-ios-mobile"), " — the iPhone / iPad backend: raw \
              touch delivery (gestures are recognized in shared Rust), focus \
              on first responder, hardware-keyboard handling."],
            [code("backend-ios-tv"), " — reserved for Apple TV / tvOS \
              focus-engine input; an empty skeleton."],
        ),
        p("The split exists because touch input and focus-engine input need \
           different plumbing, while everything below input — layout, style \
           application, the primitive vocabulary — is shared."),
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
            ["No Xcode project for the simulator. The CLI generates a small \
              wrapper crate in a build cache, runs ", code("cargo build"),
             " for the static library, links it with ", code("swiftc"),
             ", assembles the ", code(".app"), ", and launches it with ",
             code("simctl"), ". Device builds (", code("--device"),
             ") generate an Xcode project so ", code("xcodebuild"),
             " can sign them."],
        ),
    },

    section(heading = "Android") {
        p("The Android backend mirrors the iOS split: a shared core, plus a \
           mobile leaf and a TV leaf."),
        list(
            [code("backend-android-core"), " — JNI helpers, the render-thread ",
             code("RenderLoopDriver"), ", and the shared View hierarchy \
              primitives."],
            [code("backend-android-mobile"), " — touch input for phones and \
              tablets."],
            [code("backend-android-tv"), " — reserved for the Leanback / D-pad \
              focus model on Android TV; an empty skeleton."],
        ),
        p("Notes:"),
        list(
            ["Layout is Taffy (same as iOS). The Android ", code("View"),
             " system has its own layout machinery, but the framework drives \
              layout itself and pushes computed frames into the View \
              hierarchy."],
            ["No Gradle. The CLI generates a wrapper crate that builds your \
              app as a ", code("cdylib"), " (", code(".so"), "), then runs the \
              Android SDK tools directly (", code("javac"), ", ", code("d8"),
             ", ", code("aapt2"), ", ", code("apksigner"),
             ") and installs the APK with ", code("adb"), "."],
        ),
    },

    section(heading = "Roku") {
        p(code("backend-roku"), " is the experimental one. Roku devices don't \
           run Rust — the only language the device runtime supports is \
           BrightScript, plus the SceneGraph XML markup format."),
        p("The backend is a command emitter: every capability call becomes a \
           JSON ", code("RokuCommand"), " in a queue. ",
          code("idealyst build --roku"), " runs your ", code("app()"),
          " once in a generated wrapper binary, snapshots the command stream, \
           and lays out a side-loadable package in ", code("dist/roku/"),
          " (plus ", code("dist/roku.zip"), "): the snapshot, a small \
           BrightScript runtime that replays it against SceneGraph nodes, and \
           your ", code("#[method]"), " functions transpiled to BrightScript."),
        p("Implications:"),
        list(
            ["Button events are not yet sent back from the device, and there \
              is no ", code("idealyst dev"), " loop for Roku. This backend \
              isn't viable for shipping consumer apps. It's primarily a \
              demonstration that the framework's seams can target a platform \
              with no native Rust runtime."],
            ["Closures don't ship. Reactive expressions have to round-trip \
              through the host. The ", code("Derived<T>"), " and ",
             code("Action"), " types (mentioned in ",
             link("Reactivity", to = "reactivity"), ") carry a structured \
              form specifically for this case — generator backends can \
              serialize the reactive intent without serializing a closure."],
            ["Some primitives have no Roku analog. GPU ", code("graphics"),
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
            idealyst dev --web           # only the web backend
            idealyst dev --ios           # only the iOS backend
            idealyst dev --web --local   # build and run the app in the browser itself
        "##),
        p(code("idealyst dev"), " runs in runtime-server mode by default: one \
           sidecar process runs your app and each platform connects as a thin \
           client. ", code("--local"), " builds and runs the app on each \
           platform instead."),
    },

    section(heading = "Writing your own backend") {
        p("The shipped backends cover the major platforms, but there's no \
           reason to stop there. ", code("Host"), " is a handful of tree \
           operations, every capability-trait method but a few has a working \
           default, and a working backend lives in one Rust crate that depends \
           on ", code("runtime-scene"), ", ", code("runtime-vocabulary"), ", ",
          code("runtime-shared"), ", and whatever native bindings it needs."),
        p("Things people could plug in:"),
        list(
            ["An embedded display driver (e-paper, OLED)"],
            ["A game-engine or custom-GPU renderer"],
            ["A backend for a platform the framework doesn't ship yet (KaiOS, \
              a console, anything)"],
        ),
        p("The dedicated ",
          link("Writing your own backend", to = "writing-a-backend"),
          " page walks through both layers, the lifecycle of a backend node, \
           and the relationship between the mount drivers and the capability \
           traits. This page is intentionally just the map."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Writing your own backend", to = "writing-a-backend"),
             " — the full trait, lifecycle, and a worked example."],
            [link("Dev tools", to = "dev-tools"),
             " — runtime-server in depth, the wire protocol, hot reload."],
            [link("Architecture in more depth", to = "overview"),
             " (on the Overview) — where backends sit relative to ",
             code("wire"), ", ", code("dev-hot"), ", and ",
             code("runtime-layout"), "."],
        ),
    },
}
