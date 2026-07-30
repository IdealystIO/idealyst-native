# idealyst-native

A Rust-based cross-platform UI ecosystem. Write your app once (UI tree, styles,
state, and client-side logic) and let a **backend** decide what "running on a
platform" actually means.

The ecosystem ships backends for **web** (WASM + DOM), **Android** (JNI + native
View hierarchy), **iOS** (UIKit via objc2), **macOS** (AppKit via objc2), and
**Roku** (BrightScript / SceneGraph transpile), plus an in-progress
custom-renderer family on top of [wgpu](https://wgpu.rs/). The backend seam is
two traits deep: `runtime_scene::Host` (seven structural operations) plus the
per-primitive capability traits a backend chooses to implement, every one of
which has a documented default. You can target anything you can drive from Rust
(a custom renderer, a TUI, an embedded display) without touching app code.

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
- **MCP server.** `idealyst mcp` ([`crates/mcp/server`](crates/mcp/server))
  speaks stdio JSON-RPC and turns each registry capability into an MCP tool
  (alongside the static component catalog). Drop it into a Claude Code /
  Desktop config and an LLM can drive a running iOS / Android / web app
  directly: fill out forms, navigate, assert state. It reaches the app's
  Robot bridge by discovery (`~/.idealyst/apps/`) or an explicit
  `--robot-port`.
- **`#[component]` methods.** Nested `#[method]` fns inside a component
  is auto-registered as JSON-callable. External automation can invoke
  component methods by name without per-app glue.

The same model gets you Detox-style E2E, dev tools, and agentic control
from one architectural seam. See
[`crates/runtime/shared/src/robot/`](crates/runtime/shared/src/robot/) for
the registry + bridge protocol,
[`crates/runtime/vocabulary/src/robot.rs`](crates/runtime/vocabulary/src/robot.rs)
for the per-primitive registration, and
[`crates/mcp/server/`](crates/mcp/server/) (the `idealyst mcp` command) for
the MCP entry point. Gated on the `robot` Cargo feature; production builds
leave it off.

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
cargo install --path crates/tools/cli --force
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
idealyst publish ios  # distribution-signed .ipa (add --upload for App Store Connect)
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
| Runtime core: primitives (`runtime-vocabulary`), scene + mount (`runtime-scene`), reactive kernel (`runtime-world`) | Working |
| `ui!` / `jsx!` / `#[component]` macros | Working |
| `stylesheet!` macro (themes, variants, overrides) | Working |
| `Ref<H>`: primitive handles + user-component handles via `#[method]` | Working |
| Reactive `if` / `when`, `for` loops in DSLs | Working |
| `idea-ui` component library (Card, Modal, Popover, Select, Switch, Tabs, Field, Alert, …) | Working |
| Icon registry (`icons-lucide`) | Working |
| Robot automation + MCP server: introspection registry, `#[component]` + `#[method]`, agent control | Working |
| Hot reload: dev server + runtime-server (Application-as-a-Service) shell + wire protocol | Working |
| Server-driven UI: wire protocol + `SceneModel` snapshot | Working |
| Custom rendering: `render-wgpu` (core, phone, tablet, tv skins) | In progress |
| Native backend: interactions / media / OS integration | In progress |
| Async data: `resource` / `mutation` (`runtime_vocabulary::async_reactive`) | Working |
| Accessibility: `AccessibilityProps` on every primitive + `caps::A11yOps` per backend | Working |
| SSR / SSG + hydration (`backend-ssr`, byte-identical goldens) | Working |

### Backends

| Backend | Status | Notes |
| --- | --- | --- |
| `backend-web` (WASM + DOM) | Working | Reference backend. Most complete primitive coverage. |
| `backend-android-mobile` (JNI + Views) | Working | Phone form factor. `tv` variant is a stub. |
| `backend-ios-mobile` (UIKit via objc2) | Working | Phone form factor. `tv` variant is a stub. |
| `backend-macos` (AppKit via objc2) | Early | Window shell + basics. Many primitives unimplemented (see matrix). |
| `backend-roku` (BrightScript / SceneGraph transpile) | Working | Theme switching temporarily disabled (token refactor); panics on theme update. |
| `render-wgpu` (custom renderer, embeddable) | In progress | Implements the same `Host` + capability traits over a GPU pipeline; `host-winit` / `host-web` wire it to OS windows. |

### Per-backend primitive coverage

A blank cell means the backend inherits the capability trait's default. Those
defaults degrade rather than crash: a missing widget renders the
`ExternalOps::missing_primitive_placeholder` box, and a missing container
lowers to a plain view. What *does* panic is an unregistered third-party
payload — see the registration seam below.

| Element | web | iOS-mobile | Android-mobile | macOS | Roku | wgpu |
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
| Third-party primitives (SDK payload + registered handler) | ✓ | partial | partial | partial | partial | partial |

Web and Android-mobile are the most complete. iOS-mobile is catching up but
missing `Video` and `Virtualizer`. macOS is a structural skeleton that
needs the same UIKit-style primitive work iOS already has. Roku is locked
behind the theme-refactor regression noted above. The wgpu renderer implements
the full capability surface but is still in active development on the
rendering side. Per-SDK host coverage — which third-party primitives have a
real handler on which backend, and which fall back to the placeholder — is
tabulated in
[`docs/migrating-to-runtime-v2.md`](docs/migrating-to-runtime-v2.md#external-sdks-the-third-party-primitive-layer).
The short version: the caps-generic SDKs (`markdown`, `table`, `codeblock`)
work on every host from one handler, the DOM-shaped ones (`maps`, `webview`,
`canvas`, `svg`, `video`, `form`) are web-first with a placeholder elsewhere,
and `toolbar` has a real macOS `NSToolbar` leg.

## The shape of an app

Application code is one crate that depends on the author surface
(`runtime-core`, plus `runtime-vocabulary` and `runtime-scene`). It declares
components, styles, and a root tree, and knows nothing about the platform it
will run on. `examples/welcome` is the scaffold's source of truth for the dep
set.

```rust
use runtime_core::{component, signal, ui, Element};

#[component]
pub fn App() -> Element {
    let count = signal(0);

    ui! {
        text { "Hello from idealyst-native" }
        button(
            label = "Increment",
            on_click = move || count.update(|n| n + 1),
        )
        text { move || format!("Count: {}", count.get()) }
    }
}
```

Two things the snippet is showing on purpose: primitives are **snake_case**
(PascalCase always means a `#[component]`), and `update` takes `&T` and returns
the new value — writes stage and commit at the driver's flush, so
`set(count.get() + 1)` twice in one handler would net `+1` while two `update`s
net `+2`. See
[`docs/automatic-batching.md`](docs/automatic-batching.md).

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
│              runtime-world / scene / vocabulary           │
│  reactive kernel (staged-commit signals, effects, memos)  │
│  · Element + realize + Registry · primitives (view/text/  │
│  button/when/...) as handlers · styles + theming          │
└──────────────────────────────────────────────────────────┘
                            │
                            ▼  (Host + caps::*Ops)
        ┌───────┬───────┬───────┬───────┬───────┬───────┐
        ▼       ▼       ▼       ▼       ▼       ▼       ▼
       web   Android   iOS    macOS   Roku   wgpu     (yours)
       DOM   JNI/View  UIKit  AppKit  BrS    GPU
```

The framework controls **what** to render and **when** to update. The backend
controls **how** that happens on the target platform. The seam is small enough
that a new backend is on the order of "implement a trait" rather than "fork
the framework."

For the long version (the `Host` trait, the capability traits, the mount path,
the flush driver, and the rules a backend must follow), see
**[`docs/backend.md`](docs/backend.md)**.

## Subsystem status

Beyond per-primitive parity (the matrix above), a few cross-cutting subsystems
are worth calling out:

- **Reactivity.** Writes **stage** and commit at the backend's flush driver,
  so every event handler, timer, animation frame, and async completion is one
  logical update with one deduped effect pass — and memos settle before user
  effects, which makes the diamond case glitch-free. The kernel is
  `crates/runtime/world`; the model and the per-backend drivers are in
  **[`docs/automatic-batching.md`](docs/automatic-batching.md)** and
  **[`docs/reactivity.md`](docs/reactivity.md)**.
- **Animation.** `caps::AnimationOps` carries `set_animated_f32` /
  `set_animated_color`, `IconOps` carries `animate_icon_stroke`, and the
  shared substrate exposes `AnimatedValue<T>` with spring + decay drivers and
  a per-thread clock. The full author-facing model (value handles, animator
  factories, declarative `Transition` on style props vs. imperative
  interruptible motion) is documented in
  **[`docs/animation.md`](docs/animation.md)**. Style transitions and
  gesture-driven animations flow through the same per-frame write path.
- **Accessibility.** Every primitive payload carries an `AccessibilityProps`;
  every builder exposes granular `a11y_*` setters and a whole-struct
  `accessibility` setter; `caps::A11yOps` (`update_accessibility`,
  `announce_for_accessibility`, `dump_accessibility_tree`) is what each
  backend maps to UIAccessibility / NSAccessibility /
  `AccessibilityNodeInfo` / ARIA. See
  **[`docs/accessibility.md`](docs/accessibility.md)**.
- **Cross-backend parity, mechanically pinned.** The op sequences the runtime
  emits are frozen as goldens and checked on every run: backend op-log
  streams (`crates/dev/scene-parity`), pixel-exact CPU framebuffers,
  cell-exact terminal grids, byte-exact Roku command streams, byte-exact SSR
  html + head CSS, byte-exact email output, and the whole website's SSG
  output. On top of that, `crates/dev/robot-e2e/examples/conformance` drives
  the same scenarios against real backends through the robot bridge.

## Repository layout

The workspace is grouped by concern. Within each group, each subdirectory is a
Cargo crate.

```
crates/
  runtime/              # The framework itself
    world/              # Reactive kernel: per-world arenas, Copy handles, staged-commit flush
    scene/              # Element, Host, Registry, realize + structural drivers
    vocabulary/         # Primitives (payloads + handlers), caps::*Ops traits, glue (author surface)
    shared/             # Permanent substrate: styles, colors, assets/fonts, animation, events,
                        #   scheduling, robot registry, introspection, per-primitive handles
    facade/             # The `runtime_core::…` spelling — re-exports glue::* + the macro set
    macros/             # #[component], ui!, jsx!, stylesheet! proc-macros
    layout/             # Taffy-based flex layout helper for native backends
    core/               # The pre-runtime-v2 walker half — being deleted
    reactive/           # arena/ + refs/ allocators used by the shared substrate

  css/                  # StyleRules → CSS, shared by the web and SSR backends

  backend/              # Host implementations (Host + caps::*Ops + a newcore boot entry)
    web/                # WASM + DOM (the reference backend)
    ssr/                # Server-side render / SSG / SSR server
    ios/{core,mobile,tv}        android/{core,mobile,tv}
    macos/  apple/core/         # AppKit + shared objc2 helpers
    terminal/  cpu/  roku/  email/  linux/  windows/
    posix-log-capture/  # Robot log-buffer LogCapture impl

  gpu-backend/          # Custom rendering on a GPU pipeline
    engine/             # render-wgpu
    api/  painter/
    host/               # Per-OS shells (appkit, winit, web, macos-desktop, ios-mobile, …)
    variant/            # phone/ tablet/ tv — pre-wired form factors

  ui/                   # User-facing component libraries
    idea-ui/            # Cross-platform component library (Card, Modal, Popover, Field, …)
    idea-theme/         # Token/theme model idea-ui builds on
    idea-ui-nav/        # Navigation chrome (TabBar, Drawer, StackHeader)
    idea-ui-mail/       # Email-safe component set for backend-email
    icons-lucide/       # Lucide icon pack; tree-shakeable
    idea-ui-docs-derive/# #[derive(DocControls)] proc macro powering the docs site

  sdk/                  # Third-party-style extensions: a payload type + a registered handler
    client/             # maps, webview, canvas, svg, video, markdown, table, codeblock, form,
                        #   navigators, storage, net, i18n, gesture/pan/dnd, media, … (~40 crates)
    server/             # jobs, pubsub, email, config

  api/                  # Full-stack server layer: #[server] fns, server-kit, AWS adapter

  dev/                  # Dev-mode + test infrastructure
    server/ client/ http/ reload/ web-host/ wire/   # hot reload + wire protocol
    scene-parity/ parity-goldens/ mock-backend/     # golden op-sequence parity harness
    robot-e2e/ robot-test/ robot-relay/             # cross-platform robot suites
    newcore-app/ newcore-*-smoke/                   # per-platform runtime smoke apps

  tools/
    cli/                # idealyst CLI: new, dev, build, run, test, doctor, configure, export
    build/  run/        # Per-target build + run orchestration
    lint/ configure/ port/ premint-dump/ wasm-split/ icon-gen/ dev-tui/ docs-app/

  mcp/                  # Stdio MCP server (`idealyst mcp`): component catalog + Robot tools
    catalog/ server/
```

Where a crate has non-obvious wiring, runtime requirements, or behavioural
quirks, it has its own `README.md`. The most useful entry points:

- [`crates/runtime/README.md`](crates/runtime/README.md): how the runtime
  crates fit together — kernel, scene, vocabulary, shared substrate, facade.
- [`crates/runtime/vocabulary/COVERAGE.md`](crates/runtime/vocabulary/COVERAGE.md):
  every capability trait, its methods, and its defaults.
- [`crates/dev/scene-parity/README.md`](crates/dev/scene-parity/README.md):
  the golden op-sequence corpus and the enumerated divergence list.
- [`crates/dev/wire/README.md`](crates/dev/wire/README.md):
  wire protocol shared by hot-reload + server-driven UI.
- [`crates/runtime/layout/README.md`](crates/runtime/layout/README.md):
  how the native backends drive flex layout through Taffy.
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
- [`crates/gpu-backend/README.md`](crates/gpu-backend/README.md):
  the GPU rendering pipeline, host requirements, debug-stats feature.
- [`crates/tools/port/README.md`](crates/tools/port/README.md): source-porter
  design (compiler skeleton + AI hole-filling).

For framework design docs (UI layer, reactivity, styling, animation, fonts,
primitives, backend contract) see **[`docs/`](docs/)**.

## Running the examples

The examples under [`examples/`](examples/) all use the same CLI:

```bash
cd examples/welcome
idealyst dev                # hot-reload web preview at http://localhost:8080
idealyst run ios            # iOS simulator
idealyst run android        # Android emulator / device
idealyst publish ios        # distribution .ipa → App Store Connect (--upload)
```

`idealyst new my-app` is shorthand for "copy `examples/welcome` to `my-app` and
adjust crate names." Once you're in either, the workflow is the same.

Other examples worth knowing about:

- [`examples/fiddle`](examples/fiddle): sandbox for quick framework
  experiments.
- [`websites/idea-ui-docs`](websites/idea-ui-docs): the live docs site for
  the `idea-ui` component library, built with the framework itself.
- [`crates/backend/roku/examples/hello-roku`](crates/backend/roku/examples/hello-roku): minimal Roku target.
- [`crates/mcp/examples/mcp-demo`](crates/mcp/examples/mcp-demo): exercises the framework MCP
  catalog.
- [`websites/tutorial`](websites/tutorial): the interactive tutorial app.
- [`crates/dev/newcore-app`](crates/dev/newcore-app): the runtime proof crate —
  one source tree compiled and e2e-tested on every backend.

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
