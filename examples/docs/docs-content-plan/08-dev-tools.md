# Dev tools

This page covers the moving parts behind day-to-day development:
how the CLI turns your one-crate app into running platform builds,
how hot reload patches a running app without losing its state, what
**app-as-server** (AAS) is and when to use it, and how the MCP
server hooks Robot up to an external tool like Claude Desktop.

Most of what's here lives below the surface of the user-facing
commands. You can ship apps without reading any of it. But if you
hit a weird build issue, want to script the dev loop, or want to
extend the tooling, this is the layer to know about.

## How an app gets built

The headline rule: **you author one crate. Everything else is
generated.**

Your repo looks like this:

```
my-app/
  Cargo.toml          # crate manifest + Idealyst metadata
  src/
    lib.rs            # exports `pub fn app() -> Primitive`
```

There's no `ios/`, no `android/`, no `web/`. The per-platform host
crates that turn `app()` into a runnable platform artifact are
**generated on demand by the CLI** into a build cache:

```
target/
  idealyst/
    web/              # wasm-bindgen entry + pkg/ + index.html
    ios/              # Xcode project + static lib
    android/          # Gradle project + cdylib
    roku/             # Channel package layout
    aas/              # Dev-host binary
```

Each platform's directory holds whatever that platform expects.
Each one depends on your app crate as a library, calls its
`app()` function from the right entry point (a WASM `start()`, a
JNI `attach()`, an `@UIApplicationMain`, a Roku manifest), and
hands it to the matching backend.

You don't author these files. You don't even *see* them unless
you go looking. The CLI regenerates them as needed; their
contents change with the framework, not with your app.

### Why this shape

The build cache approach trades visible boilerplate for one
moving part: a project's "platform support" is a list of strings
in `Cargo.toml`, not a folder structure you have to maintain.

```toml
[package.metadata.idealyst.app]
name = "my-app"
bundle_id = "com.example.my-app"
targets = ["web", "ios", "android", "roku"]
```

Adding iOS to a project that didn't have it is editing this list
and running `idealyst dev --ios`. Removing it is deleting the
string. No host crate to add or remove, no `Cargo.toml` to merge,
no platform-specific scaffolding to keep up to date as the
framework evolves.

### Per-platform tools

Each backend has its own native build flow. The CLI invokes them:

- **Web** — `wasm-pack build --target web`, then serves the
  output from a local HTTP server.
- **iOS** — `cargo build` for the static lib, then `xcodebuild`
  on the generated Xcode project, then a simulator launch.
- **Android** — `cargo build` for the `cdylib`, then `gradle
  assembleDebug`, then `adb install` to an emulator or device.
- **Roku** — Rust build for the host process, BrightScript
  packaging for the device side, push over the developer-mode
  HTTP interface.
- **AAS** — `cargo build` for the dev-host binary, then run.

You can run any of these by hand if you need to debug the
pipeline. The CLI is orchestration; the underlying tools are
unchanged.

### Exporting a generated project

`idealyst scaffold <platform>` (planned — see [Getting
Started](#) for current status) materializes the generated host
crate for a platform out of the build cache and into your repo.
Once exported, the CLI builds *from* your hand-editable source
instead of regenerating. The escape hatch when you need to do
something the default scaffold doesn't.

## Hot reload

When you save a file with `idealyst dev` running, the CLI rebuilds
the affected crate, hands the new code to the running app, and
the UI updates in place. State doesn't reset. Scroll position
doesn't reset. Navigation state doesn't reset. The user's typed
text in a `TextInput` is still there.

This works because of two pieces working together: a jump-table
function dispatcher and a primitive-tree diff.

### The function dispatcher

`#[component]` splits every component function into two parts at
compile time:

- The **outer function** keeps the public name (`counter`). Its
  body is `framework_hot::call(__counter_hot_impl, args)` — a
  dispatch through a runtime jump table.
- The **inner function** (`__counter_hot_impl`) holds the real
  body.

When you change a component's source and the rebuild produces a
new `__counter_hot_impl`, the jump table's entry is updated to
point at the new function. The next time the component runs, it
runs the new code. The outer function — and everything that
holds a function pointer to it — never has to be updated.

This is the same mechanism the [subsecond](https://github.com/DioxusLabs/dioxus)
crate from the Dioxus project uses. Idealyst's `framework-hot`
sits on top, integrating the dispatch with the reactive
substrate.

### The primitive-tree diff

The dispatcher takes care of "the new function runs next time."
But "next time" might not be soon, and even when it does run, it
might return a primitive tree that's structurally identical to
the old one — just with different values. The framework needs to
figure out what changed and what to update on the backend.

That's `framework-hot`'s other job. When a component re-renders
after a hot patch, the framework:

1. **Hashes each primitive in the new tree** by its identity
   (position in the tree + relevant content). Identical
   identities map to the same backend node.
2. **Diffs the trees** to find which nodes changed, which moved,
   which were added or removed.
3. **Emits the minimal sequence of backend operations** —
   `update_text`, `apply_style`, `insert`, `clear_children` —
   needed to morph the live tree into the new one.

What this means in practice:

- **Signal values survive.** Signals live in the reactive arena,
  not in the primitive tree. A hot patch doesn't touch them.
- **Component state survives** if the component's place in the
  tree is stable. Move a component or change its props
  structurally and you may lose its state.
- **Backend nodes survive** when their identity is stable.
  Editing the contents of a `Text` keeps the same DOM/UIView/
  Android-View node — the backend just gets `update_text(node,
  new_string)`.

### What forces a full reload

Some changes can't be patched. The framework falls back to a
full rebuild + remount when it detects one. The common cases:

- **Top-level signature changes** — adding a parameter to `app()`,
  for example. The dispatcher can't reconcile the call site.
- **Adding a `methods!` block** to a component that didn't have
  one. The return type changes from `Primitive` to
  `Bindable<Handle>`.
- **Renaming or removing a component.** No identity to map.

For everyday editing — tweaking strings, layout, styles, signal
values, even adding new primitives inside a component — hot
reload covers it. A full reload is the framework saying "I tried
and the change is structural enough that I'm starting over."

## AAS — app-as-server

This is the part that's worth understanding even if you never use
it for anything fancy: it changes what "running the app" means.

### The default — local-render mode

`idealyst dev --web` (or `--ios`, or `--android`) builds the app
for that platform, runs it natively, and hot-reloads source
changes in place. Everything stays in one process per platform.
If you want to run the same app on web and iOS simultaneously,
two builds, two processes.

This is fine and fast. It's what most development looks like.

### AAS mode

`idealyst dev --aas` is different. The CLI builds **one
dev-host binary** that runs the user's reactive runtime. It
starts a WebSocket server. It advertises itself over the local
network using **Bonjour** (Apple's name for mDNS / DNS-SD —
the same zero-config service discovery iTunes, AirPlay, and
"Shared" devices in Finder all use). Clients on the same network
find the dev-host without you typing a URL.

Then clients connect:

```
┌─────────────────────────────┐
│   AAS dev-host (your Mac)   │
│   - app()'s reactive state  │
│   - signals, effects, scopes│
│   - the primitive tree      │
└──────────────┬──────────────┘
               │  WebSocket: wire commands
               │  (NodeId, StyleId, HandlerId)
   ┌───────────┼───────────┐
   ▼           ▼           ▼
 browser     iOS sim    Android dev
 (thin       (thin       device
  client)     client)    (thin client)
```

Each client is a **thin client backend**. It receives wire
commands from the dev-host and replays them against its real
backend (DOM on web, UIKit on iOS, Views on Android). It doesn't
run any of your app code — it doesn't need to. The dev-host is
the single source of truth for what the app looks like.

When you click a button on the iOS client, the click event is
forwarded back to the dev-host over the wire. The dev-host's
button handler fires. Signals update. Effects re-run. The new
wire commands ship to **every connected client**. All of them
update simultaneously.

When you edit `src/lib.rs` and save, only the dev-host binary
rebuilds. The clients are unaware that a build happened — they
just start receiving different wire commands. Navigation state,
scroll position, and signal values that were on the dev-host's
side survive across the rebuild because they live in the
dev-host's arena, not in any client.

### What AAS is good for

- **Cross-platform comparison.** Run web, iOS, and Android
  clients side by side, navigate on one, watch the others
  follow. Useful for spotting platform-specific rendering
  differences in real time.
- **State that survives source edits.** Navigation, scroll
  positions, complex form state — all of it lives in the
  dev-host's reactive arena and persists across rebuilds. Useful
  when you're 6 screens deep into testing and don't want to
  re-navigate after every change.
- **Driving demos.** One canonical instance of the app, multiple
  windows or devices showing it.
- **Debugging the wire protocol or backends.** Because the wire
  is the seam, you can log it, replay it, modify it in flight.

### What AAS isn't good for

- **GPU work.** `Graphics` primitives don't ship over the wire.
  They emit a placeholder when the dev-host is in AAS mode. If
  you're building anything with a `wgpu` canvas, run in
  local-render mode.
- **Performance testing.** Every interaction pays a WebSocket
  round-trip. AAS is for development, not perf measurement.
- **Production.** AAS is a development tool. Shipping an
  app-as-server architecture has security implications that the
  current implementation doesn't address.

### A few specifics

- **Discovery via Bonjour.** Clients browse for
  `_idealyst-dev._tcp.` service records on the local network.
  Different projects on the same network get different bundle
  IDs in their records, so a client running your `com.example.
  my-app` build picks the host advertising the same bundle id —
  not your coworker's `com.example.other-app` host on the same
  Wi-Fi. Cross-platform: works on macOS (built in), iOS (built
  in), Android (via NSD), and most modern Linux distros (via
  Avahi). Windows ships Bonjour with iTunes; without it, you'd
  need to install the Apple Bonjour print services or
  point-and-connect via IP instead.
- **Connection-time snapshot.** When a fresh client connects mid-
  session, the dev-host sends it a **`SceneModel` snapshot** —
  the current state of the tree — rather than replaying the full
  command log. The client comes in synced.
- **Reverse channel for callbacks.** Click handlers, text
  changes, and other input events go from client to dev-host on
  the same WebSocket. The dev-host resolves them to in-process
  closures and fires them.

The [Architecture](#architecture-in-more-depth) section of the
Overview points at the supporting crates: `framework-wire` for
the protocol, `framework-dev-client` for the app-side replayer,
`backend-aas-shell-native` for the desktop client side.

## The MCP server — Claude Desktop and Robot

When Robot is enabled (`--features robot`), your app listens on
TCP port `9718` for clients that speak the Robot bridge protocol.
Anything that can connect and issue JSON-RPC over a TCP socket
can drive the app: list components, click buttons, type into
inputs, invoke `methods!` methods, read frames.

`robot-mcp-proxy` is a small binary that **bridges this bridge to
MCP**. MCP — the Model Context Protocol — is the protocol Claude
Desktop and similar tools use to expose tools to a model. The
proxy speaks MCP on stdio and the Robot bridge protocol over TCP,
translating between them.

The flow:

```
┌───────────────┐         stdio          ┌───────────────────┐
│ Claude        │ ───────── MCP ───────► │ robot-mcp-proxy   │
│ Desktop       │ ◄────── (JSON-RPC) ─── │ (translates)      │
└───────────────┘                        └──────────┬────────┘
                                                    │
                                                    │ TCP :9718
                                                    │ (Robot bridge)
                                                    ▼
                                         ┌───────────────────┐
                                         │  Your running app │
                                         │  (--features robot)│
                                         └───────────────────┘
```

To wire it up, point the MCP client at the proxy binary. For
Claude Desktop, that's a few lines in its config:

```json
{
    "mcpServers": {
        "my-app": {
            "command": "robot-mcp-proxy",
            "args": ["--port", "9718"]
        }
    }
}
```

Now the model gets every Robot tool — `find_element`, `click`,
`type_text`, `get_snapshot`, `invoke_method`, and the rest — as
callable functions. You can ask it to interact with your
running app in natural language; it issues the tool calls; the
proxy translates them; the app responds.

The full list of tools and what they do lives on the [Robot](#)
page.

### What this is good for

The expected use cases:

- **Interactive UI testing with an LLM in the loop.** Describe a
  scenario; the model exercises it; you watch what happens.
- **Reproducing bugs.** Hand the model the symptoms; it pokes
  the UI until it finds the path to the broken state.
- **Authoring tests.** Have the model walk a happy path, log the
  tool calls it made, and convert them into a deterministic
  test script.

You can also write your own MCP client — the protocol is
documented and the proxy is small. The MCP integration is one
example of what the Robot bridge enables, not the only one.

## A simple mental model

To recap how the pieces fit:

- **The CLI** turns your one-crate project into platform builds.
  Per-platform wrappers live in a build cache, generated on
  demand.
- **Hot reload** patches a running app without restarting it.
  Component bodies update through a function-pointer dispatcher;
  primitive trees update through identity-keyed diffs.
- **AAS** is a dev mode where the framework runs once on your
  machine and thin clients connect over WebSocket. State lives
  in one place; every client sees it.
- **Robot** is the introspection layer that lets external
  processes drive a running app.
- **The MCP proxy** is the bridge from MCP-speaking tools (Claude
  Desktop, etc.) to Robot.

All four are independent and optional. You can build apps using
none of them — `cargo build`, manual wrapper crates, no hot
reload, no Robot. The tooling exists because building this way
is faster and more pleasant, not because the framework demands
it.

## Where to read more

- [Robot](#) — the tools the bridge exposes, in detail.
- [Backends](#) — what the AAS thin clients are doing on each
  platform.
- [Architecture in more depth](#architecture-in-more-depth) (on
  the Overview) — where `framework-hot`, `framework-wire`, and
  `framework-dev-client` sit.
- [Hot reload internals](#) — identity hashing, diff strategy,
  what survives a patch in detail.
- [The wire protocol](#) — the `Command` enum and id namespaces,
  for anyone building their own wire consumer.
