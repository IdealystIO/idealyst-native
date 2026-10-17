//! Getting Started page — built via the `docs!` macro.
//!
//! Walks the reader from "nothing installed" to a running Idealyst
//! app with hot reload. Mirrors
//! `docs-content-plan/01-getting-started.md`.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "getting-started",
    title = "Getting Started",
    category = Reference,
    description = "From nothing installed to a running Idealyst app that updates live when you edit it.",
    related = ["overview", "primitives", "reactivity", "components", "dev-tools"],
    concepts = [Cli, BuildCache, AppBackendSplit, HotReload],

    section(heading = "What you're about to do") {
        list(
            ["Install the ", code("idealyst"), " CLI."],
            ["Scaffold a new project."],
            ["Run the dev server."],
            ["Edit the code and see it update on screen."],
        ),
        p("The whole thing should take a few minutes if Rust is already on your machine."),
        p("This page gets you from \"nothing installed\" to a running Idealyst app \
           that updates live when you edit it. The same project builds for web, \
           iOS, Android, macOS, and the other shipped platforms — you pick which \
           platform to run."),
    },

    section(heading = "What you need") {
        list(
            ["Rust, stable (1.78 or newer recommended). Install via rustup if \
              you don't have it."],
            ["Tools for the platform(s) you want to run. You only need the ones \
              you'll actually use — the CLI only invokes the toolchain for the \
              targets you select."],
            ["Web — a modern browser and the ", code("wasm32-unknown-unknown"),
             " Rust target (", code("rustup target add wasm32-unknown-unknown"),
             "). Release builds also use ", code("wasm-opt"), " from binaryen."],
            ["iOS — Xcode with the iOS Simulator, plus the iOS Rust targets \
              (", code("aarch64-apple-ios-sim"), " on Apple Silicon)."],
            ["Android — the Android SDK (with an emulator or a connected \
              device) and the Android NDK, with ", code("ANDROID_NDK_HOME"),
             " set."],
            ["Roku — experimental, build-only: ", code("idealyst build --roku"),
             " produces a package to side-load. There is no dev loop for Roku."],
        ),
        p(code("idealyst doctor"), " checks your toolchain for the core, web, \
           iOS, and Android targets and tells you what's missing and how to \
           fix it."),
    },

    section(heading = "Install the CLI") {
        code(bash, r##"
            cargo install idealyst-cli --index sparse+https://crates.idealyst.io/index/ --locked
        "##),

        p("The CLI is published to the framework's registry, ",
          code("crates.idealyst.io"), ". The crate is called ", code("idealyst-cli"),
          " but the binary it installs is ", code("idealyst"), ". Confirm with:"),

        code(bash, r##"
            idealyst --version
        "##),
    },

    section(heading = "Create a project") {
        code(bash, r##"
            idealyst new my-app
            cd my-app
        "##),

        p("You'll get a layout like this:"),

        code(text, r##"
            my-app/
              Cargo.toml          # crate manifest + Idealyst config under [package.metadata.idealyst]
              index.html          # the web page shell
              dev.toml            # dev-session settings
              .mcp.json           # points MCP clients at `idealyst mcp`
              fonts/
              src/
                main.rs           # one line: `idealyst::entry!(my_app);` — the entry point for every platform
                lib.rs            # exports `pub fn app() -> Element`
                app.rs            # the welcome screen's tree
                components/ …     # the welcome screen's components
        "##),
        p(code("src/main.rs"), " is required: ", code("idealyst::entry!"),
          " reads your app config and emits the ", code("main"),
          " each platform's shell needs. Nothing in it names a platform."),

        p("There is no ", code("ios/"), " folder and no ", code("android/"),
          ". The iOS and Android wrapper crates that turn your ", code("app()"),
          " into a native library are generated on demand by the CLI into a \
           build cache (", code("target/idealyst/<project>/"), "). You don't \
           author them, and you don't see them unless you go looking. The web \
           build compiles your crate directly."),

        p("The generated ", code("Cargo.toml"), " starts with web as the only \
           target, because it needs no per-platform toolchain:"),

        code(text, r##"
            [package.metadata.idealyst.app]
            name      = "My App"
            bundle_id = "com.example.my_app"
            version   = "0.0.1"
            targets   = ["web"]
        "##),

        p(code("name"), " is the display name, title-cased from the crate name. ",
          code("bundle_id"), " uses underscores, not hyphens, because Android \
           derives JNI symbol names from it; pass ", code("--bundle-id"),
          " to ", code("idealyst new"), " to choose your own."),

        p(code("targets"), " is the list of platforms ", code("idealyst dev"),
          " and ", code("idealyst build"), " use when you don't pass an explicit ",
          code("--web"), " / ", code("--ios"), " / ", code("--android"),
          " flag. Add entries (", code("targets = [\"web\", \"ios\", \"android\"]"),
          ") when you're ready for more platforms."),
    },

    section(heading = "What's in src/lib.rs") {
        p("The scaffold's ", code("app()"), " is an animated welcome screen \
           built from a handful of components in ", code("src/components/"),
          " — something concrete to read and modify."),

        p("Every Idealyst app exposes one ", code("pub fn app() -> Element"),
          ". Keep it thin and put the tree in a ", code("#[component]"),
          " — on the web, hot patching reaches only code called through a \
           component. The minimal shape looks like this:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Element};

            #[component]
            fn Root() -> Element {
                let count = signal(0);

                ui! {
                    view {
                        text { "Hello, Idealyst" }
                        text { "Count: {count}" }
                        button(
                            label = "Increment",
                            on_click = move || count.update(|n| n + 1),
                        )
                    }
                }
            }

            pub fn app() -> Element {
                ui! { Root() }
            }
        "##),

        p("The welcome scaffold is larger, but the structure is the same — \
           components, some state, a UI tree."),

        p("A few things to notice, without going deep on any of them yet:"),

        list(
            [code("#[component]"), " makes ", code("Root"), " a component, \
              so ", code("app()"), " can render it as ", code("Root()"), " inside ",
             code("ui!"), "."],
            [code("signal(0)"), " declares reactive state. The ", code("{count}"),
             " slot in the text is a live read of the signal; the ", code("count.update(...)"),
             " inside the button's ", code("on_click"), " is a write. The framework \
              keeps that ", code("text"), " in sync when the button is \
              pressed — see Reactivity on the Overview page for the mechanism."],
            [code("ui! { ... }"), " is the DSL for declaring the UI tree. It lowers \
              to plain Rust function calls; you don't have to import the primitive \
              constructors (", code("view"), ", ", code("text"), ", ", code("button"),
             ") explicitly because the macro emits absolute paths."],
        ),
    },

    section(heading = "Building UI: components, not just primitives") {
        p("The example above uses runtime-core primitives (", code("view"), ", ",
          code("text"), ", ", code("button"), ") directly. For most projects you'll \
           want higher-level pieces too — headings, cards, themed buttons, layout \
           stacks."),

        p("You have four options:"),

        list(
            ["idea-ui — the first-party component library. The docs site is built \
              with it. See idea-ui for what's included and how to use it."],
            ["Build your own with the framework's theme and stylesheet system on \
              top of the primitive vocabulary. See Styles for how that system works."],
            ["Use a third-party library built on runtime-core."],
            ["Skip it. Primitives alone are a complete option."],
        ),

        p("The framework underneath is the same in every case."),
    },

    section(heading = "Run it") {
        code(bash, r##"
            idealyst dev
        "##),

        p("With no flags, the dev server builds every platform listed in ",
          code("targets"), " and runs each one. If you want only one platform — \
           common during day-to-day work — name it explicitly:"),

        code(bash, r##"
            idealyst dev --web        # web only
            idealyst dev --ios        # iOS simulator only
            idealyst dev --android    # Android emulator/device only
        "##),

        p("You can combine flags (", code("idealyst dev --web --ios"),
          ") to run multiple platforms in parallel; each gets its own watch + \
           rebuild loop and Ctrl-C tears them all down together."),

        p("By default ", code("idealyst dev"), " runs your app in one sidecar \
           process on your machine, and each platform connects to it as a thin \
           client. ", code("--local"), " builds and runs the app on each \
           platform instead. See ", link("Dev tools", to = "dev-tools"), "."),

        p("Hot reload is on by default. A literal edit inside ", code("ui!"),
          " applies in milliseconds; an edit to function bodies is hot-patched \
           in seconds and signal values are carried over. A change to a \
           signature, a struct, or a new item rebuilds and restarts the app, \
           and signal values are not carried over."),
    },

    section(heading = "Edit something") {
        p("With ", code("idealyst dev --web"), " running and the browser open:"),

        list(
            ["Open ", code("src/lib.rs"), "."],
            ["Change ", code("\"Hello, Idealyst\""), " to anything else."],
            ["Save."],
        ),

        p("The app should update without a page reload, and the counter value \
           should still be whatever you'd clicked it to. If a full reload happens \
           instead, that's a shape change the patcher can't apply — a changed \
           signature, a changed struct, or a new item. Hot reload covers most \
           edits."),
    },

    section(heading = "Building for production") {
        p("When you're ready to ship, run a release build:"),

        code(bash, r##"
            idealyst build --release
        "##),

        p("Same flag shape as ", code("dev"), " — with no platform flag, it builds \
           every target in your ", code("Cargo.toml"), ". Add ", code("--web"),
          " / ", code("--ios"), " / ", code("--android"), " / ", code("--roku"),
          " to narrow."),

        p("Web output lands in ", code("dist/web/"), " (the ", code("pkg/"),
          " bundle, ", code("index.html"), ", and assets); Roku's side-loadable \
           package lands in ", code("dist/roku/"), ". The iOS build produces \
           the static library and the Android build produces the ", code(".so"),
          ", from the generated wrapper under ", code("target/idealyst/<project>/"),
          "; ", code("idealyst run ios"), " and ", code("idealyst run android"),
          " package, install, and launch them."),
    },

    section(heading = "Exporting a platform project (coming soon)") {
        p(code("idealyst scaffold <platform>"), " will copy the generated host \
           crate for a platform out of the build cache and into your repo as an \
           ordinary directory, so you can hand-edit it. After exporting, the CLI \
           builds from your exported source instead of regenerating, and your \
           changes stick."),

        p("Not yet implemented. Use the default in-cache flow for now."),
    },

    section(heading = "What's next") {
        p("If the counter is on screen, you have a working setup. Next up:"),

        list(
            ["Components — define your own and reuse them."],
            ["Reactivity — signals, effects, the rules for what gets re-run."],
            ["Styles — stylesheets, themes, variants."],
            ["Primitives — the full list of built-in nodes (", code("view"), ", ",
             code("text"), ", ", code("button"), ", ", code("pressable"), ", ",
             code("scroll_view"), ", ", code("icon"), ", and more)."],
            ["Navigation — stack and swap navigators, screens, routes."],
        ),

        p("The Overview page is the right read if you skipped it — it covers the \
           model the rest of the documentation assumes."),
    },
}
