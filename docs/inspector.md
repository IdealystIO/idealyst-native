# The Inspector

The Inspector is a live debugging dashboard for running idealyst apps:
the rendered component tree with each instance's props and methods,
watched signals and their history, every navigator's back stack, and
the captured logs and phase timers. It can also act on the app: invoke a
`#[method]`, write a signal, push or pop a route. It starts in idea-ui's
dark theme; the sun/moon button (sidebar and app picker) switches to the
light theme for the session.

```text
idealyst inspect                    # the Inspector, at http://127.0.0.1:9719
idealyst dev --web --local --inspect   # run an app and open the Inspector on it
```

## Who owns what

The Inspector has two halves, and only one of them ever talks to an app.

```text
 app ──WS──▶ robot-relay ──TCP──▶ inspector-server ──HTTP + WS──▶ front end
            (idealyst dev)        (idealyst inspect)             (browser / desktop)
```

- **The server** (`crates/dev/inspector-server`, hosted by the CLI) owns
  all debug state. It discovers running apps from their registrations
  under `~/.idealyst/apps`. It holds one robot-bridge connection per
  inspected app and refreshes that app's state, both when the bridge
  pushes a change and on a fallback cadence. It keeps the running totals
  and runs every action.
- **The front end** (`examples/inspector`) renders what the server
  pushes and sends back what the user did. It never opens a connection
  to an app, so it needs nothing but one WebSocket. That's what lets it
  run in a browser, which can't open TCP sockets or read
  `~/.idealyst/apps`.

Owning the state on the server fixes a real bug as well as allowing a
browser front end. `get_perf_counters` *drains* the app's counters, so
two Inspectors that each polled the app saw a fraction of the calls
each. The server's single connection drains them and sums them, and
every front end attached to that app reads the same totals. What *does*
differ per front end is its **focus**: the selected component and
signal. The server fetches their details once per distinct focus per
refresh and sends each front end its own snapshot.

The app side is unchanged. The server speaks the same newline-JSON
bridge protocol the MCP server and `idealyst test` use, to a native
app's bridge or to the relay an `idealyst dev --local` app dials.

## The CLI

`idealyst inspect [--port 9719] [--app <id>] [--no-open]` starts the
server and opens the browser. There is one server per machine. If
something already answers `/health` on the port with
`idealyst-inspector`, the command opens the browser on that server
instead of failing on the busy port.

`idealyst dev --inspect` does the same for the session it runs. It hosts
the server in the dev process, or reuses one that's already running, and
opens the page with `?app=<id>`. The id is the relay's registration file
stem, so the page attaches as soon as the server lists the app. Without
a relay (the relay only runs with `--local`) there's no id to name, and
the page opens on the app list.

An app's id is its registration's file stem: `<name>-<pid>`, where the
pid is the process that registered. For a relayed app that's the
`idealyst dev` process, not the app.

## Protocol

`crates/dev/inspector-protocol`: plain serde types, shared by the server
and the wasm front end. One WebSocket per front end at `/ws`; each text
frame is one JSON message.

| Direction | Message | When |
| --- | --- | --- |
| server → client | `{"type":"hello","protocol":1}` | first frame |
| server → client | `{"type":"apps","apps":[…]}` | on open, then whenever the set changes (rescanned every second) |
| client → server | `attach {app}` / `attach_addr {addr}` / `detach` | pick what to inspect |
| client → server | `focus {component, signal}` | selection changed |
| client → server | `action {label, cmd, args}` | a bridge verb to run on the app |
| client → server | `rescan` | refresh the app list now |
| client → server | `highlight {element}` | box an element in the app (`null` clears); no `last_action`, no refresh |
| server → client | `{"type":"snapshot","app":…,"snapshot":{…}}` | the attached app's state changed, for this client's focus |

A snapshot is the whole picture and is sent only when it differs from
the previous one sent to that front end. An action's outcome arrives as
the next snapshot's `last_action`, and only the front end that sent the
action gets it. `app` echoes what the front end attached with, so a
frame already in flight when the user switched apps is dropped.

The server keeps nothing about a front end across connections. The
client remembers its attachment and focus and replays them when the
socket reconnects, and it retries every second while the server is
away.

## Highlighting an element

Hovering a row in the component tree boxes that element in the running
app, the way browser devtools do. The front end sends `highlight` (only
when the hovered element changes). The server calls the app's
`highlight_element` / `clear_highlight` bridge verbs and clears the box
if that front end detaches or disconnects mid-hover.

The app side lives in `runtime-vocabulary` (`robot_highlight`, robot
builds only):

- **Built from the framework's own primitives.** The box is an `overlay`
  holding one absolutely positioned `view`, the same composition idea-ui's
  `ToastHost` floats above an app with. It needs no per-backend code, and
  it passes clicks through.
- **Mounted only while a highlight is up.** Until then, a dev build's tree
  is exactly the release build's. The first `view` mounted (normally the
  app root) records how to mount into itself. The verb realizes the
  overlay as that view's last child, and `clear_highlight` removes it.
- **Follows its element.** It re-reads the element's `absolute_frame`
  every 120 ms, and takes itself down when the element unmounts.
- **Invisible to introspection.** It never appears in `get_snapshot`,
  `count_elements`, or the Inspector's own tree.
- **Needs a frame.** It depends on the backend's `absolute_frame`. That's
  wired on web, macOS, iOS, Linux and wgpu. Android reports only screen
  pixels for now, so `highlight_element` answers "no frame" there.

## Guarding the socket

Browsers don't apply CORS to WebSockets, so any page the developer
visits could otherwise open `ws://127.0.0.1:9719/ws` and drive their dev
app. The server binds 127.0.0.1 only and refuses the upgrade unless the
`Origin` is its own (`http://127.0.0.1:<port>`, `http://localhost:<port>`)
or absent. Native clients such as the desktop Inspector send no `Origin`.

## How the front end gets into the CLI

The CLI's `build.rs` compiles `examples/inspector` for the web with the
same pipeline `idealyst build --web --release` runs, and embeds the
staged files (`index.html`, the JS glue, one wasm module) with
`include_bytes!`. The CLI installs from a git checkout of this
workspace, so the source is always there.

- Building the CLI therefore needs the wasm toolchain (the
  `wasm32-unknown-unknown` target, `wasm-opt`, and `wasm-bindgen`: the
  Inspector still reads its host and query string through web-sys, so it
  builds in hybrid mode). When a
  piece is missing, the CLI still builds. It emits a build warning, and
  `idealyst inspect` says why there's no page. The server still runs.
- `build.rs` re-runs when any workspace crate in the Inspector's wasm
  dependency closure changes (read from `cargo metadata`). The nested
  build is incremental, but after a runtime edit it still recompiles.
  Set `IDEALYST_CLI_SKIP_INSPECTOR=1` for CLI work that doesn't need the
  page.
- To work on the front end without rebuilding the CLI, build it yourself
  and serve that bundle from disk:

  ```text
  idealyst build --web examples/inspector --out-dir /tmp/inspector
  idealyst inspect --bundle-dir /tmp/inspector
  ```

## The desktop build

`examples/inspector` still builds for macOS. It connects to the same
server at `ws://127.0.0.1:9719/ws` (`IDEALYST_INSPECT_URL` overrides
this), so `idealyst inspect --no-open` must be running.
`IDEALYST_INSPECT_APP=<id>` plays the part of `?app=`.
