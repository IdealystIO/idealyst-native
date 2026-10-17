//! Dev tools page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "dev-tools",
    title = "Dev tools",
    category = Tools,
    description = "How the CLI builds your app, how hot reload patches it, what runtime-server is, and how MCP exposes Robot to external tools.",
    related = ["backends", "robot", "getting-started", "writing-a-backend"],
    concepts = [Cli, HotReload, RuntimeServer, WireProtocol, McpServer, BuildCache],

    section(heading = "Overview") {
        p("This page covers the moving parts behind day-to-day development: \
           how the CLI turns your one-crate app into running platform builds, \
           how hot reload patches a running app without losing its state, what \
           app-as-server (runtime-server) is and when to use it, and how the MCP server \
           hooks Robot up to an external tool like Claude Desktop."),
        p("Most of what's here lives below the surface of the user-facing \
           commands. You can ship apps without reading any of it. But if you \
           hit a weird build issue, want to script the dev loop, or want to \
           extend the tooling, this is the layer to know about."),
    },

    section(heading = "How an app gets built") {
        p("The headline rule: you author one crate. Everything else is generated."),
        p("Your repo looks like this:"),
        code(text, r##"
            my-app/
              Cargo.toml          # crate manifest + Idealyst metadata
              src/
                main.rs           # `idealyst::entry!(my_app);` — the entry point for every platform
                lib.rs            # exports `pub fn app() -> Element`
        "##),
        p("There's no ", code("ios/"), " and no ", code("android/"),
          ". The web build compiles your crate itself, through ",
          code("src/main.rs"), ". The iOS, Android, and runtime-server builds \
           use wrapper crates the CLI generates on demand into a build cache:"),
        code(text, r##"
            target/
              idealyst/
                <project>/
                  web/                   # staging for the wasm bundle (shipped from dist/web)
                  ios/wrapper/           # staticlib wrapper
                  android/wrapper/       # cdylib wrapper
                  runtime-server/app/    # the dev sidecar that runs your app
                  runtime-server/host/   # the dev host
        "##),
        p("Each wrapper depends on your app crate as a library, supplies the \
           platform entry point (a C-callable ", code("ios_main"),
          " on iOS, the JNI entry on Android), and hands ", code("app()"),
          " to the matching backend."),
        p("You don't author these files. You don't even see them unless you \
           go looking. The CLI regenerates them as needed; their contents \
           change with the framework, not with your app."),
    },

    section(heading = "Why this shape") {
        p("The build cache approach trades visible boilerplate for one moving \
           part: a project's \"platform support\" is a list of strings in ",
          code("Cargo.toml"), ", not a folder structure you have to maintain."),
        code(toml, r##"
            [package.metadata.idealyst.app]
            name = "My App"
            bundle_id = "com.example.my_app"
            targets = ["web", "ios", "android"]
        "##),
        p("Adding iOS to a project that didn't have it is editing this list \
           and running ", code("idealyst dev --ios"), ". Removing it is \
           deleting the string. No host crate to add or remove, no ",
          code("Cargo.toml"), " to merge, no platform-specific scaffolding to \
           keep up to date as the framework evolves."),
    },

    section(heading = "Per-platform tools") {
        p("Each backend has its own native build flow. The CLI invokes them:"),
        list(
            ["Web — ", code("cargo build --target wasm32-unknown-unknown"), " on your crate, the CLI's own glue pass (the ", code("wasm-bindgen"), " CLI runs only for apps that link wgpu or ", code("web-sys"), "), ", code("wasm-split"), ", and ", code("wasm-opt"), " for release builds. Dev serves the result from a local HTTP server; ", code("idealyst build --web"), " writes it to ", code("dist/web/"), "."],
            ["iOS — ", code("cargo build"), " for the static lib, ", code("swiftc"), " to link the app, then ", code("simctl"), " to install and launch on a simulator. Device builds generate an Xcode project so ", code("xcodebuild"), " can sign them."],
            ["Android — ", code("cargo build"), " for the ", code("cdylib"), ", then the SDK tools directly (", code("javac"), ", ", code("d8"), ", ", code("aapt2"), ", ", code("apksigner"), ") and ", code("adb install"), " to an emulator or device. No Gradle."],
            ["Roku — ", code("idealyst build --roku"), " lays out a side-loadable package in ", code("dist/roku/"), " from a snapshot of your app's commands plus a BrightScript runtime. There is no dev loop: ", code("idealyst dev"), " refuses a Roku target."],
            ["runtime-server — ", code("cargo build"), " for the sidecar wrapper (your app plus the recording backend), then run it."],
        ),
        p("You can run any of these by hand if you need to debug the \
           pipeline. The CLI is orchestration; the underlying tools are \
           unchanged."),
    },

    section(heading = "Exporting a generated project") {
        p(code("idealyst scaffold <platform>"), " is reserved for materializing \
           a platform's generated wrapper out of the build cache and into your \
           repo, so you could hand-edit it. It is not implemented: running it \
           prints a \"not yet implemented\" notice."),
    },

    section(heading = "Hot reload") {
        p("When you save a file with ", code("idealyst dev"), " running, the \
           CLI sorts the save into one of three tiers before doing any work:"),
        list(
            ["Overlay patch — only literals inside ", code("ui!"), " bodies changed. The running app edits its own mounted tree. No compiler; milliseconds."],
            ["Hot patch — only function bodies changed. The edited crate is rebuilt into a patch, a jump table rebinds the patched functions, and the mounted tree re-runs in place with signal values carried over. Seconds."],
            ["Rebuild — anything that changes a file's shape (a signature, a struct, a new item). In runtime-server mode the sidecar rebuilds and restarts, and clients keep their connections and receive a fresh snapshot. With ", code("--local"), " on the web the bundle rebuilds and the page reloads."],
        ),
        p("The hot-patch tier rests on two pieces: a jump-table function \
           dispatcher, and carrying state across a re-run of the tree."),
    },

    section(heading = "The function dispatcher") {
        p("With hot reload on, ", code("#[component]"), " splits every \
           component function into two parts at compile time:"),
        list(
            ["The outer function keeps the public name and signature (", code("Counter"), "). Its body is ", code("dev_hot::call(__Counter_hot_impl, (props,))"), " — a dispatch through a runtime jump table."],
            ["The inner function (", code("__Counter_hot_impl"), ") holds the real body."],
        ),
        p("When a patch carries a new ", code("__Counter_hot_impl"), ", the \
           jump table's entry is updated to point at it. The next time the \
           component runs, it runs the new code. The outer function — and \
           everything that calls it — never has to be updated."),
        p(code("dev-hot"), " is a thin layer over the subsecond crate from the \
           Dioxus project. With the feature off, ", code("dev_hot::call"),
          " is a direct call and the split costs nothing."),
    },

    section(heading = "What survives a patch") {
        p("A patch rebinds functions; it does not re-render. So after a patch \
           the framework re-runs the mounted tree: it moves each signal's value \
           out of the old tree, drops the tree, and builds it again, handing \
           each ", code("signal()"), " call its predecessor's value. Backend \
           nodes are rebuilt along with the tree."),
        p("What this means in practice:"),
        list(
            ["Signals created while the tree mounts survive. They are matched by position: the component path, the order of ", code("signal()"), " calls in the body, and the value's type."],
            ["A body that adds or reorders a ", code("signal()"), " call gets fresh state from that call on, rather than another variable's value."],
            ["Signals created later — in an event handler, or in an ", code("if"), " / keyed ", code("for"), " / navigator screen that mounts later — start fresh."],
            ["On the web, patches reach only code called through a ", code("#[component]"), ". Keep ", code("app()"), " thin and render a root component from it."],
        ),
    },

    section(heading = "What forces a full reload") {
        p("Some changes can't be patched. The framework falls back to a full \
           rebuild + remount when it detects one. The common cases:"),
        list(
            ["Top-level signature changes — adding a parameter to ", code("app()"), ", for example. The dispatcher can't reconcile the call site."],
            ["Adding the first ", code("#[method]"), " to a component. It generates a ", code("<Name>Handle"), " and a ", code("bind_to"), " prop, so the component's props change shape."],
            ["Renaming or removing a component. No identity to map."],
        ),
        p("For everyday editing — tweaking strings, layout, styles, signal \
           values, even adding new primitives inside a component — hot reload \
           covers it. A full reload is the framework saying \"I tried and the \
           change is structural enough that I'm starting over.\""),
    },

    section(heading = "runtime-server — app-as-server") {
        p("This is the part that's worth understanding even if you never use \
           it for anything fancy: it changes what \"running the app\" means."),
    },

    section(heading = "The default — runtime-server mode") {
        p(code("idealyst dev"), " (with ", code("--web"), ", ", code("--ios"),
          ", ", code("--android"), ", or any mix) builds one sidecar binary \
           that runs your app's reactive runtime, and starts a WebSocket \
           server. The CLI points every client it launches at that server — \
           native clients get the address baked into their build — so there \
           is no network discovery and no URL to type."),
        p("Then clients connect:"),
        code(text, r##"
            ┌─────────────────────────────┐
            │  sidecar (your machine)     │
            │   - app()'s reactive state  │
            │   - signals, effects, scopes│
            │   - the primitive tree      │
            └──────────────┬──────────────┘
                           │  WebSocket: wire commands
                           │  (NodeId, StyleId, HandlerId)
               ┌───────────┼───────────┐
               ▼           ▼           ▼
             browser     iOS sim    Android dev
             (thin       (thin       device
              client)     client)    (thin client)
        "##),
        p("Each client is a thin client backend. It receives wire commands \
           from the sidecar and replays them against its real backend (DOM \
           on web, UIKit on iOS, Views on Android). It doesn't run any of \
           your app code — it doesn't need to. The sidecar is the single \
           source of truth for what the app looks like."),
        p("When you click a button on the iOS client, the click event is \
           forwarded back to the sidecar over the wire. The sidecar's button \
           handler fires. Signals update. Effects re-run. The new wire \
           commands ship to every connected client. All of them update \
           simultaneously."),
        p("When you save, the clients never rebuild. A literal or body-only \
           edit is applied inside the running sidecar, so signal values on \
           its side survive. A shape change rebuilds and restarts the \
           sidecar; clients keep their connections and receive a fresh \
           snapshot."),
    },

    section(heading = "Local-render mode") {
        p(code("idealyst dev --local"), " (with ", code("--web"), ", ",
          code("--ios"), ", …) builds the app for each platform and runs it \
           there, with its own watch loop. On the web the page runs the app as \
           wasm and saves land in the same three tiers; native platforms \
           rebuild and relaunch on every save."),
        p("Use it to run the real wasm build, when the framework workspace \
           isn't reachable for the sidecar build, or to bypass the wire \
           protocol."),
    },

    section(heading = "What runtime-server is good for") {
        list(
            ["Cross-platform comparison. Run web, iOS, and Android clients side by side, navigate on one, watch the others follow. Useful for spotting platform-specific rendering differences in real time."],
            ["State that survives source edits. The app's state lives in the sidecar, so overlay and hot patches keep it (see What survives a patch for the limits). Useful when you're 6 screens deep into testing and don't want to re-navigate after every change."],
            ["Driving demos. One canonical instance of the app, multiple windows or devices showing it."],
            ["Debugging the wire protocol or backends. Because the wire is the seam, you can log it, replay it, modify it in flight."],
        ),
    },

    section(heading = "What runtime-server isn't good for") {
        list(
            ["GPU work. ", code("graphics"), " primitives don't ship over the wire. They emit a placeholder in runtime-server mode. If you're building anything with a ", code("wgpu"), " canvas, run with ", code("--local"), "."],
            ["Performance testing. Every interaction pays a WebSocket round-trip. runtime-server is for development, not perf measurement."],
            ["Production. runtime-server is a development tool. Shipping an app-as-server architecture has security implications that the current implementation doesn't address."],
        ),
    },

    section(heading = "A few specifics") {
        list(
            ["Fixed endpoint, no discovery. The CLI bakes the sidecar's address into native client builds (", code("IDEALYST_DEV_ENDPOINT=ws://host:port"), "), and the client connects straight to it. There is no Bonjour / mDNS browsing."],
            ["Connection-time snapshot. When a fresh client connects mid-session, the sidecar sends it a ", code("SceneModel"), " snapshot — the current state of the tree — rather than replaying the full command log. The client comes in synced."],
            ["Reverse channel for callbacks. Click handlers, text changes, and other input events go from client to sidecar on the same WebSocket. The sidecar resolves them to in-process closures and fires them."],
        ),
        p("The Architecture section of the Overview points at the supporting \
           crates: ", code("wire"), " for the protocol, ",
          code("dev-client"), " for the app-side replayer, ",
          code("runtime-server-shell-native"), " for the native client side."),
    },

    section(heading = "The MCP server — Claude Desktop and Robot") {
        p("Every dev build carries Robot: your app listens on TCP port ",
          code("9718"), " for clients that speak the Robot bridge protocol. \
           Anything that can connect and issue JSON over a TCP socket can \
           drive the app: find elements, tap buttons, type into inputs, call \
           the functions the app exposes for tests, invoke ", code("#[method]"),
          " methods, read frames."),
        p(code("idealyst mcp"), " is the server that bridges this bridge to \
           MCP. MCP — the Model Context Protocol — is the protocol Claude Code, \
           Claude Desktop, and similar tools use to expose tools to a model. \
           It speaks MCP on stdio and the Robot bridge protocol over TCP, \
           translating between them (and serves the component catalog too)."),
        p("The flow:"),
        code(text, r##"
            ┌───────────────┐         stdio          ┌───────────────────┐
            │ Claude Code / │ ───────── MCP ───────► │ idealyst mcp      │
            │ Desktop       │ ◄────── (JSON-RPC) ─── │ (translates)      │
            └───────────────┘                        └──────────┬────────┘
                                                                │
                                                                │ TCP :9718
                                                                │ (Robot bridge)
                                                                ▼
                                                     ┌───────────────────┐
                                                     │  Your running app │
                                                     │  (a dev build)    │
                                                     └───────────────────┘
        "##),
        p("To wire it up, point the MCP client at the ", code("idealyst"),
          " binary (", code("idealyst mcp install"), " writes this into \
           ", code(".mcp.json"), " for you). For Claude Desktop, that's a few \
           lines in its config:"),
        code(json, r##"
            {
                "mcpServers": {
                    "idealyst": {
                        "command": "idealyst",
                        "args": ["mcp"]
                    }
                }
            }
        "##),
        p("Now the model gets every Robot tool — ", code("find_element"),
          ", ", code("click"), ", ", code("tap_native"), ", ", code("type_text"),
          ", ", code("call_function"), ", ", code("get_snapshot"), ", ",
          code("invoke_method"), ", and the rest — as callable functions. You can ask it to interact with your \
           running app in natural language; it issues the tool calls; the \
           proxy translates them; the app responds."),
        p("The full list of tools and what they do lives on the Robot page."),
    },

    section(heading = "Running a test suite on every platform") {
        p(code("idealyst test"), " runs a project's ", code("#[robot_test]"),
          " suite (", code("tests/robot.rs"), ", with ", code("robot-test"),
          " as a dev-dependency) against the app on one platform:"),
        code(bash, r##"
            idealyst test --web      # or --macos, --ios, --android, --linux, --windows
        "##),
        p("It builds the app fresh and launches it, waits for that project's \
           app to register, and runs ", code("cargo test --test robot"),
          " against it. The same suite runs unchanged on every platform. The \
           app is marked as test-driven, so an in-app self-test that checks ",
          code("robot::is_test_driven()"), " stands down. ", code("--filter"),
          " narrows the run and ", code("--attach"), " drives an app you \
           already started with ", code("idealyst dev"), "."),
        p("A suite's ", code("click()"), " taps with real platform input where \
           the backend can synthesize it, so a test also catches an overlay or \
           a disabled native widget swallowing taps. The Robot page covers the \
           API."),
    },

    section(heading = "What this is good for") {
        p("The expected use cases:"),
        list(
            ["Interactive UI testing with an LLM in the loop. Describe a scenario; the model exercises it; you watch what happens."],
            ["Reproducing bugs. Hand the model the symptoms; it pokes the UI until it finds the path to the broken state."],
            ["Authoring tests. Have the model walk a happy path, log the tool calls it made, and convert them into a deterministic test script."],
        ),
        p("You can also write your own MCP client — the protocol is \
           documented and the proxy is small. The MCP integration is one \
           example of what the Robot bridge enables, not the only one."),
    },

    section(heading = "A simple mental model") {
        p("To recap how the pieces fit:"),
        list(
            ["The CLI turns your one-crate project into platform builds. Per-platform wrappers live in a build cache, generated on demand."],
            ["Hot reload patches a running app without restarting it. Component bodies update through a function-pointer dispatcher; the mounted tree then re-runs with signal values carried over."],
            ["runtime-server is a dev mode where the framework runs once on your machine and thin clients connect over WebSocket. State lives in one place; every client sees it."],
            ["Robot is the introspection layer that lets external processes drive a running app."],
            ["The MCP proxy is the bridge from MCP-speaking tools (Claude Desktop, etc.) to Robot."],
        ),
        p("All four are independent and optional. You can build apps using \
           none of them — ", code("cargo build"), ", manual wrapper crates, \
           no hot reload, no Robot. The tooling exists because building this \
           way is faster and more pleasant, not because the framework demands \
           it."),
    },

    section(heading = "Where to read more") {
        list(
            ["Robot — the tools the bridge exposes, in detail."],
            ["Backends — what the runtime-server thin clients are doing on each platform."],
            ["Architecture in more depth (on the Overview) — where ", code("dev-hot"), ", ", code("wire"), ", and ", code("dev-client"), " sit."],
            ["Hot reload internals — the patch pipeline, how state is carried, what survives a patch in detail."],
            ["The wire protocol — the ", code("Command"), " enum and id namespaces, for anyone building their own wire consumer."],
        ),
    },
}
