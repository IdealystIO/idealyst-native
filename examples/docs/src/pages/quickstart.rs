//! Getting Started page — built via the `docs!` macro.
//!
//! Walks the reader from "nothing installed" to a running Idealyst
//! app with hot reload. Migrated mechanically from
//! `docs-content-plan/01-getting-started.md`.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

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
           that updates live when you edit it. The same project will build for \
           web, iOS, Android, and Roku out of the box — you pick which platform \
           to run."),
    },

    section(heading = "What you need") {
        list(
            ["Rust (1.70 or newer). Install via rustup if you don't have it."],
            ["Tools for the platform(s) you want to run. You only need the ones \
              you'll actually use — the scaffold builds for all of them, but the \
              dev server only invokes the toolchain for the targets you select."],
            ["Web — a modern browser. The CLI runs ", code("wasm-pack"), " for you."],
            ["iOS — Xcode and the iOS Simulator. No manual setup beyond the \
              standard Xcode install."],
            ["Android — Android Studio (for the SDK and an emulator) plus the \
              Android NDK."],
            ["Roku — a Roku device in developer mode, or the Roku simulator. \
              Experimental; expect rough edges."],
        ),
        p("A future ", code("idealyst doctor"), " command will check your toolchain \
           automatically and report what's missing. Until that lands, the \
           platform builders will tell you what they couldn't find if something \
           isn't installed."),
    },

    section(heading = "Install the CLI") {
        code(bash, r##"
            cargo install idealyst-cli
        "##),

        p("The crate is called ", code("idealyst-cli"), " but the binary it installs is ",
          code("idealyst"), ". Confirm with:"),

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
              src/
                lib.rs            # exports `pub fn app() -> Primitive` — your whole app
        "##),

        p("That's it. There is no ", code("ios/"), " folder, no ", code("android/"),
          ", no ", code("web/"), ". The per-platform host crates that turn your ",
          code("app()"), " into a runnable binary or bundle are generated on demand \
           by the CLI into a build cache. You don't author them, and you don't see \
           them unless you go looking."),

        p("The generated ", code("Cargo.toml"), " enables every supported platform \
           as a build target by default:"),

        code(text, r##"
            [package.metadata.idealyst.app]
            name = "my-app"
            bundle_id = "com.example.my-app"
            targets = ["web", "ios", "android", "roku"]
        "##),

        p(code("targets"), " is the list of platforms the CLI will build for when \
           you don't pass an explicit ", code("--web"), " / ", code("--ios"), " / ",
          code("--android"), " / ", code("--roku"), " flag. You can prune this list \
           later (drop platforms you don't target), or add a new entry when more \
           platforms ship."),
    },

    section(heading = "What's in src/lib.rs") {
        p("The scaffold's ", code("app()"), " function is a small showcase that \
           exercises most of the framework's primitives — text, a counter with a \
           button, a toggle, an icon, a scrollable container — so you can see \
           what's on offer and have something concrete to modify."),

        p("Every Idealyst app starts the same way: one function annotated with ",
          code("#[component]"), " that returns a ", code("Primitive"),
          " tree. The minimal shape looks like this:"),

        code(rust, r##"
            use runtime_core::{component, signal, ui, Primitive};

            #[component]
            pub fn app() -> Primitive {
                let count = signal!(0);

                ui! {
                    View {
                        Text { "Hello, Idealyst" }
                        Text { format!("Count: {}", count.get()) }
                        Button(
                            label = "Increment",
                            on_click = move || count.update(|n| *n += 1),
                        )
                    }
                }
            }
        "##),

        p("The scaffold expands on this with more primitives, but the structure \
           is the same — a component, some state, a UI tree."),

        p("A few things to notice, without going deep on any of them yet:"),

        list(
            [code("#[component]"), " marks ", code("app()"), " as a component. \
              Every Idealyst app starts from one."],
            [code("signal!(0)"), " declares reactive state. The ", code("count.get()"),
             " inside ", code("format!"), " is a tracked read; the ", code("count.update(...)"),
             " inside the button's ", code("on_click"), " is a write. The framework \
              keeps the surrounding ", code("Text"), " in sync when the button is \
              pressed — see Reactivity on the Overview page for the mechanism."],
            [code("ui! { ... }"), " is the DSL for declaring the UI tree. It lowers \
              to plain Rust function calls; you don't have to import the primitive \
              constructors (", code("view"), ", ", code("text"), ", ", code("button"),
             ") explicitly because the macro emits absolute paths."],
        ),
    },

    section(heading = "Building UI: components, not just primitives") {
        p("The scaffold uses runtime-core primitives (", code("View"), ", ",
          code("Text"), ", ", code("Button"), ") directly. For most projects you'll \
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

        p("Hot reload is on by default. The dev server watches your source files, \
           rebuilds incrementally on save, and patches the running app in place — \
           your scroll position, navigation state, and current signal values are \
           preserved across edits."),
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
           instead, that's a structural change the patcher couldn't apply — \
           usually a new top-level component, a re-shaped signal, or a change to \
           the component graph. Hot reload covers most edits; full reloads happen \
           rarely and only when they have to."),
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

        p("Output lands in ", code("target/idealyst/<platform>/"),
          ". Each platform's directory holds whatever the platform expects: a \
           WASM bundle and an ", code("index.html"), " for web, an Xcode ",
          code(".app"), " for iOS, an APK for Android, a side-loadable channel \
           package for Roku."),
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
            ["Primitives — the full list of built-in nodes (", code("View"), ", ",
             code("Text"), ", ", code("Button"), ", ", code("Pressable"), ", ",
             code("ScrollView"), ", ", code("Icon"), ", and more)."],
            ["Navigation — drawer and tab navigators, screens, routes."],
        ),

        p("The Overview page is the right read if you skipped it — it covers the \
           model the rest of the documentation assumes."),
    },
}
