# Backends

A backend is the piece of code that knows how to put an Idealyst
app on a specific platform's screen. The framework hands it a tree
of `Element`s and a stream of updates; the backend translates
those into native widgets, layout, and input events.

The `Host` trait plus the capability traits are the seam. Anything that
implements them can run an app. This page is the **map** of the backends Idealyst
ships — what each one targets, what makes it interesting, what
it can and can't do. For the trait's full surface and what writing
one looks like, see [Writing your own backend](writing-a-backend).

## The shipped backends

These backends come in the box:

| Backend | Target |
| --- | --- |
| `backend-web` | Browser (WASM + DOM) |
| `backend-ios-mobile` | iOS / iPadOS (UIKit) |
| `backend-android-mobile` | Android phones / tablets (Views) |
| `backend-macos` | macOS (AppKit) |
| `backend-linux` | Linux (GTK4) |
| `backend-windows` | Windows (Win32) |
| `backend-roku` | Roku devices (SceneGraph + BrightScript) |
| `backend-ssr` | Server-side rendering + static generation (HTML) |
| `backend-email` | Email-safe HTML |
| `backend-terminal` | A terminal character grid |
| `backend-cpu` | A pixel framebuffer (software rasterizer) |
| `render-wgpu` | A GPU renderer that paints its own pixels |

Each one lives under `crates/backend/` (the GPU renderer under
`crates/gpu-backend/`) and gets pulled in by the
CLI when you target the matching platform. `backend-ios-tv` and
`backend-android-tv` exist as empty skeletons; tvOS and Android TV
are not supported.

Dev mode adds one more piece: `runtime-server-shell-native`, the thin
client that replays a dev session on a device. See runtime-server
below.

## Web

`backend-web` drives the DOM via `web-glue`, the framework's own JS
bindings (no `web-sys` / `wasm-bindgen`). Your
app compiles to WebAssembly, the backend creates DOM elements as
the scene mounts primitives, and the browser handles layout,
input, and rendering.

A few things worth knowing:

- **Layout is the browser's.** No Taffy here — the backend just
  sets CSS properties and lets the browser do the work. Flex
  layout maps cleanly because the framework's flex model is a
  subset of CSS flexbox.
- **Stylesheets become CSS classes.** Each `(stylesheet, variants)`
  combination mints one class lazily; `apply_style` just sets
  `className`. Theme swaps are CSS variable writes — see
  [Styles](styles) for the full mechanism.
- **The CLI builds and packages the module itself.** It runs
  `cargo build` on the app crate for `wasm32-unknown-unknown` and
  writes the JS glue; the `wasm-bindgen` CLI runs only for apps that
  link wgpu or their own `web-sys`. `idealyst dev --web` watches
  your source and serves the app from a local HTTP server. Most
  saves apply in place, without a page reload.

## iOS

The iOS backend renders to UIKit via the `objc2` crate. It is split
across three sub-crates:

- **`backend-ios-core`** — the shared substrate. Style application,
  color/flex conversion, and the `NSTimer`-based render loop
  driver.
- **`backend-ios-mobile`** — the iPhone / iPad backend: raw touch
  delivery (gestures are recognized in shared Rust, not by
  `UIGestureRecognizer`), focus on first responder,
  hardware-keyboard handling.
- **`backend-ios-tv`** — reserved for Apple TV / tvOS focus-engine
  semantics; currently an empty skeleton.

The split exists because the input model genuinely differs:
touch-based UIs and focus-engine-based UIs need different
gesture-recognizer plumbing and different selection chrome. They
share everything *below* input — the same flex layout, the same
style application, the same primitive vocabulary.

A few specifics:

- **Layout is Taffy.** The backend hands flex constraints to
  [Taffy](https://github.com/DioxusLabs/taffy) and applies the
  resulting frames to `UIView` instances. Same engine the Dioxus
  ecosystem uses; one of the few external dependencies Idealyst
  pulls in.
- **Safe-area insets propagate reactively.** The native safe-area
  changes (rotation, dynamic island, software keyboard) drive a
  framework signal; primitives that opt into safe-area padding
  re-apply automatically.
- **No Xcode project for the simulator.** The CLI generates a
  small wrapper crate in a build cache, runs `cargo build` for the
  static library, links it with `swiftc`, assembles the `.app`, and
  launches it with `simctl`. Device builds (`--device`) generate an
  Xcode project so `xcodebuild` can sign them.

## Android

The Android backend mirrors the iOS split: a shared core, plus a
mobile leaf and a TV leaf.

- **`backend-android-core`** — JNI helpers, the render-thread
  `RenderLoopDriver`, and the shared View hierarchy primitives.
- **`backend-android-mobile`** — touch input for phones and
  tablets.
- **`backend-android-tv`** — reserved for the Leanback / D-pad focus
  model on Android TV; currently an empty skeleton.

Notes:

- **Layout is Taffy** (same as iOS). The Android `View` system has
  its own layout machinery, but the framework drives layout
  itself and pushes computed frames into the View hierarchy.
- **No Gradle.** The CLI generates a wrapper crate that builds
  your app as a `cdylib` (`.so`), then runs the Android SDK tools
  directly (`javac`, `d8`, `aapt2`, `apksigner`) and installs the APK
  with `adb`.

## Roku

`backend-roku` is the experimental one. Roku devices don't run
Rust — the only language the device runtime supports is
BrightScript, plus the SceneGraph XML markup format.

The backend is a **command emitter**: every capability call becomes
a JSON `RokuCommand` in a queue. `idealyst build --roku` runs your
`app()` once in a generated wrapper binary, snapshots the command
stream, and lays out a side-loadable package in `dist/roku/` (plus
`dist/roku.zip`): the snapshot, a small BrightScript runtime that
replays it against SceneGraph nodes, and your `#[method]` functions
transpiled to BrightScript.

Implications:

- **Button events are not yet sent back from the device, and there
  is no `idealyst dev` loop for Roku.** This backend isn't viable for
  shipping consumer apps. It's primarily
  a demonstration that the framework's seams can target a
  platform with no native Rust runtime.
- **Closures don't ship.** Reactive expressions have to round-trip
  through the host. The `Derived<T>` and `Action` types
  (mentioned in [Reactivity](reactivity)) carry a structured form
  specifically for this case — generator backends can serialize
  the reactive intent without serializing a closure.
- **Some primitives have no Roku analog.** GPU `graphics` surfaces are not
  supported; certain layout features are approximated.

Read `backend-roku`'s source comments for the exact constraints if
you're curious. For now: cool to see, not what you'd build a
production TV app on.

## runtime-server — the dev-mode backend

`runtime-server-shell-native` is the **app-as-server** client. It's
unusual because it doesn't render anything itself — it forwards
backend operations to whatever real backend is on the other end.

The shape:

```
┌────────────────────┐  WebSocket  ┌────────────────────┐
│  runtime-server dev-host      │ ──────────► │  runtime-server shell client  │
│  (your app, on     │             │  (browser / phone, │
│   the dev machine) │ ◄────────── │   thin client)     │
└────────────────────┘             └────────────────────┘
   Runs the reactive                  Receives wire
   runtime; emits wire                commands and
   commands to clients                renders them
```

Why have this? Because it gives you one running instance of your
app's reactive runtime, with arbitrary platforms connecting as
thin clients. Edit code on the dev machine, every connected
client updates. Navigate on one client, the navigation state
syncs to the others.

runtime-server is its own concept worth a page — see [Dev tools](dev-tools) for the
full story, including how the wire protocol works and what
"app-as-server" actually buys you in day-to-day development.

## Picking a backend

You pick a backend by picking a platform target. Inside
`Cargo.toml`:

```toml
[package.metadata.idealyst.app]
targets = ["web", "ios", "android", "roku"]
```

…and the CLI selects the matching backend per target when you
build. There's no code change to switch.

For one-off runs:

```bash
idealyst dev --web           # only the web backend
idealyst dev --ios           # only the iOS backend
idealyst dev --web --local   # build and run the app in the browser itself
```

`idealyst dev` runs in runtime-server mode by default: one sidecar
process runs your app and each platform connects as a thin client.
`--local` builds and runs the app on each platform instead.

## Writing your own backend

The shipped backends cover the major platforms, but there's no
reason to stop there. `Host` is a handful of tree operations, every
capability-trait method but a few has a working default, and a working
backend lives in one Rust crate that depends on `runtime-scene`,
`runtime-vocabulary`, `runtime-shared`, and whatever native bindings it
needs (not on `runtime-core`, the author surface).

Things people could plug in:

- An embedded display driver (e-paper, OLED)
- A game-engine or custom-GPU renderer
- A backend for a platform the framework doesn't ship yet (KaiOS, a
  console, anything)

The dedicated [Writing your own backend](writing-a-backend) page walks through the
`Host` trait and the capability traits, the lifecycle of a backend
node, and what gets called when. This page is intentionally just the map.

## Where to read more

- [Writing your own backend](writing-a-backend) — `Host`, the capability traits,
  lifecycle, and a worked example.
- [Dev tools](dev-tools) — runtime-server in depth, the wire protocol, hot reload.
- [Architecture in more depth](overview) (on the Overview) — where
  backends sit relative to `wire`, `dev-hot`, and
  `runtime-layout`.
