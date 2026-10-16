# Robot

Robot is an introspection layer that lets another process drive a
running Idealyst app — list the components on screen, find a button
by its label, click it, read a TextInput's frame, invoke an
imperative method on a mounted component. It's the framework's
answer to "how do I test the app, or automate it, or let an AI poke
at it" without writing a per-platform test harness.

Every backend answers the same verbs, so one Playwright-style suite
runs unchanged on web, macOS, iOS, Android, Linux and Windows. A test
that passes on one platform and fails on another has found a real
difference between the backends.

## Enabling Robot

Every dev build carries it: `idealyst dev` and `idealyst test` turn on
the framework's `robot` features themselves. Release builds carry none
of it, and an app declares no feature of its own.

Everything an app writes to be testable — a `test_id`,
`robot::watch_signal`, a `#[robot_fn]` registration,
`robot::is_test_driven()` — compiles in every build and does nothing
outside a robot build, so it needs no `cfg`. (An app-side
`#[cfg(feature = "robot")]` would be off in dev builds too: the CLI
turns on the framework's feature, not the app's.)

A robot build exposes a TCP bridge (port `9718` by default) and
registers itself under `~/.idealyst/apps/`. The typical consumers are
`idealyst test` and the **MCP server** (`idealyst mcp`) — see the
[Dev tools](dev-tools) page for wiring it into an MCP client. The
bridge protocol is small enough that you can write your own client in
any language with a TCP socket and a JSON parser.

## Test IDs

Every element can carry a `test_id`:

```rust
ui! {
    button(
        label = "Submit",
        on_click = move || submit(),
        test_id = "submit-button",
    )
}
```

`test_id` is the stable name your tests refer to. It doesn't change
when you tweak a label or restructure a screen, which makes it more
durable than matching by visible text.

You can also find elements by label, by label substring, or by
primitive kind ("Button", "Text", "TextInput", …), so test IDs
aren't strictly required. They just give you the most reliable
hook.

## The control surface

Robot exposes query, interact, functions, geometry, highlight, logs,
component, signal and navigator operations.

### Query the tree

| Tool | What it does |
| --- | --- |
| `find_element` | First element matching `test_id` / `label` / `label_contains` / `kind`. |
| `find_all_elements` | Every element matching the same criteria. |
| `get_snapshot` | The full component hierarchy as a tree of `{id, kind, label, children}`. |
| `get_children` | Direct children of an element. |
| `get_parent` | Parent of an element. |
| `count_elements` | Total mounted elements, optionally filtered by kind. |

A typical scripted check is "find by test_id, assert its children
look right." `get_snapshot` is the heavy hammer when you want to
inspect the whole tree at once.

### Interact

| Tool | What it does |
| --- | --- |
| `click` | Press an element inside the framework: a button, pressable or link runs its press, and a view's `on_touch` gets a synthetic tap. A tap on a Text inside a clickable row reaches the row. Skips the OS event path, so overlays and hit testing don't take part. |
| `tap_native` | Tap the element's center with real platform input, through the toolkit's event path (hit testing, overlays, gesture recognizers, the native disabled state). Refuses as a finger would fail: not visible, center outside the window, or covered (the error names what's on top). Answers `{mechanism}`: `native` on web, macOS, Android and Windows; `hit_test` on iOS (UIKit's hit test checks the point, then the press runs); `unsupported` on Linux. |
| `drag` | Move one pointer along `points` (`[x, y]` in px from the element's top-left): `Began`, `Moved`…, `Ended` into the element's `on_touch` or the nearest ancestor's. Draws on a signature pad, drives a pan or drag-to-reorder. On a Slider each point's x along the track sets the value. Pass `hold_ms` to press and hold at the first point before moving: drag-and-drop on phones and tablets picks up after a 200 ms hold. |
| `type_text` | Replace a TextInput's value with new text. |
| `set_toggle` | Set a Toggle's `Signal<bool>`. |
| `set_slider` | Set a Slider's `Signal<f32>`. |
| `drag_native` | `drag` with real platform input (unsupported on iOS and Linux). |
| `focus` / `blur` | Move keyboard focus in/out of an element. |
| `open_link` | Deliver a deep link as the OS would: observers, interceptors, then the navigators. |

Each element action takes an `element_id` from a prior query, and a
disabled control refuses it. The flow is always: find → act → query →
assert. `get_state {element_id}` reads an element's own state — a
field's text, a toggle's checked state, `disabled`, `focused`, a
scroller's offset.

### Read geometry

| Tool | What it does |
| --- | --- |
| `get_frame` | Bounding rect in the *parent's* coordinate system. |
| `get_absolute_frame` | Bounding rect in viewport (window) coordinates. |

Both return `{x, y, width, height}` in pixels, or `null` if the
element exists but hasn't been laid out yet. Use `get_frame` to
answer "where is this relative to its container"; use
`get_absolute_frame` for "where is this on screen".

### Highlight an element

| Tool | What it does |
| --- | --- |
| `highlight_element` | Draw a translucent box over `{element_id}` in the running app. It follows the element as it moves and disappears when the element unmounts. |
| `clear_highlight` | Remove the box. |

The box is drawn by the framework's own `overlay` and `view`, so it
looks the same on every platform. It passes clicks through to the app,
and it never appears in `get_snapshot` or `count_elements`. Nothing is
added to the app until the first highlight. It needs the element's
`get_absolute_frame`: on a platform that can't report one,
`highlight_element` returns an error saying so.

### Inspect logs

| Tool | What it does |
| --- | --- |
| `get_logs` | Captured log entries — framework, backend, stdout/stderr. Each `{ts, source, text}`. Supports `since` for polling, `limit` for last-N. |
| `clear_logs` | Drop the buffer. Useful before reproducing an issue. |

Captured logs include `eprintln!` from Rust, `NSLog` from iOS, and
the backend's own diagnostics. The buffer is a ring; old entries
are dropped if you don't drain.

### Inspect components

Every mounted `#[component]` registers itself in robot builds (every
`idealyst dev` build), with its props and source location. Registering
adds no node to the tree: a dev build lays out exactly like a release
build.

| Tool | What it does |
| --- | --- |
| `get_snapshot` | The element tree. A node that is a component's root carries `components: [{instance_id, name}]`, outermost first (a component whose root is another component's element shares it). Read top-down, those links are the rendered component hierarchy. |
| `list_components` | Every mounted component instance: `{instance_id, name, file, line, element_id, methods: [{name, args}]}`. |
| `get_component` | One instance with its props: `props: [{name, type, mode, value}]`. `mode` is `static` or `live` for a `Reactive<T>` prop (which arm the caller passed), `signal`, `handler`, `children`, `value` or `opaque`; `value` is the current `Debug` rendering (truncated), or `null` when the type has no `Debug`. |
| `invoke_method` | Call one of a component's `#[method]`s with a JSON args object keyed by parameter name. |

So if you have:

```rust
#[component]
fn Counter(initial: i32) -> Element {
    let value = signal(initial.get());
    #[method]
    fn bump_by(n: i32) { value.update(|v| v + n); }
    ui! { text(test_id = "count") { "{value}" } }
}
```

…a test script can call:

```jsonc
// invoke_method
{
    "instance_id": 12,
    "method": "bump_by",
    "args": { "n": 5 }
}
```

…and the running app's counter increments by 5. Args
JSON-deserialize into the parameter types reported by
`list_components`, so anything `serde` can decode works, including
custom structs.

### Watch signals

A signal is visible to the bridge once the app calls
`robot::watch_signal("name", sig)`; `robot::watch_signal_writable`
also lets the bridge set it (the value type must be `Deserialize`).

| Tool | What it does |
| --- | --- |
| `list_watched_signals` | `{id, name, value, writes, changed_ago_ms, writable}` per watched signal. |
| `read_signal` | One value, by `name` or `id`. |
| `get_signal_history` | The last 64 values, oldest first, each with `ago_ms`. |
| `write_signal` | Set a writable signal: `{id or name, value}` with the value as JSON. |

### Call functions

Some things a test needs have no element to act on: seed data, fake a
location, take the device offline, read what notifications the app
posted. The app registers them with `#[robot_fn]`, and a test calls
them by name:

```rust
/// Fill the inbox with `count` sample messages.
#[robot_fn(name = "demo.seed")]
fn seed_inbox(count: u32) -> Result<Vec<String>, String> { /* … */ }

// At boot, on the UI thread (no cfg):
runtime_vocabulary::robot_fn::register(seed_inbox::robot());
```

| Tool | What it does |
| --- | --- |
| `list_functions` | What is registered, with each function's doc comment and signature. |
| `call_function` | `{name, args}`, args keyed by parameter name; the return value comes back as JSON. An `async fn` answers `{done: false, call}`, polled with `poll_function` until done. |

The device SDKs ship fakes a test turns on the same way, each
registered with that SDK's `register_robot_controls()`:
`location.set` / `.clear`, `connectivity.set` / `.clear`,
`permissions.set` / `.clear` / `.requests`, `notifications.log` /
`.clear_log`. A test assumes the answer it needs instead of meeting a
dialog it can't drive. The fakes compile only into robot builds.

### Drive navigators

| Tool | What it does |
| --- | --- |
| `list_navigators` | Every navigator: active route and path, back stack, `controllable`. |
| `navigate` | `{nav_id, action: push/replace/reset/pop, path}`. The path is a full path (the navigator's base included) and resolves the way a deep link does. |

The Inspector is built on exactly these verbs. `idealyst inspect` opens it in the browser; its server drives them on each app it inspects.

## Writing a suite

Host-side tests live in `tests/robot.rs` with `robot-test` as a
dev-dependency, and read like Playwright. Every action waits for its
element to exist and accept it, and every assertion retries until it
holds, so a test never sleeps:

```rust
use robot_test::{json, robot_test, App};

#[robot_test]
fn increment_updates_count(app: &mut App) {
    app.call_function("demo.set_count", json!({ "value": 0 }));
    app.test_id("count-display").assert_text("Count: 0");
    app.test_id("increment-btn").click();
    app.test_id("increment-btn").click();
    app.test_id("count-display").assert_text("Count: 2");
}
```

`click()` here is native-first: it taps with real input and falls back
to the in-framework press only where the backend has no native tier.
Run the suite on any platform with `idealyst test --web` (or
`--macos`, `--ios`, `--android`, `--linux`, `--windows`): it builds and
launches the app, waits for it, and runs `cargo test --test robot`
against it.

## Use cases

Things people use Robot for:

- **Automated UI tests.** Write a test once that exercises the
  same screen on every platform. The script doesn't change per
  platform.
- **Driving demos.** Script a deterministic walkthrough that
  always lands on the right screen with the right data.
- **Accessibility tooling.** External processes can read the
  component tree, frames, and labels — the building blocks of an
  alternate input system.
- **Letting an LLM poke at the app.** The MCP proxy bundled with
  the framework was built for this case: connect Claude Desktop
  (or another MCP client) to your running app, and the model can
  drive it via natural language.

## The MCP server

`idealyst mcp` is the stdio MCP server that ships with the framework.
It speaks the [Model Context Protocol](https://modelcontextprotocol.io)
on stdin/stdout and the Robot bridge protocol over TCP. It finds a
running app via the per-process registration files under
`~/.idealyst/apps/`, or you can point it at a known bridge with
`--robot-port`. Any MCP client can then drive that app through it.

Wiring it into Claude Code or Claude Desktop is a config change
(`idealyst mcp install` writes this for you):

```json
{
    "mcpServers": {
        "idealyst": {
            "command": "idealyst",
            "args": ["mcp"]
        }
    }
}
```

…and the model gets every tool on this page as a callable
function, plus the static component catalog.

The Robot MCP tools mirror the bridge surface above: `find_element`,
`get_state`, `click`, `tap_native`, `type_text`, `list_functions` /
`call_function`, `open_link`, `get_snapshot`,
`list_component_instances` / `invoke_method` (the MCP's
`list_components` is the static catalog), and `call_verb` for anything
without its own tool. The server's `testing` guide walks an agent
through writing a suite.

## Constraints and notes

- **Single thread.** Robot reads from the thread the app's UI runs
  on; each request is answered between frames, and staged reactive
  work settles after every action.
- **The bridge is dev-mode.** There's no auth on port `9718`. Release
  builds compile none of it.
- **Native input varies by backend.** `tap_native` is real input on
  web, macOS, Android and Windows, a hit-test check on iOS, and
  unsupported on Linux. Roku's Robot support is a subset of the
  runtime backends' — geometry queries depend on what the device-side
  runtime exposes.
- **Custom primitives are visible.** Builtin primitives register with
  Robot at mount; a third-party primitive's handler registers its node with
  `runtime_vocabulary::robot_ext::register_extension_node`.

## Where to read more

- [Dev tools](dev-tools) — the MCP proxy in context, the bridge protocol
  on the wire, how Robot fits with `idealyst dev`.
- [Components](components) — `#[method]` fns, the source of
  `invoke_method` targets.
- [Primitives](primitives) — the `test_id` slot every primitive carries.
