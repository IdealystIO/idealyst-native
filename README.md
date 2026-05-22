# idealyst-native

A Rust-based cross-platform UI ecosystem. Write your app once (UI tree, styles,
state, and client-side logic) and let a **backend** decide what "running on a
platform" actually means.

The ecosystem ships backends for **web** (WASM + DOM), **Android** (JNI + native
View hierarchy), **iOS** (UIKit via objc2), **macOS** (AppKit via objc2), and
**Roku** (BrightScript / SceneGraph transpile), plus an in-progress
custom-renderer family on top of [wgpu](https://wgpu.rs/). The `Backend` trait
is the only seam: writing a new backend means implementing a handful of
methods. You can target anything you can drive from Rust (a custom renderer,
a TUI, an embedded display) without touching app code.

> **Status: under construction.** APIs are still under development and may change, use at your own risk.
> See the [Roadmap](#roadmap) for what's implemented per backend.

## What makes this different

Cross-platform Rust UI is a crowded space. The thing this framework does that
others don't is bake first-class automation and agentic control into the
framework itself.

Every mounted primitive registers with a shared introspection registry that
exposes a stable handle, a `test_id`, a label, and a primitive kind. One
registry, three consumers:

- **E2E test harnesses.** Query by `test_id`, click buttons, type into
  inputs, read signals, snapshot the tree. The same `Robot` API works on
  web, iOS, and Android. No separate platform runners per target.
- **MCP server.** [`crates/robot-mcp-proxy`](crates/robot-mcp-proxy) speaks
  stdio JSON-RPC and turns each registry capability into an MCP tool. Drop
  it into a Claude Desktop config and an LLM can drive a running iOS /
  Android / web app directly: fill out forms, navigate, assert state.
- **`#[component]` methods.** A `methods! { ... }` block inside a component
  is auto-registered as JSON-callable. External automation can invoke
  component methods by name without per-app glue.

The same model gets you Detox-style E2E, dev tools, and agentic control
from one architectural seam. See
[`crates/framework/core/src/robot/`](crates/framework/core/src/robot/) for
the registry + bridge protocol, and
[`crates/robot-mcp-proxy/`](crates/robot-mcp-proxy/) for the MCP entry
point. Gated on the `robot` Cargo feature; production builds leave it off.

## Installing the CLI

The `idealyst` CLI is the entry point for everything user-facing: scaffolding
new projects, building / running them for web / iOS / Android, the hot-reload
dev server, and the doctor command for diagnosing your toolchain. It's built
from source via `cargo install`; there are no pre-built binaries yet.

### Prerequisites

- **Rust** stable toolchain (1.78+ recommended). Install via
  [rustup](https://rustup.rs/) if you don't already have it.
- **Git**. `cargo install --git` needs it on your `PATH`.

Per-platform tooling (Xcode for iOS, Android NDK for Android, `wasm-pack` for
web bundling) is only needed when you actually `build` / `run` for that target.
The CLI itself has no platform dependencies. `idealyst doctor` will tell you
what each enabled target is missing.

### Install

```bash
cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli
```

That fetches the latest commit on `master`, compiles in release mode, and drops
the `idealyst` binary into `~/.cargo/bin/` (which is on your `PATH` if you set
Rust up through `rustup`).

To pin to a specific commit / tag / branch:

```bash
cargo install --git https://github.com/IdealystIO/idealyst-native --rev <sha>    idealyst-cli
cargo install --git https://github.com/IdealystIO/idealyst-native --tag <tag>    idealyst-cli
cargo install --git https://github.com/IdealystIO/idealyst-native --branch <br>  idealyst-cli
```

To re-install / upgrade over an existing copy, add `--force`.

### Verify

```bash
idealyst --help
```

You should see the subcommand list (`new`, `init`, `dev`, `build`, `run`,
`doctor`, …).

### Building from a local checkout

If you've cloned the repo and want to install your local working copy instead
of fetching from GitHub:

```bash
git clone https://github.com/IdealystIO/idealyst-native
cd idealyst-native
cargo install --path crates/cli --force
```

The `--force` is needed once you already have `idealyst` installed; cargo
otherwise refuses to overwrite an existing binary of the same name.

### Your first project

```bash
idealyst new my-app
cd my-app
idealyst dev          # hot-reload web preview at http://localhost:8080
idealyst run ios      # build + boot in the iOS simulator (requires Xcode)
idealyst run android  # build + install on a running emulator / device
```

`idealyst new` scaffolds the [`examples/welcome`](examples/welcome) project
verbatim: a complete three-act animated intro, full Inter typeface bundle,
web + iOS + Android wiring already in place. Edit `src/app.rs` and the
per-element files under `src/components/` to make it yours.

## What is Idealyst?

I quit my job and got bored, so I started working on this.

Idealyst is a project started as a way to bring sanity to cross-platform development in a way I felt made sense. This goes beyond defining components that render everywhere, I wanted to standardize everything in the app development ecosystem: from components, theme, navigation, animations, and much, much more.

I started building off what I was comfortable with - React and React Native. I am a big fan of the way React works as a framework, and I have nearly a decade of experience working in it. I love the strong and hardworking community it has built, especially on the React Native side, to make app building simple and performant despite the complexities of using Javascript as your runtime in a native environment. It has come a long way. I decided to make a component library, alongside a vast amount of extensions components for things like Camera, Audio, push notifications, that provided a standardized API that ran on Web and Mobile with very high fidelity on both. This project exists as https://github.com/IdealystIO/idealyst-framework, and I've used it in real production apps.

The past few years I have started to really dig into Rust - and I fell in love with the syntax. I started to think to myself that it would be cool to take my experience working in Web and Mobile app development, and build a framework that could achieve near native performance. I also wanted to step away from being so heavily opinionated, allowing people to extend the framework for themselves. But this was a daunting task, and it's not a project I ever felt I had time for.

Then I quit my job, and with this new free time, I started to tinker. AI became a huge part of my workflow, it allowed me to iterate quickly on ideas I've had without having to spend days or weeks actually writing the implementations myself. I love Rust, but I don't feel like an expert and it was daunting to imagine such a big project written with it. But AI has been a huge help, and after a lot of pondering, this project is the result - and I am super proud of it so far.

## Roadmap

"Working" below means **available on at least one backend**, not "complete on
all backends." Per-backend parity for the more involved primitives is
summarised in the matrix further down.

### Framework

| Area | Status |
| --- | --- |
| `framework-core`: primitives, reactivity, render walker | Working |
| `ui!` / `jsx!` / `#[component]` macros | Working |
| `stylesheet!` macro (themes, variants, overrides) | Working |
| `Ref<H>`: primitive handles + user-component handles via `methods!` | Working |
| Reactive `if` / `when`, `for` loops in DSLs | Working |
| `idea-ui` component library (Card, Modal, Popover, Select, Switch, Tabs, Field, Alert, …) | Working |
| Icon registry (`icons-lucide`) | Working |
| Robot automation + MCP server: introspection registry, `#[component] methods!`, agent control | Working |
| Hot reload: dev server + AAS (Application-as-a-Service) shell + wire protocol | Working |
| Server-driven UI: wire protocol + `SceneModel` snapshot | Working |
| Custom rendering: `render-wgpu` (core, phone, tablet, tv skins) | In progress |
| Native backend: interactions / media / OS integration | In progress |
| Async data / `Resource<T>` | Planned |
| Accessibility: first-class `AccessibilityProps` on every primitive, per-backend `set_accessibility_*` hooks | Planned |
| SSR + Hydration | Planned |

### Backends

| Backend | Status | Notes |
| --- | --- | --- |
| `backend-web` (WASM + DOM) | Working | Reference backend. Most complete primitive coverage. |
| `backend-android-mobile` (JNI + Views) | Working | Phone form factor. `tv` variant is a stub. |
| `backend-ios-mobile` (UIKit via objc2) | Working | Phone form factor. `tv` variant is a stub. |
| `backend-macos` (AppKit via objc2) | Early | Window shell + basics. Many primitives unimplemented (see matrix). |
| `backend-roku` (BrightScript / SceneGraph transpile) | Working | Theme switching temporarily disabled (token refactor); panics on theme update. |
| `render-wgpu` (custom renderer, embeddable) | In progress | Drives the same `Backend` trait through a GPU pipeline; `host-winit` / `host-web` wire it to OS windows. |

### Per-backend primitive coverage

A blank cell means the trait default panics with `unimplemented!()`. Author
code that reaches for that primitive on that backend will crash, not
silently no-op.

| Primitive | web | iOS-mobile | Android-mobile | macOS | Roku | wgpu |
|---|---|---|---|---|---|---|
| View / Text / Button (core) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Image | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| TextInput | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| ScrollView | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| Slider | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| Toggle | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| Icon | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| ActivityIndicator | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| Graphics | ✓ | ✓ | ✓ |  | ✓ | ✓ |
| Link | ✓ | ✓ | ✓ |  |  | ✓ |
| Video | ✓ |  | ✓ |  |  | ✓ |
| Virtualizer / FlatList | ✓ |  | ✓ |  |  | ✓ |
| `Primitive::External` (third-party SDKs: Maps, WebView) | ✓ | ✓ | ✓ |  |  | partial |

Web and Android-mobile are the most complete. iOS-mobile is catching up but
missing `Video` and `Virtualizer`. macOS is a structural skeleton that
needs the same UIKit-style primitive work iOS already has. Roku is locked
behind the theme-refactor regression noted above. The wgpu renderer is
implemented at the `Backend` trait level but is still in active development
on the rendering side.

## The shape of an app

Application code is one crate that depends only on `framework-core`. It declares
components, styles, and a root tree, and knows nothing about the platform it
will run on.

```rust
use framework_core::{ui, component, signal, Primitive};

#[component]
pub fn app() -> Primitive {
    let count = signal!(0);

    ui! {
        Text { "Hello from idealyst-native" }
        Button(
            label = "Increment",
            on_click = move || count.update(|n| *n += 1),
        )
        Text { format!("Count: {}", count.get()) }
    }
}
```

A **platform host** is a tiny separate crate per target. It wires the shared
app to a backend and a mount point. The host is the only place that knows
what platform it's running on. The same `app()` is byte-for-byte identical
on every platform.

The full surface of `ui!` / `jsx!` / `#[component]` / `stylesheet!` / `Ref<H>`
is documented in **[`docs/ui-layer.md`](docs/ui-layer.md)**. Read that for the
authoring guide. The deep dives on reactivity, styling, primitives, and the
backend contract live alongside it under [`docs/`](docs/).

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                       Application                         │
│   components, signals, stylesheets, ui! / jsx! / ...      │
└──────────────────────────────────────────────────────────┘
                            │
                            ▼
┌──────────────────────────────────────────────────────────┐
│                     framework-core                        │
│  reactivity (signals, effects)  · primitives (View/Text/  │
│  Button/When/...)  ·  styles + theming  ·  render walker  │
└──────────────────────────────────────────────────────────┘
                            │
                            ▼  (Backend trait)
        ┌───────┬───────┬───────┬───────┬───────┬───────┐
        ▼       ▼       ▼       ▼       ▼       ▼       ▼
       web   Android   iOS    macOS   Roku   wgpu     (yours)
       DOM   JNI/View  UIKit  AppKit  BrS    GPU
```

The framework controls **what** to render and **when** to update. The backend
controls **how** that happens on the target platform. The seam is small enough
that a new backend is on the order of "implement a trait" rather than "fork
the framework."

For the long version (render walker, per-primitive lifecycle, the rules a
backend must follow), see **[`docs/backend.md`](docs/backend.md)**.

## Subsystem status

Beyond per-primitive parity (the matrix above), a few cross-cutting subsystems
are worth calling out:

- **Animation.** The `Backend` trait carries `set_animated_f32` /
  `set_animated_color` / `animate_icon_stroke` hooks, and `framework-core`
  exposes `AnimatedValue<T>` with spring + decay drivers and a per-thread
  clock. The full author-facing model (value handles, animator factories,
  declarative `Transition` on style props vs. imperative interruptible
  motion) is documented in **[`docs/animation.md`](docs/animation.md)**.
  Style transitions and gesture-driven animations both flow through the
  same per-frame write path.
- **Accessibility.** **Currently minimal; this is a known gap.** Image
  carries an `accessibilityLabel` that maps to `alt` / `accessibilityLabel`
  / `contentDescription`. Link carries an accessibility role on native. The
  identity layer exposes stable string IDs intended for `aria-labelledby` /
  `aria-controls`. What's not yet in the trait: a generalised
  `AccessibilityProps` struct on every primitive, `set_accessibility_label` /
  `set_role` / `announce_for_accessibility`, focus-order plumbing.
  Production apps will need this; it's on the roadmap above.
- **Cross-backend test parity.** The reactive + walker test suite uses a
  `MockBackend` that records every call into an event log. 98 tests
  exercise diamond invalidation, fan-out ordering, dynamic-dependency
  drift, ref minting, control flow, and rebuild scenarios. The web backend
  adds its own suite. A multi-backend conformance suite that runs the same
  scenarios against every backend (and against the three reactive-binding
  paths: Rust `Effect`, native-side dispatcher, wire-serialized metadata)
  is the natural next step for keeping "Working" mechanically defensible.

## Repository layout

The workspace is grouped by concern. Within each group, each subdirectory is a
Cargo crate.

```
crates/
  framework/            # The framework itself
    core/               # Primitives, Backend trait, render walker, reactivity, styles
    macros/             # #[component], ui!, jsx!, stylesheet! proc-macros
    reactive/
      arena/            # Arena allocator backing the reactive system
      refs/             # Ref<H> machinery
    wire/               # Dev-mode hot-reload + server-driven UI wire protocol
    hot/                # Hot-reload runtime facade over subsecond
    dev-client/         # App-side replay engine; wraps a real Backend and applies an incoming stream
    native-layout/      # Taffy-based flex layout helper for native backends (iOS/Android/macOS)
    mcp/                # Framework MCP component catalog (component metadata for tooling)

  backend/              # Backend implementations of the framework Backend trait
    web/                # WASM + DOM backend
    ios/
      core/             # iOS shared layer (used by mobile + tv)
      mobile/           # iOS UIKit backend, phone form factor
      tv/               # iOS tvOS variant (stub)
    ios-stack/          # Alternative iOS backend (research / experiment)
    android/
      core/             # Android shared layer
      mobile/           # Android JNI + View backend, phone form factor
      tv/               # Android TV variant (stub)
    apple/core/         # AppKit/UIKit shared helpers (objc2)
    macos/              # macOS AppKit backend
    roku/               # Roku BrightScript / SceneGraph generator backend
    aas-shell-native/   # Native AAS-shell (sync WebSocket transport, mDNS discovery)
    posix-log-capture/  # Robot log-buffer LogCapture impl
    [README.md per backend describes its quirks]

  render/               # Custom rendering; implements Backend on top of a GPU pipeline
    api/                # Platform-agnostic preview-backend API
    wgpu/               # wgpu render backend

  host/                 # Per-OS shells that host render-wgpu
    appkit/             # AppKit host for macOS
    winit/              # Winit host for cross-platform native windows
    web/                # Browser host for the wgpu backend running on WebGPU

  native/               # Pre-wired render-wgpu shells per form factor
    phone/ tablet/ tv/  # Each crate just picks defaults + skin

  skin/                 # Visual "skins" that fake a native look in the wgpu renderer
    ios-sim/ android-sim/

  ui/                   # User-facing component libraries
    idea-ui/            # Cross-platform component library (Card, Modal, Popover, Field, …)
    icons-lucide/       # Lucide icon pack; tree-shakeable, only referenced icons ship
    idea-ui-docs-derive/# #[derive(DocControls)] proc macro powering the docs site

  sdk/                  # Third-party-style extensions wired through Primitive::External
    maps/ maps-core/ maps-web/   # MapView primitive
    webview/            # WebView primitive (cfg-gated single-crate pattern)
    idea-codeblock/     # Read-only colored-text panel primitive

  cli/                  # idealyst CLI: scaffold, dev-serve, build, run, doctor
  build/                # Per-target build orchestration (web, ios, android, macos, roku, aas, sim)
  run/                  # Per-target run helpers (ios sim, android device, macos, roku)
  dev/                  # Hot-reload dev server pieces
    server/ http/ reload/ web-host/

  port/                 # Source porters (React/Vue/Svelte/Solid → idealyst Rust); see ./port/README.md
  mcp-server/           # Stdio MCP server exposing the framework's component catalog
  robot-mcp-proxy/      # Host-side MCP server for robots
```

Where a crate has non-obvious wiring, runtime requirements, or behavioural
quirks, it has its own `README.md`. The most useful entry points:

- [`crates/framework/core/README.md`](crates/framework/core/README.md):
  the `Backend` trait, primitive vocabulary, render walker, reactivity
  internals.
- [`crates/framework/macros/README.md`](crates/framework/macros/README.md):
  `#[component]`, `ui!`, `jsx!`, `stylesheet!`. The author-facing macros.
- [`crates/framework/wire/README.md`](crates/framework/wire/README.md):
  wire protocol shared by hot-reload + server-driven UI.
- [`crates/framework/native-layout/README.md`](crates/framework/native-layout/README.md):
  how iOS/Android backends drive flex layout through Taffy.
- [`crates/backend/web/README.md`](crates/backend/web/README.md):
  scheduler / time-source bootstrap requirements, animated-value capabilities.
- [`crates/backend/ios/mobile/README.md`](crates/backend/ios/mobile/README.md):
  UIKit quirks the backend works around (scroll bounds, intrinsic sizing,
  corner-radius clamping, etc.).
- [`crates/backend/android/mobile/README.md`](crates/backend/android/mobile/README.md):
  Kotlin runtime requirements; JNI integration.
- [`crates/backend/macos/README.md`](crates/backend/macos/README.md):
  what's implemented vs. still missing on the AppKit backend.
- [`crates/backend/roku/README.md`](crates/backend/roku/README.md):
  theme-switching regression status, generator backend caveats.
- [`crates/render/wgpu/README.md`](crates/render/wgpu/README.md):
  the GPU rendering pipeline, host requirements, debug-stats feature.
- [`crates/port/README.md`](crates/port/README.md): source-porter design
  (compiler skeleton + AI hole-filling).

For framework design docs (UI layer, reactivity, styling, animation, fonts,
primitives, backend contract) see **[`docs/`](docs/)**.

## Running the examples

The examples under [`examples/`](examples/) all use the same CLI:

```bash
cd examples/welcome
idealyst dev                # hot-reload web preview at http://localhost:8080
idealyst run ios            # iOS simulator
idealyst run android        # Android emulator / device
```

`idealyst new my-app` is shorthand for "copy `examples/welcome` to `my-app` and
adjust crate names." Once you're in either, the workflow is the same.

Other examples worth knowing about:

- [`examples/animation-test`](examples/animation-test): exercises the
  animation system (springs, decay, gestures).
- [`examples/fiddle`](examples/fiddle): sandbox for quick framework
  experiments.
- [`examples/idea-ui-docs`](examples/idea-ui-docs): the live docs site for
  the `idea-ui` component library, built with the framework itself.
- [`examples/hello-roku`](examples/hello-roku): minimal Roku target.
- [`examples/mcp-demo`](examples/mcp-demo): exercises the framework MCP
  catalog.

## Build profile

The workspace's release profile is tuned for **binary size**, not CPU speed.
UI workloads aren't compute-bound, but bytes-over-the-wire matter for the WASM
target. `opt-level = "z"`, LTO on, single codegen unit, panic = abort,
symbols stripped. A `release-debug` profile inherits release but keeps DWARF
so `twiggy` can attribute bytes to specific functions.

## Special Thanks

Dioxus is another really cool initiative creating a Rust based cross platform development framework. I even use one of their tools (Taffy!) as the flex-layout renderer for the ios and android backends. Idealyst's approach is unique from theirs in how we render the applications, but I certainly was inspired by their work. Please check them out and support their development! https://github.com/DioxusLabs/dioxus

Special thanks to @GelScott for dealing with my insane rambles as I designed and implemented the framework.

## License

MIT.
