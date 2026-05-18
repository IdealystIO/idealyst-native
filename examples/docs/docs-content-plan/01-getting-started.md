# Getting Started

This page gets you from "nothing installed" to a running Idealyst app
that updates live when you edit it. The same project will build for
web, iOS, Android, and Roku out of the box — you pick which platform
to run.

## What you're about to do

1. Install the `idealyst` CLI.
2. Scaffold a new project.
3. Run the dev server.
4. Edit the code and see it update on screen.

The whole thing should take a few minutes if Rust is already on your
machine.

## What you need

- **Rust** (1.70 or newer). Install via [rustup](https://rustup.rs/)
  if you don't have it.
- **Tools for the platform(s) you want to run.** You only need the
  ones you'll actually use — the scaffold builds for all of them, but
  the dev server only invokes the toolchain for the targets you
  select.
  - *Web* — a modern browser. The CLI runs `wasm-pack` for you.
  - *iOS* — Xcode and the iOS Simulator. No manual setup beyond the
    standard Xcode install.
  - *Android* — Android Studio (for the SDK and an emulator) plus the
    Android NDK.
  - *Roku* — a Roku device in developer mode, or the Roku simulator.
    Experimental; expect rough edges.

A future `idealyst doctor` command will check your toolchain
automatically and report what's missing. Until that lands, the
platform builders will tell you what they couldn't find if something
isn't installed.

## Install the CLI

```bash
cargo install idealyst-cli
```

The crate is called `idealyst-cli` but the binary it installs is
`idealyst`. Confirm with:

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
  src/
    lib.rs            # exports `pub fn app() -> Primitive` — your whole app
```

That's it. There is no `ios/` folder, no `android/`, no `web/`. The
per-platform host crates that turn your `app()` into a runnable
binary or bundle are generated on demand by the CLI into a build
cache. You don't author them, and you don't see them unless you go
looking.

The generated `Cargo.toml` enables every supported platform as a
build target by default:

```toml
[package.metadata.idealyst.app]
name = "my-app"
bundle_id = "com.example.my-app"
targets = ["web", "ios", "android", "roku"]
```

`targets` is the list of platforms the CLI will build for when you
don't pass an explicit `--web` / `--ios` / `--android` / `--roku`
flag. You can prune this list later (drop platforms you don't
target), or add a new entry when more platforms ship.

## What's in `src/lib.rs`

The scaffold's `app()` function is a small showcase that exercises
most of the framework's primitives — text, a counter with a button,
a toggle, an icon, a scrollable container — so you can see what's
on offer and have something concrete to modify.

Every Idealyst app starts the same way: one function annotated with
`#[component]` that returns a `Primitive` tree. The minimal shape
looks like this:

```rust
use framework_core::{component, signal, ui, Primitive};

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
```

The scaffold expands on this with more primitives, but the structure
is the same — a component, some state, a UI tree.

A few things to notice, without going deep on any of them yet:

- `#[component]` marks `app()` as a component. Every Idealyst app
  starts from one.
- `signal!(0)` declares reactive state. The `count.get()` inside
  `format!` is a tracked read; the `count.update(...)` inside the
  button's `on_click` is a write. The framework keeps the
  surrounding `Text` in sync when the button is pressed — see
  [Reactivity](#) on the Overview page for the mechanism.
- `ui! { ... }` is the DSL for declaring the UI tree. It lowers to
  plain Rust function calls; you don't have to import the primitive
  constructors (`view`, `text`, `button`) explicitly because the
  macro emits absolute paths.

## Building UI: components, not just primitives

The scaffold uses framework-core primitives (`View`, `Text`,
`Button`) directly. For most projects you'll want higher-level
pieces too — headings, cards, themed buttons, layout stacks.

You have four options:

- **idea-ui** — the first-party component library. The docs site is
  built with it. See [idea-ui](#) for what's included and how to use
  it.
- **Build your own** with the framework's theme and stylesheet
  system on top of the primitive vocabulary. See
  [Styles](#) for how that system works.
- **Use a third-party library** built on framework-core.
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

Hot reload is on by default. The dev server watches your source
files, rebuilds incrementally on save, and patches the running app
in place — your scroll position, navigation state, and current
signal values are preserved across edits.

## Edit something

With `idealyst dev --web` running and the browser open:

1. Open `src/lib.rs`.
2. Change `"Hello, Idealyst"` to anything else.
3. Save.

The app should update without a page reload, and the counter value
should still be whatever you'd clicked it to. If a full reload
happens instead, that's a structural change the patcher couldn't
apply — usually a new top-level component, a re-shaped signal, or a
change to the component graph. Hot reload covers most edits; full
reloads happen rarely and only when they have to.

## Building for production

When you're ready to ship, run a release build:

```bash
idealyst build --release
```

Same flag shape as `dev` — with no platform flag, it builds every
target in your `Cargo.toml`. Add `--web` / `--ios` / `--android` /
`--roku` to narrow.

Output lands in `target/idealyst/<platform>/`. Each platform's
directory holds whatever the platform expects: a WASM bundle and an
`index.html` for web, an Xcode `.app` for iOS, an APK for Android,
a side-loadable channel package for Roku.

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
- **Primitives** — the full list of built-in nodes (`View`, `Text`,
  `Button`, `Pressable`, `ScrollView`, `Icon`, and more).
- **Navigation** — drawer and tab navigators, screens, routes.

The Overview page is the right read if you skipped it — it covers
the model the rest of the documentation assumes.
