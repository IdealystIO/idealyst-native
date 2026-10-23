//! Get set up — install the CLI, scaffold a project, run it, set up the
//! editor and the MCP server, and tour the files.
//!
//! The command-line lessons (0.1–0.5) put a [`Terminal`] beside the prose:
//! the lesson's real commands as numbered steps, played into a terminal
//! with output modeled on what the real tools print. The step data and the
//! `idealyst doctor` model live at the bottom of this file. Shell and TOML
//! listings in the prose are plain `CodePanel(shell = true)`s (they are not
//! Rust); the two app.rs listings are compiled starters from
//! `crate::fiddles::getting_started`.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage, workspace};
use crate::routes::{
    START_ANATOMY_ROUTE, START_CREATE_ROUTE, START_EDITOR_ROUTE, START_INSTALL_ROUTE, START_MCP_ROUTE,
    START_RUN_ROUTE,
};
use crate::terminal::{Os, TermStep, Targets, Terminal};

pub fn install() -> Element {
    ui! {
        LessonPage(
            current = START_INSTALL_ROUTE.name(),
            title = "Install the CLI".to_string(),
            lead = "The idealyst CLI handles everything from new projects to store builds. It's \
                built from source, so all you need first is Rust.".to_string(),
            learn = vec![
                "Install Rust with rustup, then the CLI with cargo install",
                "Check your machine with idealyst doctor",
                "Which platform tools you need, and when",
            ],
            workspace = workspace(|| ui! { Terminal(steps = INSTALL_STEPS, show_targets = true) }),
        ) {
            Typography(content = "Before you start".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Everything you do with Idealyst goes through the idealyst command: \
                    creating a project, running it with hot reload, building it for a store, and \
                    diagnosing your toolchain. It is a Rust program, installed with cargo, so the \
                    one thing you need first is a stable Rust toolchain from rustup (rustup.rs), \
                    with the WebAssembly target added \u{2014} that's what the web build compiles \
                    to. Steps 1 to 3 on the right do both and check the result.".to_string()
            )

            Typography(content = "Install the CLI".to_string(), kind = typography_kind::H2)
            Typography(
                content = "There are no prebuilt binaries yet. The CLI is published to the \
                    framework's own registry; --index names it directly, so no cargo configuration \
                    is needed, and --locked builds exactly the dependency graph the release was \
                    tested with. cargo compiles it in release mode and puts idealyst in \
                    ~/.cargo/bin. The first build takes a few minutes.".to_string()
            )
            CodePanel(
                src = "cargo install idealyst-cli \\\n  --index sparse+https://crates.idealyst.io/index/ \\\n  --locked".to_string(),
                shell = true,
            )
            Callout(label = "Pin or upgrade".to_string()) {
                Typography(
                    content = "Add --version <x.y.z> to install a particular release. Later, \
                        idealyst update replaces the binary with the newest release, and idealyst \
                        update --check only reports whether there is one.".to_string(),
                    muted = true,
                )
            }

            Typography(content = "Platform tools, only when you need them".to_string(), kind = typography_kind::H2)
            Typography(
                content = "doctor reports what each platform still needs. A platform's tooling is \
                    only required once you build for it: Xcode for iOS and macOS (which means a \
                    Mac), the Android SDK and NDK for Android. Pick your targets under the \
                    terminal and run the last step to see what doctor checks for them. A \u{2713} \
                    is installed, a \u{2717} is a required tool that's missing, and a \u{25b5} is \
                    optional or only needed for some builds.".to_string()
            )
            Typography(
                content = "Bare idealyst doctor reports every platform but only fails on the core \
                    tools; idealyst doctor ios checks one platform and fails if it isn't ready. \
                    This course runs on the web, so you can skip the rest for now.".to_string()
            )

            Callout(label = "Working from a framework checkout".to_string()) {
                Typography(
                    content = "To run an unreleased commit, install from your clone with cargo \
                        install --path crates/tools/cli --force. A project created inside the \
                        checkout builds against the checkout's framework source, not the \
                        registry.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Installing, updating, and running unreleased commits.".to_string(),
                link_label = "README \u{2014} Getting started".to_string(),
                doc_file = "/README.md#getting-started".to_string(),
            )
        }
    }
}

pub fn create() -> Element {
    ui! {
        LessonPage(
            current = START_CREATE_ROUTE.name(),
            title = "Your first project".to_string(),
            lead = "One command scaffolds an app; one function is where its UI begins.".to_string(),
            learn = vec![
                "Scaffold an app with idealyst new",
                "Where your UI begins: app() in src/app.rs",
                "Run it in the browser with idealyst dev",
            ],
            workspace = workspace(|| ui! { Terminal(steps = CREATE_STEPS) }),
        ) {
            Typography(
                content = "Steps 1 and 2 on the right create a project called my-app and step into \
                    it.".to_string()
            )
            Typography(
                content = "The name must be a valid Cargo package name: lowercase, with hyphens or \
                    underscores. Pass --bundle-id com.example.my_app to set the reverse-DNS \
                    identifier the iOS and Android builds use (underscores, not hyphens); it \
                    defaults to com.example.<name>.".to_string()
            )
            Typography(
                content = "What you get is an ordinary Rust crate. There is no Xcode or Gradle \
                    project to maintain: the CLI generates each platform's wrapper at build time, \
                    from your crate. Lesson 0.6 walks through every file; for now, one matters.".to_string()
            )

            Typography(content = "src/app.rs".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Your UI begins in a function named app that returns an Element. The \
                    scaffold's version plays an animated welcome; the smallest possible one is \
                    this:".to_string()
            )
            CodePanel(src = include_str!("../fiddles/getting_started/hello.rs").to_string())
            Typography(
                content = "Step 4 runs the scaffold's app: idealyst dev builds it for the web and \
                    serves it on localhost:8080, where you'll see \"Welcome to Idealyst\" rise out \
                    of a sunrise.".to_string()
            )

            Callout(label = "How the live examples work".to_string()) {
                Typography(
                    content = "From lesson 1.1 on, the workspace on the right is an editor holding a \
                        whole app.rs. Run sends it to a compile server, which builds it with \
                        idealyst build --web \u{2014} the same command you would ship with \u{2014} \
                        and the preview loads the result. Compiler errors are underlined in the code \
                        and printed beneath it.".to_string(),
                    muted = true,
                )
            }
        }
    }
}

pub fn run() -> Element {
    ui! {
        LessonPage(
            current = START_RUN_ROUTE.name(),
            title = "Run it".to_string(),
            lead = "idealyst dev builds the app, opens it, and updates it every time you save.".to_string(),
            learn = vec![
                "What idealyst dev does on every save",
                "The three update tiers: literal patch, hot patch, rebuild",
                "Running on a device, and building for release",
            ],
            workspace = workspace(|| ui! { Terminal(steps = RUN_STEPS) }),
        ) {
            CodePanel(
                src = r##"idealyst dev            # Cargo.toml's targets (web, for a new project)
idealyst dev --web      # http://localhost:8080
idealyst dev --ios      # iOS simulator
idealyst dev --android  # Android emulator or device
idealyst dev --macos    # a native macOS window"##.to_string(),
                shell = true,
            )
            Typography(
                content = "The first build compiles the framework and takes a minute or two; after \
                    that, builds are incremental. Open the address it prints and you'll see the \
                    welcome screen: \"Welcome to Idealyst\" rising out of a sunrise.".to_string()
            )

            Typography(content = "Save, and it updates".to_string(), kind = typography_kind::H2)
            Typography(
                content = "With dev running, edit src/app.rs and save. The CLI picks the cheapest \
                    update that is still correct: a change to a literal (a string, a number) patches \
                    the running app in milliseconds; a change inside a function body hot-patches \
                    the compiled code and keeps your app's state; anything that changes a type or a \
                    signature rebuilds and restarts the app.".to_string()
            )
            Typography(
                content = "The steps on the right play one of each against this app.rs: dev \
                    starting up, then a string edit, a change inside the function, and a change to \
                    a signature, with the line dev prints for each. The patch tiers shown are the \
                    in-page ones, which --local enables on the web.".to_string()
            )
            CodePanel(src = include_str!("../fiddles/getting_started/edit.rs").to_string())

            Callout(label = "Where the app runs during dev".to_string()) {
                Typography(
                    content = "By default dev runs your app() in a host process and streams what it \
                        renders to the browser or device, so one session can drive several screens \
                        at once. Add --local to compile the app into the target itself instead \
                        \u{2014} the mode to use when testing a platform capability like the \
                        camera, which then uses the device's hardware rather than your \
                        computer's.".to_string(),
                    muted = true,
                )
            }

            Typography(content = "Run on a device".to_string(), kind = typography_kind::H2)
            Typography(
                content = "run builds and launches without the dev loop; build produces the \
                    artifact you ship. Both take the platform explicitly.".to_string()
            )
            CodePanel(
                src = r##"idealyst run ios                # build + launch in the simulator
idealyst run android            # build + install on an emulator or device
idealyst build --web --release  # a deployable bundle in dist/web"##.to_string(),
                shell = true,
            )

            DocsLink(
                summary = "The three save tiers, what state survives a patch, and watching a session from a tool.".to_string(),
                link_label = "Hot reload".to_string(),
                doc_file = "hot-reload.md".to_string(),
            )
        }
    }
}

pub fn editor() -> Element {
    ui! {
        LessonPage(
            current = START_EDITOR_ROUTE.name(),
            title = "Editor setup: VS Code".to_string(),
            lead = "rust-analyzer for the language, the Idealyst extension for the framework's \
                vocabulary, and one command that wires them together.".to_string(),
            learn = vec![
                "Set up rust-analyzer for Idealyst code",
                "Add the Idealyst extension's completions and docs",
                "Wire the framework's lint into your editor",
            ],
            workspace = workspace(|| ui! { Terminal(steps = EDITOR_STEPS) }),
        ) {
            Typography(
                content = "Any editor that runs rust-analyzer works with Idealyst. This lesson \
                    sets up VS Code, because that is where the framework ships extra help: \
                    completion and docs for the words inside ui! and stylesheet!, and the \
                    framework's own lint shown inline next to compiler errors.".to_string()
            )

            Typography(content = "1. rust-analyzer".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Install the rust-analyzer extension (rust-lang.rust-analyzer). It \
                    understands your app the way the compiler does \u{2014} types, go to \
                    definition, errors as you save \u{2014} including inside ui! blocks, which the \
                    macro expands in a form rust-analyzer can follow.".to_string()
            )

            Typography(content = "2. The Idealyst extension".to_string(), kind = typography_kind::H2)
            Typography(
                content = "rust-analyzer knows Rust, not the framework's vocabulary: inside a \
                    stylesheet! it offers no completions at all. The Idealyst extension fills that \
                    gap from your project's catalog \u{2014} the same data the MCP server serves \
                    (lesson 0.5):".to_string()
            )
            Typography(
                content = "In ui!: every primitive and component as you type a tag, each \
                    component's props inside its parentheses, and the values a prop accepts after \
                    name = (tones, variants, icons, enum variants), adding the use line a value \
                    needs. Hovering a tag, prop or value shows its docs. In stylesheet!: token \
                    completion off the block's binding \u{2014} t. offers color, spacing, radius \
                    \u{2026} and t.spacing. lists the sizes with their values. In a function body: \
                    snippets for signal, memo, effect!, spawn_then and the rest, with their \
                    docs.".to_string()
            )
            Typography(
                content = "The extension is plain JavaScript in the framework repository, not yet \
                    on the Marketplace. Link it into VS Code's extensions folder from a checkout, \
                    then run Developer: Reload Window.".to_string()
            )
            Typography(
                content = "Steps 2 and 3 on the right clone the repository and link the extension; \
                    on Windows, creating the link needs Developer Mode or an administrator \
                    shell.".to_string()
            )
            Typography(
                content = "The first time you open a project it builds the catalog of your \
                    dependencies, which takes a few minutes cold and seconds after that; your own \
                    components are re-read on every save. Its progress and any failure go to \
                    Output \u{25b8} Idealyst. After a cargo update, run Idealyst: Refresh Catalog \
                    from the command palette.".to_string()
            )

            Typography(content = "3. Wire them together".to_string(), kind = typography_kind::H2)
            Typography(
                content = "From your project, configure vscode writes the workspace settings. It \
                    recommends both extensions in .vscode/extensions.json, and points \
                    rust-analyzer's check at a small script, .vscode/ra-check.sh, that runs cargo \
                    check and idealyst lint together \u{2014} so a framework idiom gone wrong is \
                    underlined like a compiler warning. It only adds its own keys; the rest of \
                    your settings are left alone.".to_string()
            )
            CodePanel(
                src = r##"idealyst configure vscode                    # pick what to apply
idealyst configure vscode --non-interactive  # apply everything
idealyst configure vscode --remove           # take it all back out"##.to_string(),
                shell = true,
            )

            Callout(label = "If components show no types".to_string()) {
                Typography(
                    content = "When cargo check is clean but rust-analyzer knows nothing about your \
                        components' props, its proc-macro build is failing \u{2014} most often \
                        because a rust-analyzer.cargo.features entry in your user settings, left \
                        from another project, names a feature this crate lacks. Add \
                        \"rust-analyzer.cargo.features\": [] to the project's \
                        .vscode/settings.json, then run rust-analyzer: Restart server.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "What each configure aspect writes, and how the extension finds a project's catalog.".to_string(),
                link_label = "VS Code setup".to_string(),
                doc_file = "vscode.md".to_string(),
            )
        }
    }
}

pub fn mcp() -> Element {
    ui! {
        LessonPage(
            current = START_MCP_ROUTE.name(),
            title = "AI-assisted development with MCP".to_string(),
            lead = "Your project comes with an MCP server: a coding agent reads the framework's \
                real API and drives your running app.".to_string(),
            learn = vec![
                "What the project's MCP server gives a coding agent",
                "Reading the framework through the catalog tools",
                "Letting an agent drive your running app",
            ],
            workspace = workspace(|| ui! { Terminal(steps = MCP_STEPS) }),
        ) {
            Typography(
                content = "MCP (the Model Context Protocol) is how coding agents such as Claude \
                    Code call tools. idealyst mcp is such a server, and it answers from your \
                    project rather than from whatever an agent remembers: the components, props, \
                    tokens and guides of the framework version you actually depend on, plus your \
                    own components.".to_string()
            )

            Typography(content = "It's already connected".to_string(), kind = typography_kind::H2)
            Typography(
                content = "idealyst new writes a .mcp.json at the project root, which Claude Code \
                    loads automatically when you open the project. Any other MCP client takes the \
                    same command. To add it to a project that doesn't have it, run idealyst mcp \
                    install there.".to_string()
            )
            Typography(
                content = "Steps 1 and 2 on the right show the file and the command; step 3 starts \
                    the app, which is what the app-driving tools below connect to.".to_string()
            )

            Typography(content = "Reading the framework".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The catalog tools describe what your code can use. list_primitives and \
                    describe_primitive cover the built-in elements; list_components and \
                    describe_component cover every #[component] you link, idea-ui's and your own, \
                    with each prop's type and docs; list_tokens and list_values give a \
                    stylesheet's token names and a prop's accepted values; read_guide serves the \
                    framework's guides; search looks across all of it. The catalog is rebuilt as \
                    you save, so a component you just wrote is described the moment it compiles.".to_string()
            )

            Typography(content = "Driving the app".to_string(), kind = typography_kind::H2)
            Typography(
                content = "A second set of tools works on the app while it runs. run_dev starts \
                    idealyst dev and wait_for_app waits for it to come up; then find_element, \
                    click and type_text operate the UI, get_snapshot returns the element tree, \
                    screenshot captures it, read_signal and write_signal inspect and set state, \
                    and get_logs returns its output. They work on every platform the app runs on, \
                    web or simulator, through the same bridge. A typical round trip:".to_string()
            )
            CodePanel(
                src = r##"you:   Add a Clear button that empties the todo list.

agent: describe_component Button    the props, and on_click's type
       read_guide components         how this project writes components
       (edits src/app.rs)
       run_dev, wait_for_app         the app is up on the web
       type_text, click "Add"        two todos added
       click "Clear"
       get_snapshot                  no rows left: done"##.to_string(),
                shell = true,
            )

            Callout(label = "The catalog is what you link".to_string()) {
                Typography(
                    content = "An SDK you haven't added to Cargo.toml contributes nothing, so an \
                        agent can't look it up before it's a dependency. Add the crate first. Pass \
                        --no-robot to run the catalog without the app-driving tools.".to_string(),
                    muted = true,
                )
            }
            Typography(
                content = "How the catalog is built from your code, and why it can't drift from \
                    it, is lesson 11.3, Catalog, docs & MCP.".to_string()
            )

            DocsLink(
                summary = "The catalog crates, the server's tool families, and how the catalog stays current.".to_string(),
                link_label = "MCP crates".to_string(),
                doc_file = "/crates/mcp/README.md".to_string(),
            )
        }
    }
}

pub fn anatomy() -> Element {
    ui! {
        LessonPage(
            current = START_ANATOMY_ROUTE.name(),
            title = "Anatomy of a project".to_string(),
            lead = "Every file idealyst new writes, what it's for, and when you'll edit it.".to_string(),
            learn = vec![
                "Every file idealyst new writes, and what it's for",
                "The app's settings in [package.metadata.idealyst.app]",
                "How to install an SDK in src/lib.rs",
            ],
        ) {
            CodePanel(
                src = r##"my-app/
├── Cargo.toml          the crate + the app's settings
├── dev.toml            idealyst dev settings
├── index.html          the page the web build mounts into
├── fonts/              Inter, used by the welcome screen
├── .cargo/config.toml  where framework crates come from
├── .mcp.json           the MCP server for coding agents
└── src/
    ├── main.rs         the platform entry point
    ├── lib.rs          your modules + SDK registration
    ├── app.rs          pub fn app() -> Element
    └── components/     the welcome screen's pieces"##.to_string(),
                shell = true,
            )

            Typography(content = "Cargo.toml".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Four framework dependencies: idealyst (the platform shell, used only by \
                    main.rs), runtime-core (everything you write with: ui!, signal, stylesheet!), \
                    and runtime-vocabulary and runtime-scene, which the macros and the \
                    registration function name. The sidecar and dev-session features belong to \
                    idealyst dev; leave them be. The app's own settings live in one table the CLI \
                    reads:".to_string()
            )
            CodePanel(
                src = r##"[package.metadata.idealyst.app]
name      = "My App"              # the display name
bundle_id = "com.example.my_app"  # iOS / Android identifier
version   = "0.0.1"
targets   = ["web"]               # what dev and build use by default

[package.metadata.idealyst.app.icon]
source = "assets/icon.png"        # app icons + favicons, generated

[package.metadata.idealyst.app.splash]
background  = "#1a1a2e"           # shown until the app mounts
duration_ms = 0                   # 0 turns the splash off"##.to_string(),
                shell = true,
            )
            Typography(
                content = "targets takes web, ios, android, macos, linux, windows, terminal and \
                    roku. A new project lists only web, which needs no platform tooling; add the \
                    others when you're ready, or name one on the command line \
                    (idealyst dev --ios). build_number sets the store build number.".to_string()
            )

            Typography(content = "src/main.rs".to_string(), kind = typography_kind::H2)
            Typography(
                content = "One line. entry! expands to the right entry point for whatever target \
                    you build \u{2014} a wasm start function, an iOS app delegate, an Android \
                    activity \u{2014} so the rest of your crate never contains a \
                    #[cfg(target_os)] block. You won't need to touch it.".to_string()
            )
            CodePanel(src = "idealyst::entry!(my_app);".to_string())

            Typography(content = "src/lib.rs".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Declares your modules and re-exports app. It also holds \
                    register_scene_extensions, called once at startup on every platform, where \
                    you install SDKs that render their own elements \u{2014} a code block, a web \
                    view, a map. The scaffold installs none. Adding one is a dependency and a \
                    line:".to_string()
            )
            CodePanel(src = r##"# Cargo.toml
codeblock = { version = "5.0", registry = "idealyst" }

// src/lib.rs
pub fn register_scene_extensions<H>(
    registry: &mut runtime_scene::Registry<H>,
)
where
    H: runtime_scene::Host
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    codeblock::register(registry);
}"##.to_string())
            Typography(
                content = "The where clause grows with what the SDKs need from the platform; each \
                    SDK's README names it. Forgetting the register line isn't silent: the first \
                    time one of its elements renders, the app stops with a panic. Two smaller \
                    functions sit beside it: register_scene_extensions_recorder, the same \
                    registration for idealyst dev's default mode, and scene_app, the Android \
                    entry.".to_string()
            )

            Typography(content = "src/app.rs and components/".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Your code. app() is the root of the UI; components/ holds the welcome \
                    screen's parts, which you'll replace. Organize modules however you like \
                    \u{2014} it's an ordinary crate.".to_string()
            )

            Typography(content = "The other files".to_string(), kind = typography_kind::H2)
            Typography(
                content = "index.html is the page the web build mounts into; edit its <head> for \
                    meta tags or a stylesheet link. fonts/ and any other top-level folder of \
                    assets are copied into the web bundle. dev.toml holds optional idealyst dev \
                    settings, all commented out. .cargo/config.toml names the framework's \
                    registry, which the dependencies point at explicitly because several \
                    framework crate names are taken on crates.io. .mcp.json connects coding \
                    agents (lesson 0.5).".to_string()
            )

            Callout(label = "What the CLI writes".to_string()) {
                Typography(
                    content = "target/ holds cargo's output and, under target/idealyst/, the \
                        generated platform wrappers. dist/web is where idealyst build --web puts \
                        a deployable bundle. .idealyst/ holds a running session's files, such as \
                        the port coding agents use to reach the app. All of them are in the \
                        scaffold's .gitignore.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Installing, the first project, and the CLI's commands.".to_string(),
                link_label = "README \u{2014} Your first project".to_string(),
                doc_file = "/README.md#your-first-project".to_string(),
            )
        }
    }
}

// =============================================================================
// Terminal steps. Commands are the real ones; output is modeled on what the
// real tools print (captured from rustup, cargo and this CLI, then trimmed).
// =============================================================================

const RUSTC_VERSION: &str = "rustc 1.97.1 (8bab26f4f 2026-07-14)";
const CARGO_VERSION: &str = "cargo 1.97.1 (c980f4866 2026-06-30)";

fn git_version(os: Os) -> &'static str {
    match os {
        Os::Macos => "git version 2.50.1 (Apple Git-155)",
        Os::Linux => "git version 2.50.1",
        Os::Windows => "git version 2.50.1.windows.1",
    }
}

/// The reader's home, as each OS spells it.
fn home(os: Os) -> &'static str {
    match os {
        Os::Macos => "/Users/you",
        Os::Linux => "/home/you",
        Os::Windows => r"C:\Users\you",
    }
}

/// `a && b` in a POSIX shell; PowerShell sequences with `;`.
fn and_then(os: Os, a: &str, b: &str) -> String {
    match os {
        Os::Windows => format!("{a}; {b}"),
        _ => format!("{a} && {b}"),
    }
}

fn silent(_: Os, _: Targets) -> String {
    String::new()
}

// ---- 0.1 Install the CLI ----------------------------------------------------

fn install_rust(os: Os, _: Targets) -> String {
    match os {
        Os::Windows => "winget install --id Rustlang.Rustup".to_string(),
        _ => "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh".to_string(),
    }
}

fn install_rust_out(os: Os, _: Targets) -> String {
    let triple = match os {
        Os::Macos => "aarch64-apple-darwin",
        Os::Linux => "x86_64-unknown-linux-gnu",
        Os::Windows => {
            return "Found Rustup: the Rust toolchain installer [Rustlang.Rustup] Version 1.28.2\n\
                    This application is licensed to you by its owner.\n\
                    Downloading https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-pc-windows-msvc/rustup-init.exe\n\
                    Successfully verified installer hash\n\
                    Starting package install...\n\
                    Successfully installed"
                .to_string()
        }
    };
    format!(
        "info: downloading installer\n\
         \n\
         Current installation options:\n\
         \n\
         \x20  default host triple: {triple}\n\
         \x20    default toolchain: stable (default)\n\
         \x20              profile: default\n\
         \x20 modify PATH variable: yes\n\
         \n\
         1) Proceed with standard installation (default - just press enter)\n\
         2) Customize installation\n\
         3) Cancel installation\n\
         >\n\
         \n\
         \x20 stable-{triple} installed - {RUSTC_VERSION}\n\
         \n\
         Rust is installed now. Great!"
    )
}

fn add_wasm(_: Os, _: Targets) -> String {
    "rustup target add wasm32-unknown-unknown".to_string()
}

fn add_wasm_out(_: Os, _: Targets) -> String {
    "info: downloading component 'rust-std' for 'wasm32-unknown-unknown'\n\
     info: installing component 'rust-std' for 'wasm32-unknown-unknown'"
        .to_string()
}

fn check_rust(os: Os, _: Targets) -> String {
    and_then(os, "rustc --version", "git --version")
}

fn check_rust_out(os: Os, _: Targets) -> String {
    format!("{RUSTC_VERSION}\n{}", git_version(os))
}

fn install_cli(_: Os, _: Targets) -> String {
    "cargo install idealyst-cli --index sparse+https://crates.idealyst.io/index/ --locked"
        .to_string()
}

fn install_cli_out(os: Os, _: Targets) -> String {
    let bin = match os {
        Os::Windows => format!(r"{}\.cargo\bin\idealyst.exe", home(os)),
        _ => format!("{}/.cargo/bin/idealyst", home(os)),
    };
    format!(
        "    Updating `crates.idealyst.io` index\n\
         \x20Downloaded idealyst-cli v1.18.3\n\
         \x20 Installing idealyst-cli v1.18.3\n\
         \x20  Compiling proc-macro2 v1.0.95\n\
         \x20  Compiling serde v1.0.219\n\
         \x20  …\n\
         \x20  Compiling idealyst-cli v1.18.3\n\
         \x20   Finished `release` profile [optimized] target(s) in 3m 41s\n\
         \x20 Installing {bin}\n\
         \x20  Installed package `idealyst-cli v1.18.3` (executable `idealyst`)"
    )
}

fn help(_: Os, _: Targets) -> String {
    "idealyst --help".to_string()
}

fn help_out(_: Os, _: Targets) -> String {
    "Idealyst framework CLI\n\
     \n\
     Usage: idealyst <COMMAND>\n\
     \n\
     Commands:\n\
     \x20 new           Create a new project or library in a new directory\n\
     \x20 dev           Build and run with hot reload\n\
     \x20 build         Build for one or more platforms\n\
     \x20 run           Build and launch on a simulator or device\n\
     \x20 publish       Build a distribution-signed app and (optionally) ship it\n\
     \x20 doctor        Diagnose the local toolchain (Rust, web, iOS, Android; Roku pending)\n\
     \x20 update        Update this CLI to the newest release\n\
     \x20 mcp           Launch the framework MCP catalog server (stdio)\n\
     \x20 …"
        .to_string()
}

/// The platforms picked, in `doctor`'s order.
fn picked(t: Targets) -> Vec<&'static str> {
    [("web", t.web), ("ios", t.ios), ("android", t.android)]
        .into_iter()
        .filter_map(|(name, on)| on.then_some(name))
        .collect()
}

/// One platform picked checks just that one (`idealyst doctor ios`); none
/// or several, everything (bare `idealyst doctor`).
fn doctor(_: Os, t: Targets) -> String {
    match picked(t).as_slice() {
        [one] => format!("idealyst doctor {one}"),
        _ => "idealyst doctor".to_string(),
    }
}

/// `print_line` in `crates/tools/cli/src/cmd/doctor.rs`: glyph, the
/// check's id padded to 32, the detail.
fn check_line(glyph: &str, id: &str, detail: &str) -> String {
    format!("  {glyph} {id:<32} {detail}")
}

const OK: &str = "\u{2713}";
const MISSING: &str = "\u{2717}";
const NOTE: &str = "\u{25b5}";

/// `(lines, required checks that failed)` for one group. The modeled
/// machine has the tooling for the platforms picked and none for the rest
/// — except that a Mac always has the command-line tools (git came with
/// them), and nothing makes iOS buildable off a Mac.
fn doctor_group(os: Os, group: &str, picked: bool) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut failed = 0;
    let mut check = |ok: bool, required: bool, id: &str, found: &str, fix: &str| {
        if ok {
            lines.push(check_line(OK, id, found));
        } else {
            lines.push(check_line(
                if required { MISSING } else { NOTE },
                id,
                &format!("not found \u{2014} {fix}"),
            ));
            if required {
                failed += 1;
            }
        }
    };
    match group {
        "Core" => {
            check(true, true, "rustc", RUSTC_VERSION, "");
            check(true, true, "cargo", CARGO_VERSION, "");
            check(true, true, "git", git_version(os), "");
        }
        "Web" => {
            check(true, true, "wasm32-unknown-unknown target", "installed", "");
            check(false, false, "wasm-bindgen", "", "cargo install wasm-bindgen-cli — only for apps that use wgpu or web-sys (hybrid builds) and `idealyst export`");
            check(false, false, "wasm-opt", "", "Install binaryen (brew install binaryen / apt install binaryen) — release builds only");
            check(false, false, "nightly toolchain", "", "rustup toolchain install nightly — only for --strip-panics builds");
        }
        "iOS" => {
            let mac = os == Os::Macos;
            let ready = mac && picked;
            check(mac, true, "macOS host", "macOS host", "iOS builds require macOS");
            check(mac, true, "xcrun", "xcrun version 72.", "Install Xcode + Command Line Tools (xcode-select --install)");
            check(ready, true, "xcodebuild", "Xcode 26.6", "Install the full Xcode app, then: sudo xcode-select -s /Applications/Xcode.app");
            check(ready, true, "aarch64-apple-ios target", "installed", "rustup target add aarch64-apple-ios");
            check(ready, true, "aarch64-apple-ios-sim target", "installed", "rustup target add aarch64-apple-ios-sim — Apple-Silicon simulator");
            check(false, false, "x86_64-apple-ios target", "", "rustup target add x86_64-apple-ios — Intel simulator only");
            check(false, false, "ios-deploy", "", "brew install ios-deploy — needed only for physical-device installs");
        }
        _ => {
            let sdk = match os {
                Os::Macos => format!("{}/Library/Android/sdk", home(os)),
                Os::Linux => format!("{}/Android/Sdk", home(os)),
                Os::Windows => format!(r"{}\AppData\Local\Android\Sdk", home(os)),
            };
            let sep = if os == Os::Windows { r"\" } else { "/" };
            let ndk = format!("{sdk}{sep}ndk{sep}27.1.12297006");
            check(picked, true, "ANDROID_NDK_HOME", &ndk, "Set ANDROID_NDK_HOME to your NDK (e.g. ~/Library/Android/sdk/ndk/<version>)");
            check(picked, true, "aarch64-linux-android target", "installed", "rustup target add aarch64-linux-android");
            check(picked, true, "Android SDK", &sdk, "Set ANDROID_HOME, or install the SDK via Android Studio");
            check(picked, true, "SDK build-tools", "36.0.0", "Install via: sdkmanager 'build-tools;36.0.0'");
            check(picked, true, "SDK platform (android.jar)", "installed", "Install via: sdkmanager 'platforms;android-36'");
            check(picked, true, "adb", "installed", "Install the SDK platform-tools (sdkmanager platform-tools)");
            check(picked, true, "javac (JDK)", "javac 21.0.5", "Install a JDK (e.g. Temurin 21) and put javac on PATH");
        }
    }
    (lines, failed)
}

/// What `idealyst doctor` prints for the picked targets on this OS. Only
/// the core checks and a scoped platform's count toward failure: a bare
/// run reports every platform but passes on the core tools alone.
fn doctor_out(os: Os, t: Targets) -> String {
    let platforms = picked(t);
    let scope = match platforms.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    let groups: Vec<&str> = match scope {
        Some("web") => vec!["Core", "Web"],
        Some("ios") => vec!["Core", "iOS"],
        Some(_) => vec!["Core", "Android"],
        None => vec!["Core", "Web", "iOS", "Android"],
    };
    let mut out = String::from("idealyst doctor\n\n");
    let mut failures = 0;
    for group in groups {
        let picked = match group {
            "Web" => t.web,
            "iOS" => t.ios,
            "Android" => t.android,
            _ => true,
        };
        let (lines, failed) = doctor_group(os, group, picked);
        let enforced = group == "Core" || scope.is_some();
        if enforced {
            failures += failed;
        }
        out.push_str(group);
        out.push('\n');
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
    }
    if scope.is_none() {
        out.push_str("Roku toolchain checks are not implemented yet.\n\n");
    }
    if failures == 0 {
        out.push_str("All required checks passed.");
    } else {
        out.push_str(&format!("Error: {failures} required check(s) failed \u{2014} see above"));
    }
    out
}

const INSTALL_STEPS: &[TermStep] = &[
    TermStep { title: "Install Rust", command: install_rust, output: install_rust_out },
    TermStep { title: "Add the WebAssembly target", command: add_wasm, output: add_wasm_out },
    TermStep { title: "Check Rust and Git", command: check_rust, output: check_rust_out },
    TermStep { title: "Install the idealyst CLI", command: install_cli, output: install_cli_out },
    TermStep { title: "Verify it's on your PATH", command: help, output: help_out },
    TermStep { title: "Check your platform tools", command: doctor, output: doctor_out },
];

// ---- 0.2 Your first project -------------------------------------------------

fn new_project(_: Os, _: Targets) -> String {
    "idealyst new my-app".to_string()
}

fn new_project_out(_: Os, _: Targets) -> String {
    "idealyst: framework source — registry idealyst (runtime-core 3.2)\n\
     [idealyst new] scaffolded my-app (project) at my-app\n\
     \n\
     Next steps:\n\
     \x20 cd my-app\n\
     \x20 idealyst dev          # hot-reload web preview\n\
     \x20 idealyst run ios      # build + launch in the iOS simulator (requires Xcode)\n\
     \x20 idealyst run android  # build + install on an emulator or device (requires the Android NDK)"
        .to_string()
}

fn cd_project(_: Os, _: Targets) -> String {
    "cd my-app".to_string()
}

fn list_src(os: Os, _: Targets) -> String {
    match os {
        Os::Windows => "ls src -Name".to_string(),
        _ => "ls src".to_string(),
    }
}

fn list_src_out(_: Os, _: Targets) -> String {
    "app.rs  color.rs  components  components.rs  constants.rs  coordinator.rs\n\
     lib.rs  main.rs  style_helpers.rs  typeface.rs"
        .to_string()
}

fn dev(_: Os, _: Targets) -> String {
    "idealyst dev".to_string()
}

/// `idealyst dev`'s first build of a new project, trimmed.
fn dev_out(os: Os, _: Targets) -> String {
    let dir = match os {
        Os::Windows => format!(r"{}\my-app", home(os)),
        _ => format!("{}/my-app", home(os)),
    };
    format!(
        "[dev-reload] initial build…\n\
         [build-web] cargo build --target wasm32-unknown-unknown (in {dir})\n\
         [build-web] timing: total 71.40s — cargo 71.31s | glue-extract 0.02s | stage+fingerprint 0.01s\n\
         [dev web] livereload HTTP at http://0.0.0.0:8080"
    )
}

const CREATE_STEPS: &[TermStep] = &[
    TermStep { title: "Scaffold a project", command: new_project, output: new_project_out },
    TermStep { title: "Step into it", command: cd_project, output: silent },
    TermStep { title: "See what it wrote", command: list_src, output: list_src_out },
    TermStep { title: "Run it in the browser", command: dev, output: dev_out },
];

// ---- 0.3 Hot reload & the dev loop ------------------------------------------

fn dev_local(_: Os, _: Targets) -> String {
    "idealyst dev --web --local".to_string()
}

fn edit_literal(_: Os, _: Targets) -> String {
    "# in src/app.rs, change \"Edit me\" to \"Edited\", then save".to_string()
}

fn edit_literal_out(_: Os, _: Targets) -> String {
    "[dev] patched 1 site(s) in 17 ms, no rebuild".to_string()
}

fn edit_body(_: Os, _: Targets) -> String {
    "# add a third text { … } inside app(), then save".to_string()
}

fn edit_body_out(_: Os, _: Targets) -> String {
    "[hotpatch] src/app.rs · 3 function(s) redirected · cargo 871ms · diff 47ms · link 53ms · \
     resolve 3ms · jump-table 1ms · strip+write 4ms (total 979ms)"
        .to_string()
}

fn edit_shape(_: Os, _: Targets) -> String {
    "# give app() a new struct beside it, then save".to_string()
}

fn edit_shape_out(_: Os, _: Targets) -> String {
    "[dev] rebuilding: src/app.rs changed outside its function bodies\n\
     [dev-reload] change detected, rebuilding…\n\
     [build-web] timing: total 3.14s — cargo 3.07s | glue-extract 0.03s | stage+fingerprint 0.00s\n\
     [dev-reload] rebuilt — gen=2"
        .to_string()
}

fn release(_: Os, _: Targets) -> String {
    "idealyst build --web --release".to_string()
}

fn release_out(os: Os, _: Targets) -> String {
    let dist = match os {
        Os::Windows => format!(r"{}\my-app\dist\web", home(os)),
        _ => format!("{}/my-app/dist/web", home(os)),
    };
    format!(
        "[build web] bundle → {dist}\n\
         [build web] pkg/ filenames carry the build hash — safe to serve with \
         `Cache-Control: public, max-age=31536000, immutable`; keep `*.html` at `no-cache` \
         so redeploys are picked up."
    )
}

const RUN_STEPS: &[TermStep] = &[
    TermStep { title: "Start the dev loop", command: dev_local, output: dev_out },
    TermStep { title: "Change a literal", command: edit_literal, output: edit_literal_out },
    TermStep { title: "Change a function body", command: edit_body, output: edit_body_out },
    TermStep { title: "Change a shape", command: edit_shape, output: edit_shape_out },
    TermStep { title: "Build for release", command: release, output: release_out },
];

// ---- 0.4 Editor setup -------------------------------------------------------

fn install_ra(_: Os, _: Targets) -> String {
    "code --install-extension rust-lang.rust-analyzer".to_string()
}

fn install_ra_out(_: Os, _: Targets) -> String {
    "Installing extensions...\n\
     Installing extension 'rust-lang.rust-analyzer'...\n\
     Extension 'rust-lang.rust-analyzer' was successfully installed."
        .to_string()
}

fn clone_repo(_: Os, _: Targets) -> String {
    "git clone https://github.com/IdealystIO/idealyst-native".to_string()
}

fn clone_repo_out(_: Os, _: Targets) -> String {
    "Cloning into 'idealyst-native'...\n\
     remote: Enumerating objects: done.\n\
     Receiving objects: 100%, done.\n\
     Resolving deltas: 100%, done."
        .to_string()
}

fn link_extension(os: Os, _: Targets) -> String {
    match os {
        Os::Windows => "New-Item -ItemType SymbolicLink \
                        -Path \"$HOME\\.vscode\\extensions\\idealyst.vscode-idealyst-0.1.0\" \
                        -Target \"$PWD\\idealyst-native\\editors\\vscode-idealyst\""
            .to_string(),
        _ => "ln -s \"$(pwd)/idealyst-native/editors/vscode-idealyst\" \
              ~/.vscode/extensions/idealyst.vscode-idealyst-0.1.0"
            .to_string(),
    }
}

fn configure(os: Os, _: Targets) -> String {
    and_then(os, "cd my-app", "idealyst configure vscode --non-interactive")
}

fn configure_out(_: Os, _: Targets) -> String {
    "  enabled:   extensions, lint\n\
     \x20 wrote:\n\
     \x20   ./.vscode/ra-check.sh\n\
     \x20   ./.vscode/settings.json\n\
     \x20   ./.vscode/extensions.json"
        .to_string()
}

fn open_project(_: Os, _: Targets) -> String {
    "code .".to_string()
}

const EDITOR_STEPS: &[TermStep] = &[
    TermStep { title: "Install rust-analyzer", command: install_ra, output: install_ra_out },
    TermStep { title: "Get the framework checkout", command: clone_repo, output: clone_repo_out },
    TermStep { title: "Link the Idealyst extension", command: link_extension, output: silent },
    TermStep { title: "Wire them into your project", command: configure, output: configure_out },
    TermStep { title: "Open the project", command: open_project, output: silent },
];

// ---- 0.5 AI-assisted development with MCP -----------------------------------

fn show_mcp_json(os: Os, _: Targets) -> String {
    match os {
        Os::Windows => "type .mcp.json".to_string(),
        _ => "cat .mcp.json".to_string(),
    }
}

fn mcp_json(_: Os, _: Targets) -> String {
    "{\n  \"mcpServers\": {\n    \"idealyst\": {\n      \"command\": \"idealyst\",\n      \"args\": [\"mcp\"]\n    }\n  }\n}"
        .to_string()
}

fn mcp_install(_: Os, _: Targets) -> String {
    "idealyst mcp install".to_string()
}

fn mcp_install_out(_: Os, _: Targets) -> String {
    "[idealyst mcp install] wrote ./.mcp.json\n\
     [idealyst mcp install] Server name: idealyst, command: idealyst\n\
     [idealyst mcp install] Run `idealyst dev` to launch the app; the bridge port is written to \
     .idealyst/bridge.port and the MCP server discovers it from cwd."
        .to_string()
}

fn dev_for_agents_out(_: Os, _: Targets) -> String {
    "[dev] robot relay ws://127.0.0.1:50678 (tcp bridge :50679) — apps dial it; MCP discovers \
     via ~/.idealyst/apps (--no-robot to disable)\n\
     [dev-reload] initial build…\n\
     [dev web] livereload HTTP at http://0.0.0.0:8080"
        .to_string()
}

fn claude_mcp_list(_: Os, _: Targets) -> String {
    "claude mcp list".to_string()
}

fn claude_mcp_list_out(_: Os, _: Targets) -> String {
    "Checking MCP server health...\n\nidealyst: idealyst mcp - \u{2713} Connected".to_string()
}

const MCP_STEPS: &[TermStep] = &[
    TermStep { title: "The server the scaffold wired up", command: show_mcp_json, output: mcp_json },
    TermStep { title: "Or add it to an existing project", command: mcp_install, output: mcp_install_out },
    TermStep { title: "Start the app for the agent to drive", command: dev, output: dev_for_agents_out },
    TermStep { title: "Check your agent sees it", command: claude_mcp_list, output: claude_mcp_list_out },
];

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Targets = Targets { web: true, ios: true, android: true };
    const WEB: Targets = Targets { web: true, ios: false, android: false };
    const IOS: Targets = Targets { web: false, ios: true, android: false };

    #[test]
    fn doctor_scopes_to_a_single_target() {
        assert_eq!(doctor(Os::Macos, WEB), "idealyst doctor web");
        assert_eq!(doctor(Os::Macos, ALL), "idealyst doctor");
        let out = doctor_out(Os::Macos, WEB);
        assert!(out.contains("\nWeb\n") && !out.contains("\niOS\n"), "{out}");
        assert!(out.ends_with("All required checks passed."));
    }

    #[test]
    fn ios_is_never_ready_off_a_mac() {
        let out = doctor_out(Os::Linux, IOS);
        assert!(out.contains(&check_line(MISSING, "macOS host", "not found \u{2014} iOS builds require macOS")));
        assert!(!out.contains("Xcode 26.6"), "no Xcode on Linux:\n{out}");
        assert!(out.contains("Error: "), "a scoped doctor fails on its platform:\n{out}");
    }

    #[test]
    fn a_picked_ios_on_a_mac_passes() {
        let out = doctor_out(Os::Macos, IOS);
        assert!(out.contains(&check_line(OK, "xcodebuild", "Xcode 26.6")), "{out}");
        assert!(out.ends_with("All required checks passed."), "{out}");
    }

    #[test]
    fn bare_doctor_reports_unpicked_platforms_but_passes_on_core() {
        let two = Targets { web: true, ios: false, android: true };
        let out = doctor_out(Os::Windows, two);
        assert!(out.contains(&check_line(MISSING, "macOS host", "not found \u{2014} iOS builds require macOS")));
        assert!(out.contains("Roku toolchain checks are not implemented yet."));
        assert!(out.ends_with("All required checks passed."), "{out}");
        assert!(out.contains(r"C:\Users\you\AppData\Local\Android\Sdk"), "{out}");
    }

    #[test]
    fn commands_follow_the_shell() {
        assert_eq!(check_rust(Os::Linux, WEB), "rustc --version && git --version");
        assert_eq!(check_rust(Os::Windows, WEB), "rustc --version; git --version");
        assert!(install_rust(Os::Windows, WEB).starts_with("winget"));
    }

    #[test]
    fn every_step_has_a_command() {
        for steps in [INSTALL_STEPS, CREATE_STEPS, RUN_STEPS, EDITOR_STEPS, MCP_STEPS] {
            for step in steps {
                for os in [Os::Macos, Os::Linux, Os::Windows] {
                    assert!(!(step.command)(os, WEB).is_empty(), "{}", step.title);
                }
            }
        }
    }
}
