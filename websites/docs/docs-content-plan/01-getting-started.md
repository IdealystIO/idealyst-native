# Getting Started

This page gets you from "nothing installed" to a running Idealyst app
that updates live when you edit it. The same project builds for web,
iOS, Android, macOS, and the other shipped platforms — you pick which
platform to run.

## What you're about to do

1. Install the `idealyst` CLI.
2. Scaffold a new project.
3. Run the dev server.
4. Edit the code and see it update on screen.

The whole thing should take a few minutes if Rust is already on your
machine.

## What you need

- **Rust**, stable (1.78 or newer recommended). Install via
  [rustup](https://rustup.rs/) if you don't have it.
- **Tools for the platform(s) you want to run.** You only need the
  ones you'll actually use — the CLI only invokes the toolchain for
  the targets you select.
  - *Web* — a modern browser and the `wasm32-unknown-unknown` Rust
    target (`rustup target add wasm32-unknown-unknown`). Release
    builds also use `wasm-opt` from binaryen.
  - *iOS* — Xcode with the iOS Simulator, plus the iOS Rust targets
    (`aarch64-apple-ios-sim` on Apple Silicon).
  - *Android* — the Android SDK (with an emulator or a connected
    device) and the Android NDK, with `ANDROID_NDK_HOME` set.
  - *Roku* — experimental, build-only: `idealyst build --roku`
    produces a package to side-load. There is no dev loop for Roku.

`idealyst doctor` checks your toolchain for the core, web, iOS, and
Android targets and tells you what's missing and how to fix it.

## Install the CLI

```bash
cargo install idealyst-cli --index sparse+https://crates.idealyst.io/index/ --locked
```

The CLI is published to the framework's registry,
`crates.idealyst.io`. The crate is called `idealyst-cli` but the
binary it installs is `idealyst`. Confirm with:

```bash
idealyst --version
```

## Create a project

```bash
idealyst new my-app
cd my-app
```

You'll get a layout like this:

```
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
```

`src/main.rs` is required: `idealyst::entry!` reads your app config
and emits the `main` each platform's shell needs. Nothing in it names
a platform.

There is no `ios/` folder and no `android/`. The iOS and Android
wrapper crates that turn your `app()` into a native library are
generated on demand by the CLI into a build cache
(`target/idealyst/<project>/`). You don't author them, and you don't
see them unless you go looking. The web build compiles your crate
directly.

The generated `Cargo.toml` starts with web as the only target,
because it needs no per-platform toolchain:

```toml
[package.metadata.idealyst.app]
name      = "My App"
bundle_id = "com.example.my_app"
version   = "0.0.1"
targets   = ["web"]
```

`name` is the display name, title-cased from the crate name.
`bundle_id` uses underscores, not hyphens, because Android derives
JNI symbol names from it; pass `--bundle-id` to `idealyst new` to
choose your own.

`targets` is the list of platforms `idealyst dev` and `idealyst
build` use when you don't pass an explicit `--web` / `--ios` /
`--android` flag. Add entries (`targets = ["web", "ios", "android"]`)
when you're ready for more platforms.

## What's in `src/lib.rs`

The scaffold's `app()` is an animated welcome screen built from a
handful of components in `src/components/` — something concrete to
read and modify.

Every Idealyst app exposes one `pub fn app() -> Element`. Keep it
thin and put the tree in a `#[component]` — on the web, hot patching
reaches only code called through a component. The minimal shape
looks like this:

```rust
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
```

The welcome scaffold is larger, but the structure is the same —
components, some state, a UI tree.

A few things to notice, without going deep on any of them yet:

- `#[component]` makes `Root` a component, so `app()` can render it
  as `Root()` inside `ui!`.
- `signal(0)` declares reactive state. The `{count}` slot in the
  text is a live read of the signal; the `count.update(...)` inside
  the button's `on_click` is a write. The framework keeps that
  `text` in sync when the button is pressed — see
  [Reactivity](reactivity) for the mechanism.
- `ui! { ... }` is the DSL for declaring the UI tree. It lowers to
  plain Rust function calls; you don't have to import the primitive
  constructors (`view`, `text`, `button`) explicitly because the
  macro emits absolute paths.

## Building UI: components, not just primitives

The example above uses runtime-core primitives (`view`, `text`,
`button`) directly. For most projects you'll want higher-level
pieces too — headings, cards, themed buttons, layout stacks.

You have four options:

- **idea-ui** — the first-party component library. The docs site is
  built with it.
- **Build your own** with the framework's theme and stylesheet
  system on top of the primitive vocabulary. See
  [Styles](styles) for how that system works.
- **Use a third-party library** built on runtime-core.
- **Skip it.** Primitives alone are a complete option.

The framework underneath is the same in every case.

## Run it

```bash
idealyst dev
```

With no flags, the dev server builds every platform listed in
`targets` and runs each one. If you want only one platform — common
during day-to-day work — name it explicitly:

```bash
idealyst dev --web        # web only
idealyst dev --ios        # iOS simulator only
idealyst dev --android    # Android emulator/device only
```

You can combine flags (`idealyst dev --web --ios`) to run multiple
platforms in parallel; each gets its own watch + rebuild loop and
Ctrl-C tears them all down together.

By default `idealyst dev` runs your app in one sidecar process on
your machine, and each platform connects to it as a thin client.
`--local` builds and runs the app on each platform instead. See
[Dev tools](dev-tools).

Hot reload is on by default. A literal edit inside `ui!` applies in
milliseconds; an edit to function bodies is hot-patched in seconds
and signal values are carried over. A change to a signature, a
struct, or a new item rebuilds and restarts the app, and signal
values are not carried over.

## Edit something

With `idealyst dev --web` running and the browser open:

1. Open `src/lib.rs`.
2. Change `"Hello, Idealyst"` to anything else.
3. Save.

The app should update without a page reload, and the counter value
should still be whatever you'd clicked it to. If a full reload
happens instead, that's a shape change the patcher can't apply — a
changed signature, a changed struct, or a new item. Hot reload covers
most edits.

## Building for production

When you're ready to ship, run a release build:

```bash
idealyst build --release
```

Same flag shape as `dev` — with no platform flag, it builds every
target in your `Cargo.toml`. Add `--web` / `--ios` / `--android` /
`--roku` to narrow.

Web output lands in `dist/web/` (the `pkg/` bundle, `index.html`,
and assets); Roku's side-loadable package lands in `dist/roku/`. The
iOS build produces the static library and the Android build produces
the `.so`, from the generated wrapper under
`target/idealyst/<project>/`; `idealyst run ios` and `idealyst run
android` package, install, and launch them.

## Exporting a platform project (coming soon)

`idealyst scaffold <platform>` will copy the generated host crate
for a platform out of the build cache and into your repo as an
ordinary directory, so you can hand-edit it. After exporting, the
CLI builds *from* your exported source instead of regenerating, and
your changes stick.

Not yet implemented. Use the default in-cache flow for now.

## What's next

If the counter is on screen, you have a working setup. Next up:

- **Components** — define your own and reuse them.
- **Reactivity** — signals, effects, the rules for what gets re-run.
- **Styles** — stylesheets, themes, variants.
- **Primitives** — the full list of built-in nodes (`view`, `text`,
  `button`, `pressable`, `scroll_view`, `icon`, and more).
- **Navigation** — stack and swap navigators, screens, routes.

The Overview page is the right read if you skipped it — it covers
the model the rest of the documentation assumes.
