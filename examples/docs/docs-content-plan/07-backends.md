# Backends

A backend is the piece of code that knows how to put an Idealyst
app on a specific platform's screen. The framework hands it a tree
of `Primitive`s and a stream of updates; the backend translates
those into native widgets, layout, and input events.

The `Backend` trait is the seam. Anything that implements it can
run an app. This page is the **map** of the backends Idealyst
ships — what each one targets, what makes it interesting, what
it can and can't do. For the trait's full surface and what writing
one looks like, see [Writing your own backend](#).

## The shipped backends

Five backends come in the box:

| Backend | Target | Status |
| --- | --- | --- |
| `backend-web` | Browser (WASM + DOM) | Production-ready |
| `backend-ios` | iOS / iPadOS / tvOS (UIKit) | Production-ready |
| `backend-android` | Android phones / tablets / TV (Views) | Production-ready |
| `backend-roku` | Roku devices (SceneGraph + BrightScript) | Experimental |
| `backend-aas-shell-native` | Dev-mode app-as-server client | Dev only |

Each one lives in `crates/backend/<name>` and gets pulled in by the
CLI when you target the matching platform.

## Web

`backend-web` drives the DOM via `web-sys` and `wasm-bindgen`. Your
app compiles to WebAssembly, the backend creates DOM elements as
the walker visits primitives, and the browser handles layout,
input, and rendering.

A few things worth knowing:

- **Layout is the browser's.** No Taffy here — the backend just
  sets CSS properties and lets the browser do the work. Flex
  layout maps cleanly because the framework's flex model is a
  subset of CSS flexbox.
- **Stylesheets become CSS classes.** Each `(stylesheet, variants)`
  combination mints one class lazily; `apply_style` just sets
  `className`. Theme swaps are CSS variable writes — see
  [Styles](#) for the full mechanism.
- **The dev pipeline uses `wasm-pack`.** `idealyst dev --web`
  watches your source, rebuilds the WASM module, and serves it
  from a local HTTP server. Live updates apply via hot reload —
  not full page reloads.

## iOS

`backend-ios` renders to UIKit via the `objc2` crate. The backend
is split across three sub-crates:

- **`backend-ios-core`** — the shared substrate. Style application,
  color/flex conversion, and the `NSTimer`-based render loop
  driver.
- **`backend-ios-mobile`** — touch input semantics for iPhone and
  iPad. Gesture recognizers, focus on first responder,
  hardware-keyboard handling.
- **`backend-ios-tv`** — Apple TV / tvOS focus-engine semantics.
  D-pad navigation, focus visualizers, the focus-on-press model.

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
- **The build pipeline uses `xcodebuild`.** `idealyst dev --ios`
  generates a small Xcode wrapper project in a build cache,
  invokes `cargo build` for the static library, then
  `xcodebuild` to launch on a simulator.

## Android

`backend-android` mirrors the iOS split: a shared core, plus a
mobile leaf and a TV leaf.

- **`backend-android-core`** — JNI helpers, the render-thread
  `RenderLoopDriver`, and the shared View hierarchy primitives.
- **`backend-android-mobile`** — touch input for phones and
  tablets.
- **`backend-android-tv`** — Leanback / D-pad focus model for
  Android TV.

Notes:

- **Layout is Taffy** (same as iOS). The Android `View` system has
  its own layout machinery, but the framework drives layout
  itself and pushes computed frames into the View hierarchy.
- **The build pipeline uses Gradle.** `idealyst dev --android`
  generates a Gradle wrapper around your Rust `cdylib`, then
  builds and installs to an emulator or attached device.

## Roku

`backend-roku` is the experimental one. Roku devices don't run
Rust — the only language the device runtime supports is
BrightScript, plus the SceneGraph XML markup format.

The backend works by **emitting a wire stream of commands** that a
small BrightScript thin client running on the device replays
against SceneGraph nodes. Every primitive create / update / event
goes over the network.

Implications:

- **Performance is bounded by the network round-trip.** This
  backend isn't viable for shipping consumer apps. It's primarily
  a demonstration that the framework's seams can target a
  platform with no native Rust runtime.
- **Closures don't ship.** Reactive expressions have to round-trip
  through the host. The `Derived<T>` and `Action` types
  (mentioned in [Reactivity](#)) carry a structured form
  specifically for this case — generator backends can serialize
  the reactive intent without serializing a closure.
- **Some primitives have no Roku analog.** GPU `Graphics` are not
  supported; certain layout features are approximated.

Read `backend-roku`'s source comments for the exact constraints if
you're curious. For now: cool to see, not what you'd build a
production TV app on.

## AAS — the dev-mode backend

`backend-aas-shell-native` is the **app-as-server** client. It's
unusual because it doesn't render anything itself — it forwards
backend operations to whatever real backend is on the other end.

The shape:

```
┌────────────────────┐  WebSocket  ┌────────────────────┐
│  AAS dev-host      │ ──────────► │  AAS shell client  │
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

AAS is its own concept worth a page — see [Dev tools](#) for the
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
idealyst dev --web       # only the web backend
idealyst dev --ios       # only the iOS backend
idealyst dev --aas       # AAS dev-host with whatever clients connect
```

## Writing your own backend

The shipped backends cover the major platforms, but there's no
reason to stop there. The `Backend` trait is small (~30
methods), and a working backend lives in one Rust crate that
depends only on `framework-core` and whatever native bindings it
needs.

Things people could plug in:

- A terminal renderer for command-line apps
- A custom GPU renderer with `wgpu` (Idealyst already exposes the
  `Graphics` primitive — a backend can take it further and render
  the whole tree on GPU)
- An embedded display driver (e-paper, OLED)
- A server-side renderer that emits HTML for SSR
- A backend for a platform the framework doesn't ship yet (macOS,
  Windows, Linux desktop via winit, KaiOS, anything)

The dedicated [Writing your own backend](#) page walks through the
trait's full surface, the lifecycle of a backend node, the
relationship between the walker and `Backend` methods, and what
gets called when. This page is intentionally just the map.

## Where to read more

- [Writing your own backend](#) — the full trait, lifecycle, and a
  worked example.
- [Dev tools](#) — AAS in depth, the wire protocol, hot reload.
- [Architecture in more depth](#) (on the Overview) — where
  backends sit relative to `framework-wire`, `framework-hot`, and
  `framework-native-layout`.
