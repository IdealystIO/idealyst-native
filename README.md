# idealyst-native

A Rust framework for building one app that runs natively on the web, iOS,
Android, macOS, Windows and Linux.

> [!WARNING]
> **Idealyst is in beta.** Expect frequent breaking changes while we dial in
> the SDK. APIs, crate names and macro syntax can change between releases;
> each breaking release ships a migration note in [`docs/`](docs/) and the
> [CHANGELOG](CHANGELOG.md). Pin exact versions if you build on it today.

## What is Idealyst?

Idealyst takes each platform's functionality (views, text, input, scrolling,
navigation, the camera, secure storage, notifications, and so on) and bridges
it to one abstraction. You write your app against that abstraction once: the
UI tree, styles, state and client-side logic. A **backend** for each platform
turns it into that platform's real native output: DOM elements on the web,
UIKit views on iOS, Android Views, AppKit on macOS.

The abstraction does not pretend the platforms are identical. Each backend
handles its platform's differences and quirks, such as how a scroll view
reports its offset, how the keyboard covers content, safe areas, permission
prompts and text measurement, so the behavior you see is the same everywhere.
When a difference is real and belongs to the app (a `Cmd` shortcut on macOS
and `Ctrl` elsewhere), the app can read which platform it is on and branch.
Everything else stays platform agnostic.

### Think of it as a responsive web app

The most effective way to build an Idealyst app is to design it the way you
would design a responsive web app:

- Lay it out with **flexbox** (and CSS grid). The web backend uses the
  browser's layout engine; native backends run the same model through
  [Taffy](https://github.com/DioxusLabs/taffy).
- Style **mobile first**: write the narrow layout in a stylesheet's `base`
  block, then widen it with `breakpoint` blocks (`sm`, `md`, `lg`, `xl`).
- Pull colors, spacing and radii from **style tokens** so the whole app can be
  restyled, or switched to dark mode, at runtime.
- Keep state in **signals** and let the UI react to them.

Build it that way and the same code works in a phone, a tablet, a desktop
window and a browser tab.

## Core concepts

Everything below is imported from `runtime_core`, the author-facing crate.

### Signals

A signal holds a piece of state. Reading it inside the UI subscribes that
part of the UI, and writing it updates only what read it.

```rust
use runtime_core::signal;

let count = signal(0);

count.set(5);
count.update(|n| n + 1);    // update takes &T and returns the new value
let now = count.get();      // 6
```

Writes are batched: everything an event handler, timer or async completion
writes commits together in one update pass. See
[`docs/reactivity.md`](docs/reactivity.md) and
[`docs/automatic-batching.md`](docs/automatic-batching.md).

### Memos

A memo is a derived value. It computes once, is shared by every reader, and
only notifies readers when its result actually changes.

```rust
use runtime_core::{memo, signal};

let items = signal(vec![3, 8, 12]);
let total = memo(move || items.get().iter().sum::<i32>());
let over_budget = memo(move || total.get() > 20);
```

For a value read in a single place, a plain closure (`move || count.get() * 2`)
is enough. Reach for `memo` when a derivation is shared or expensive.

### Components

A component is a function marked `#[component]`. Its parameters become props,
and its body returns a tree written with the `ui!` macro. Built-in primitives
are lowercase (`view`, `text`, `button`, …); your components are PascalCase.

```rust
use runtime_core::{component, memo, signal, ui, Element};

#[component]
pub fn Counter(label: String, #[prop(default = 1)] step: i32) -> Element {
    let count = signal(0);
    let doubled = memo(move || count.get() * 2);

    ui! {
        view(style = Card().tone(CardTone::Accent)) {
            text { "{label}: {count} (doubled: {doubled})" }
            button(
                label = "Increment",
                on_click = move || count.update(|n| n + step.get()),
            )
            if count.get() >= 10 {
                text { "That's a lot." }
            }
        }
    }
}

// Used like a primitive:
// ui! { Counter(label = "Clicks".to_string(), step = 2) }
```

`{name}` inside a text string inserts a value; when the value is a signal or
memo, that text updates live. `if`, `match` and `for` work directly inside
`ui!`. The full authoring guide is [`docs/ui-layer.md`](docs/ui-layer.md).

### Style tokens

A style token is a named style value, such as a color, a length or a number,
that stylesheets read by name. Changing a token's value restyles only the
elements that use it, without recomputing any stylesheet. A **theme** is one
complete set of token values.

Declare your tokens as a plain struct; the struct is the vocabulary and each
value of it is a theme:

```rust
use runtime_core::{Color, Length, Tokens};

#[derive(Tokens, Clone)]
pub struct Swatch { pub bg: Color, pub text: Color }

#[derive(Tokens, Clone)]
#[tokens(default = light)]
pub struct AppTheme {
    pub surface: Color,
    pub text: Color,
    pub accent: Swatch,
    pub space: Length,
    pub radius: Length,
}

fn light() -> AppTheme {
    AppTheme {
        surface: "#ffffff".into(),
        text: "#111827".into(),
        accent: Swatch { bg: "#eef2ff".into(), text: "#4338ca".into() },
        space: 12.0.into(),
        radius: 8.0.into(),
    }
}

fn dark() -> AppTheme {
    light().with(|t| {
        t.surface = "#111827".into();
        t.text = "#e5e7eb".into();
        t.accent.bg = "#1e1b4b".into();
    })
}

// At startup, inside your app:
AppTheme::follow_system(light(), dark());   // match the OS light/dark setting
// Later, from anywhere (a settings toggle, say):
AppTheme::set(dark());
```

### Stylesheets

`stylesheet!` declares a reusable style with variants, interaction states and
responsive breakpoints. `<AppTheme>` names the token vocabulary, and `t`
gives typed access to it, so a misspelled token fails the build.

```rust
use runtime_core::{stylesheet, FlexDirection};

stylesheet! {
    pub Card<AppTheme> {
        base(t) {
            flex_direction: FlexDirection::Column,   // mobile first
            padding: t.space(),
            border_radius: t.radius(),
            background: t.surface(),
            color: t.text(),
        }
        variant tone {
            #[default]
            plain(_t) {}
            accent(t) {
                background: t.accent.bg(),
                color: t.accent.text(),
            }
        }
        state hovered(t) {
            border_color: t.accent.text(),
        }
        breakpoint md(_t) {
            flex_direction: FlexDirection::Row,      // ≥ 768 dp: lay out in a row
        }
    }
}
```

The macro generates the `Card()` builder and a `CardTone` enum, used above as
`style = Card().tone(CardTone::Accent)`. A variant can also take a signal or a
closure, and the style updates when it changes. On the web, stylesheets
compile to CSS classes, and breakpoints and hover states to `@media` and
`:hover` rules. Native backends apply the same rules directly. See
[`docs/styling.md`](docs/styling.md).

### Apps and hosts

An app is one crate that knows nothing about the platform it runs on. A small
**host** crate per target wires that app to a backend and a mount point; the
host is the only code that knows which platform it is on. `idealyst new`
scaffolds both, so most apps never touch a host directly.

## SDKs

The core framework stays small: primitives, layout, styling, reactivity.
Everything else ships as an opt-in SDK crate that an app adds when it needs
it. Each SDK exposes one API and implements it per platform. Its README lists
which platforms it supports and how far each one has been tested. The full
index is [`crates/sdk/README.md`](crates/sdk/README.md).

Some of these SDKs will likely move out of this repository into their own
over time. The goal is to keep this repository to the base functionality that
most apps need, with more specialized SDKs maintained separately.

**UI and rendering**
- `canvas`: retained-mode 2D drawing (GPU via Vello where available, CPU elsewhere)
- `canvas3d`: 3D scenes (glTF models, lights, camera)
- `charts`: reactive charts drawn on a canvas, with native text and tooltips
- `svg`: vector images
- `video`: video playback
- `maps`: a native map view
- `webview`: embedded web content
- `markdown`: CommonMark / GFM rendering
- `codeblock`: syntax-highlighted code
- `pdf`: render PDF pages
- `table`: tables (a real `<table>` on web)
- `form`: a form container (Enter-to-submit and autofill on web)
- `virtualized`: virtualized lists, grids and responsive grids
- `toolbar`: window toolbar (`NSToolbar` on macOS)
- `menu`: the OS menu bar
- `qr`: show, export and scan QR codes

**Navigation**
- `stack-navigator`: push / pop stacks with a typed handle
- `swap-navigator`: flat sets of screens, for tab bars and drawers

**Gestures and input**
- `pan`, `zoom`: drag and pinch / scroll zoom, the same on mouse, trackpad and touch
- `gesture`: arbitration between several recognizers on one view
- `dnd`: in-app drag and drop (reorderable lists, boards)

**Data and networking**
- `net`: HTTP over each platform's native stack
- `graphql`: typed GraphQL client and server execution
- `sync`: offline-first cache that synchronizes with a server
- `storage`: key-value preferences and cache
- `credentials`: secure storage for secrets (Keychain, Keystore, …)
- `files`: app-private file storage
- `file-picker`, `file-export`: the system open and save dialogs
- `i18n`: typed translations checked at compile time

**Device and OS**
- `permissions`: one request / status API for runtime permissions
- `notifications`: local and scheduled notifications, push tokens
- `location`: one-shot and continuous geolocation
- `biometrics`: Face ID / Touch ID / fingerprint / Windows Hello
- `camera`, `microphone`, `screen-recorder`: live capture
- `clipboard`, `share`, `haptics`, `deep-link`, `connectivity`
- `datetime`: current time and local time zone on every target
- `random`: secure randomness and a seeded generator that matches across platforms
- `auto-update`: self-update for directly distributed desktop apps
- `offload`: run CPU-heavy work off the main thread

**Audio and video**
- `audio`: sound playback
- `audio-engine`: low-latency, mixable sound for games
- `synth`: procedural sound effects
- `denoise`: neural noise suppression for live audio
- `media-stream`: the shared live audio / video stream type
- `media-writer`: record streams to a file
- `video-decode`: decode video files into streams
- `video-compose`: real-time GPU compositing on video streams

**Server** (runs in the server binary, never in the client)
- `jobs`: background jobs and queues with retries
- `pubsub`: publish / subscribe, including WebSocket fan-out across instances
- `email`: transactional email rendered from Idealyst templates
- `cache`: key-value cache with TTL (memory or Redis)
- `idealyst-config`: shared configuration and connection profiles for the server SDKs

The full-stack layer (`#[server]` functions, the server kit and the AWS
adapter) lives under [`crates/api/`](crates/api/); see
[`docs/server-functions.md`](docs/server-functions.md).

Building your own SDK uses the same mechanism the built-in ones use: define a
payload type and register a handler per backend on the scene `Registry`. See
[`docs/registry.md`](docs/registry.md).

## Platform support

| Platform | Backend | Status |
| --- | --- | --- |
| Web | WASM + DOM; client rendering, SSR / SSG and hydration | Supported |
| iOS | UIKit | Supported |
| Android | Android Views over JNI | Supported |
| macOS | AppKit | Supported |
| Windows | Win32 | **Work in progress** |
| Linux | GTK4 | **Work in progress** |
| Server HTML | SSR / SSG and email-safe HTML | Supported |

Experimental targets: a custom GPU renderer on [wgpu](https://wgpu.rs/) that
draws its own pixels, a terminal UI, a CPU software rasterizer, and Roku. The
tvOS and Android TV backends are empty skeletons.

### Per-backend primitive coverage

Every backend implements every capability trait; a blank cell means that
backend leaves the primitive's `create_*` method on the trait default. Those
defaults degrade rather than crash: a missing widget renders the
`ExternalOps::missing_primitive_placeholder` box, and a missing container
lowers to a plain view. What *does* panic is an unregistered third-party
payload (see [SDKs](#sdks)).

| Primitive | web | iOS | Android | macOS | Linux | Windows | Roku | terminal | CPU | SSR | wgpu |
|---|---|---|---|---|---|---|---|---|---|---|---|
| view / text / button | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| image / icon / text_input / toggle / activity_indicator | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| scroll_view | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| text area (multiline text_input) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |  |  | ✓ | ✓ | ✓ |
| slider | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |  | ✓ | ✓ | ✓ |
| graphics | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |  | ✓ | ✓ | ✓ |
| link | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |  | ✓ |  | ✓ | ✓ |
| virtualizer / flat_list | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |  |  | ✓ | ✓ | ✓ |
| virtual grid | ✓ | ✓ | ✓ | ✓ |  |  |  |  |  |  |  |
| overlay / portal | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

Third-party primitives (an SDK payload type plus a handler registered on the
scene `Registry`) are covered per SDK, per backend: some SDKs lower to builtin
primitives on every host, others ship a handler per platform (`video`, `svg`, `codeblock`, `toolbar`, `webview`, …) and
fall back to the placeholder box where no handler exists. Each SDK's README
lists the hosts it supports.

## Getting started

The `idealyst` CLI is the entry point for everything user-facing: scaffolding
new projects, building / running them for web / iOS / Android, the hot-reload
dev server, and the doctor command for diagnosing your toolchain. It's
published to the framework's registry, `crates.idealyst.io`, and built from
source by `cargo install`; there are no pre-built binaries yet.

### Prerequisites

- **Rust** stable toolchain (1.78+ recommended). Install via
  [rustup](https://rustup.rs/) if you don't already have it.

Per-platform tooling (Xcode for iOS, Android NDK for Android, the
`wasm32-unknown-unknown` rustup target for web) is only needed when you actually `build` / `run` for that target.
The CLI itself has no platform dependencies. `idealyst doctor` will tell you
what each enabled target is missing.

### Install

```bash
cargo install idealyst-cli --index sparse+https://crates.idealyst.io/index/ --locked
```

That builds the newest release in release mode and drops the `idealyst` binary
into `~/.cargo/bin/` (which is on your `PATH` if you set Rust up through
`rustup`). `--index` names the registry directly, so no `.cargo/config.toml`
or environment variable is needed; `--locked` builds the exact dependency
graph the release was checked against. Add `--version <x.y.z>` to install a
particular release.

### Update

```bash
idealyst update          # rebuild the newest release and replace this binary
idealyst update --check  # only report whether a newer release exists
```

`update` reads the registry's index, and when a newer release is published it
runs the same `cargo install` into the directory the running binary was
installed to. It keeps a build cache in `~/.idealyst/cli-target`, so later
updates rebuild only what changed. `--version <x.y.z>` installs a specific
release, older ones included.

### Unreleased commits

To run a commit that hasn't been released yet, install from git. Inside a git
checkout the framework's crates name their registry only as `"idealyst"`, so
cargo needs it defined in the environment:

```bash
export CARGO_REGISTRIES_IDEALYST_INDEX=sparse+https://crates.idealyst.io/index/
cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli
cargo install --git https://github.com/IdealystIO/idealyst-native --rev <sha> idealyst-cli
```

Without the variable cargo reports the CLI itself as missing (`could not find
\`idealyst-cli\` in … with version \`*\``) rather than naming the registry. A
later `idealyst update` moves a git install back onto releases once a newer
one is published.

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

## Built-in automation and agent control

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
   ┌──────┬──────┬──────┬──────┬──────┬──────┬──────┬──────┬──────┐
   ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼
  web    iOS  Android macOS  Linux Windows Roku  headless  wgpu  (yours)
  DOM   UIKit JNI/View AppKit GTK4  Win32  BrS  SSR/email/  GPU
                                                term/CPU
```

The framework controls **what** to render and **when** to update. The backend
controls **how** that happens on the target platform. Because that boundary is
small, supporting a new platform means writing a new backend. You don't have
to change the framework itself.

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
    core/               # runtime-core: the `runtime_core::…` author surface — re-exports glue + the macro set
    world/              # Reactive kernel: per-world arenas, Copy handles, staged-commit flush
    scene/              # Element, Host, Registry, realize + structural drivers
    vocabulary/         # Primitives (payloads + handlers), caps::*Ops traits, glue (author surface)
    shared/             # Substrate: styles, colors, assets/fonts, animation, events,
                        #   scheduling, robot registry, introspection, per-primitive handles
    macros/             # #[component], ui!, jsx!, stylesheet! proc-macros
    macros-parse/ macros-expand/   # ui! front end (shared with the CLI) + expansion library
    template/           # A ui! site as data: descriptors + patch validation (hot reload)
    layout/             # Taffy-based flex/grid layout helper for native backends
    web-glue/           # Framework-owned wasm ⇄ JS boundary
    zone-offset/        # Local UTC offset from the OS zone database

  css/                  # StyleRules → CSS, shared by the web and SSR backends

  backend/              # Host implementations (Host + caps::*Ops in each crate's newcore.rs)
    web/                # WASM + DOM (the reference backend)
    ssr/  email/        # Server-side render / SSG / SSR server; email-safe HTML
    ios/{core,mobile,tv,rs-shell}   android/{core,mobile,tv}
    macos/  apple/core/ # AppKit + shared objc2 helpers
    linux/  windows/    # GTK4, Win32
    terminal/  cpu/  roku/
    image-source/       # Shared image-source decoding helpers

  host/                 # Native desktop shells: gtk/ (Linux), win32/ (Windows)

  gpu-backend/          # Custom rendering on a GPU pipeline
    engine/             # render-wgpu
    api/                # Shared engine API types
    painter/            # android-sim/ ios-sim/ — per-look primitive geometry
    host/               # Per-OS shells (appkit, winit, web, macos-desktop, ios-mobile, wgpu-accesskit, …)
    variant/            # phone/ tablet/ tv — pre-wired form factors

  ui/                   # User-facing component libraries
    idea-ui/            # Cross-platform component library (Card, Modal, Popover, Field, …)
    idea-theme/         # Token/theme model idea-ui builds on
    idea-theme-editor/  # Live theme editor
    idea-ui-nav/        # Navigation chrome (TabBar, Drawer, StackHeader)
    idea-ui-mail/       # Email-safe component set for backend-email
    icons-lucide/       # Lucide icon pack; tree-shakeable
    idea-ui-docs-derive/# #[derive(DocControls)] proc macro powering the docs site

  sdk/                  # Third-party-style extensions: a payload type + a registered handler
    client/             # maps, webview, canvas, svg, video, markdown, table, codeblock, form,
                        #   navigators, storage, net, i18n, gesture/pan/dnd, media, …
    server/             # jobs, pubsub, email, cache, config

  api/                  # Full-stack server layer: #[server] fns, server-kit, AWS adapter

  streaming/            # Remote components: wasm bundles run on-device (wasmi)
  ota/                  # Over-the-air delivery of remote components

  idealyst/  idealyst-macros/   # Build and run an app with plain `cargo build`

  dev/                  # Dev-mode + test infrastructure
    server/ client/ http/ reload/ web-host/ wire/ hot/   # hot reload + wire protocol
    scene-parity/ parity-goldens/ mock-backend/ host-mock/  # golden op-sequence parity harness
    robot-e2e/ robot-test/ robot-relay/             # cross-platform robot suites
    inspector-server/ inspector-protocol/           # `idealyst inspect`
    newcore-app/ newcore-*-smoke/                   # per-platform runtime smoke apps

  tools/
    cli/                # idealyst CLI: new, dev, build, run, test, doctor, configure, export
    build/  run/        # Per-target build + run orchestration
    lint/ configure/ port/ premint-dump/ wasm-split/ icon-gen/ dev-tui/ docs-app/ js-min/ registry/

  mcp/                  # Stdio MCP server (`idealyst mcp`): component catalog + Robot tools
    catalog/ catalog-scan/ server/
```

Where a crate has non-obvious wiring, runtime requirements, or behavioural
quirks, it has its own `README.md`. The most useful entry points:

- [`crates/runtime/README.md`](crates/runtime/README.md): how the runtime
  crates fit together — kernel, scene, vocabulary, shared substrate, and the
  `runtime-core` author surface.
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
  bootstrap and the AppKit-vs-UIKit differences the backend works around.
- [`crates/backend/roku/README.md`](crates/backend/roku/README.md):
  how token updates reach a generator backend, and its caveats.
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

## Project goals

- **One app, every platform.** Write the UI, styles, state and logic once and
  ship it to the web, phones, tablets and desktops, with the same look and
  behavior on each.
- **Native output and near-native performance.** Each platform renders with
  its own toolkit, driven by compiled Rust rather than a JavaScript runtime.
- **A standard for the whole app, not just components.** Theming, navigation,
  animation, accessibility, device features and the server side all share
  one consistent API.
- **A small core that is easy to extend.** The framework provides the base
  primitives. Everything else is built as an SDK on the same extension point
  that third parties use, so you can add what your app needs without forking.
- **Built for testing and automation.** Every app can be inspected and driven
  the same way on every platform, by tests, dev tools or AI agents.

## Special Thanks

Dioxus is another really cool initiative creating a Rust based cross platform development framework. I even use one of their tools (Taffy!) as the flex-layout renderer for the ios and android backends. Idealyst's approach is unique from theirs in how we render the applications, but I certainly was inspired by their work. Please check them out and support their development! https://github.com/DioxusLabs/dioxus

Special thanks to @GelScott for dealing with my insane rambles as I designed and implemented the framework.

## License

MIT.
