# Robot

Robot is an introspection layer that lets another process drive a
running Idealyst app — list the components on screen, find a button
by its label, click it, read a TextInput's frame, invoke an
imperative method on a mounted component. It's the framework's
answer to "how do I test the app, or automate it, or let an AI poke
at it" without writing a per-platform test harness.

When Robot is on, every backend exposes the same query surface, and
the same script can drive the web build, the iOS app on a
simulator, or the Android app on a device. You write the test
once; it runs against whichever backend you happen to be looking
at.

## Enabling Robot

Robot is feature-gated. Build your app with `--features robot` to
turn it on:

```bash
idealyst dev --web -- --features robot
idealyst build --release -- --features robot
```

(The flags after `--` are passed through to `cargo`.)

The app then exposes a TCP bridge on port `9718` by default. Any
process that speaks the bridge protocol can connect and start
issuing commands. The typical consumer is the **MCP server**
(`robot-mcp-proxy`) bundled with the framework — see the [Dev
tools](#) page for how to wire it into Claude Desktop or another
MCP client.

You don't have to use the MCP proxy. The bridge protocol is small
enough that you can write your own client in a few hundred lines of
any language with a TCP and JSON parser.

When the `robot` feature is off, every cost — the bridge thread,
the introspection metadata, the `test_id` slots on each primitive —
compiles to nothing. There's no production overhead.

## Test IDs

Every primitive accepts an optional `test_id` when Robot is on:

```rust
ui! {
    Button(
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

Robot exposes four kinds of operations: **query**, **interact**,
**geometry**, and **introspect components**.

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
| `click` | Fire a button's or pressable's `on_click`. |
| `type_text` | Replace a TextInput's value with new text. |
| `set_toggle` | Set a Toggle's `Signal<bool>`. |
| `set_slider` | Set a Slider's `Signal<f32>`. |
| `focus` / `blur` | Move keyboard focus in/out of an element. |

Each interact tool takes an `element_id` from a prior query. The
flow is always: find → act → query → assert.

### Read geometry

| Tool | What it does |
| --- | --- |
| `get_frame` | Bounding rect in the *parent's* coordinate system. |
| `get_absolute_frame` | Bounding rect in viewport (window) coordinates. |

Both return `{x, y, width, height}` in pixels, or `null` if the
element exists but hasn't been laid out yet. Use `get_frame` to
answer "where is this relative to its container"; use
`get_absolute_frame` for "where is this on screen".

### Inspect logs

| Tool | What it does |
| --- | --- |
| `get_logs` | Captured log entries — framework, backend, stdout/stderr. Each `{ts, source, text}`. Supports `since` for polling, `limit` for last-N. |
| `clear_logs` | Drop the buffer. Useful before reproducing an issue. |

Captured logs include `eprintln!` from Rust, `NSLog` from iOS, and
the backend's own diagnostics. The buffer is a ring; old entries
are dropped if you don't drain.

### Drive component methods

The most interesting part for testing apps that use `methods!`:

| Tool | What it does |
| --- | --- |
| `list_components` | Every mounted `#[component]` instance that declared a `methods!` block. Returns `{instance_id, fn_name, methods: [{name, args}]}`. |
| `invoke_method` | Call one of those methods with a JSON args object keyed by parameter name. |

So if you have:

```rust
#[component]
pub fn counter(props: &Props) -> Bindable<CounterHandle> {
    let value = signal!(0);
    methods! {
        fn reset(&self) { value.set(0); }
        fn bump_by(&self, n: i32) { value.update(|v| *v += n); }
    }
    // ...
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
`list_components` — anything `serde` can decode works, including
custom structs.

This is what makes `methods!` actually useful for testing: the
parent's view of a component's imperative surface becomes the
test's view too.

## What a session looks like

A test that bumps a counter and checks the result:

```jsonc
// 1. Find the counter's display by test_id.
{ "tool": "find_element", "args": { "test_id": "count-display" } }
// → { "element_id": 7, "kind": "Text", "label": "Count: 0" }

// 2. Find the increment button.
{ "tool": "find_element", "args": { "test_id": "increment-btn" } }
// → { "element_id": 9, "kind": "Button" }

// 3. Click it twice.
{ "tool": "click", "args": { "element_id": 9 } }
{ "tool": "click", "args": { "element_id": 9 } }

// 4. Re-read the display.
{ "tool": "find_element", "args": { "element_id": 7 } }
// → { "element_id": 7, "kind": "Text", "label": "Count: 2" }
```

That's the entire shape of a Robot-driven test, modulo the JSON-RPC
envelope the bridge protocol wraps around it.

## Use cases

Things people use Robot for:

- **Automated UI tests.** Write a test once that exercises the
  same screen on web, iOS, and Android. The script doesn't change
  per platform.
- **Driving demos.** Script a deterministic walkthrough that
  always lands on the right screen with the right data.
- **Accessibility tooling.** External processes can read the
  component tree, frames, and labels — the building blocks of an
  alternate input system.
- **Letting an LLM poke at the app.** The MCP proxy bundled with
  the framework was built for this case: connect Claude Desktop
  (or another MCP client) to your running app, and the model can
  drive it via natural language.

## The MCP proxy

`robot-mcp-proxy` is a small binary that ships with the framework.
It speaks the [Model Context Protocol](https://modelcontextprotocol.io)
on stdin/stdout and the Robot bridge protocol over TCP. Point it
at a running app (the default is `127.0.0.1:9718`) and any MCP
client can drive that app through it.

Wiring it into Claude Desktop is a config change:

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

…and the model gets every tool on this page as a callable
function.

The full set of MCP tools the proxy exposes mirrors the bridge
surface above: `find_element`, `click`, `type_text`,
`get_snapshot`, `invoke_method`, and so on.

## Constraints and notes

- **Single thread, single arena.** Robot reads from the same
  thread the app runs on. Queries are synchronous and serialized
  with the render walker.
- **The bridge is dev-mode.** There's no auth on port `9718`. Don't
  ship a Robot-enabled binary to end users. Leaving the feature
  off (the default) is the production posture.
- **Generator backends are partial.** Roku's Robot support is a
  subset of what runtime backends provide — geometry queries in
  particular depend on what the device-side runtime exposes. Web,
  iOS, and Android have the full surface.
- **Custom primitives are visible.** Anything that flows through
  the standard `Backend` create/insert/update calls shows up in
  Robot's queries automatically. There's no separate registration
  step for new primitives.

## Where to read more

- [Dev tools](#) — the MCP proxy in context, the bridge protocol
  on the wire, how Robot fits with `idealyst dev`.
- [Components](#) — `methods!` blocks, the source of
  `list_components` / `invoke_method` targets.
- [Primitives](#) — the `test_id` slot every primitive carries.
