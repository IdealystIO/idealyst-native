+++
title = "Testing apps (robot E2E)"
order = 62
tags = ["testing", "e2e", "robot", "robot_test", "robot_fn", "tap_native", "call_function", "idealyst test", "mcp"]
+++

# Testing apps (robot E2E)

Every dev build carries the **robot**: a bridge that finds elements in the
running app, acts on them and reads them back, with the same verbs on every
backend. One Playwright-style suite runs unchanged on web, macOS, iOS,
Android, Linux and Windows, so a test that passes on one platform and fails
on another has found a real difference between the backends.

The same bridge is what this MCP server's robot tools drive. An agent can
explore a live app with them, then write the suite down as code.

Release builds carry none of it. Everything below compiles in every build
and does nothing outside a robot build, so **an app never needs a `cfg` to
be testable**. (An app can't gate these on a feature of its own anyway:
`idealyst dev` / `idealyst test` turn on the framework's `robot` features,
never the app crate's.)

## 1. Make the app addressable

Give the elements a test touches a `test_id`:

```rust
ui! {
    view() {
        text_input(value = email, test_id = "email")
        button(label = "Sign in", on_click = submit, test_id = "submit")
        text(test_id = "greeting") { "Hello, {name}" }
    }
}
```

Most assertions need nothing more. Every element reports its own state:
a field's text, a toggle's checked state, `disabled`, `focused`, a
scroller's offset, plus `visible` and `in_viewport`.

For app state with no element, expose it by name or as a function:

```rust
// A signal a test reads (`read_signal`) or, if writable, sets.
runtime_vocabulary::robot::watch_signal("cart_count", cart_count);

/// Fill the inbox with `count` sample messages.
#[runtime_core::robot_fn(name = "demo.seed")]
fn seed_inbox(count: u32) -> Result<Vec<String>, String> { /* … */ }

// At boot, on the UI thread:
runtime_vocabulary::robot_fn::register(seed_inbox::robot());
```

- `watch_signal` must run where effect creation is legal (a component body
  or the app's root build), because the entry lives as long as that scope.
  It takes a `Signal`, `ReadSignal` or `Memo` whose value is `Debug`.
  `watch_signal_writable` also lets the bridge set it; its type must be
  `Deserialize`.
- A `#[robot_fn]` is a free function: no generics, no `self`, no borrowed
  parameters. Its arguments arrive as a JSON object keyed by parameter
  name, decoded with `serde`; a missing `Option` is `None`. The return
  value is encoded back, and an `Err` fails the call with its `Display`.
  `<fn_ident>::robot()` is named after the Rust function, not the
  `name = "…"`.
- A `#[robot_fn]` may be an `async fn`. It runs on the backend's executor,
  and the caller polls it to completion (the client and MCP tool do that
  for you).
- Functions run on the UI thread outside any `World`, like an event
  handler. They may read and write signals, but not create them, so keep
  the signal in a `thread_local` the app fills at mount.

An app with an in-app self-test should stand it down while a driver owns
the app: `if !runtime_vocabulary::robot::is_test_driven() { … }`.

## 2. Fake what the OS would answer

A test can't tap through a permission dialog, move the device or pull the
network cable. The device SDKs ship fakes a test turns on through robot
functions. Register them at boot (no `cfg`, inert in release):

| SDK | Register | Functions |
|---|---|---|
| `location` | `location::register_robot_controls()` | `location.set {latitude, longitude, accuracy_m?, …}`, `location.clear` |
| `connectivity` | `connectivity::register_robot_controls()` | `connectivity.set {online, transport?}`, `connectivity.clear` |
| `notifications` | `notifications::register_robot_controls()` | `notifications.log` (what the app posted), `notifications.clear_log` |
| `permissions` | `permissions::register_robot_controls()` | `permissions.set {permission, status}`, `permissions.clear`, `permissions.requests` |

A fake replaces the platform for the SDK's whole surface until it is
cleared. For example, after `location.set`, `current()` resolves the fix
with no permission prompt, every live `watch` receives it, and the
device's own updates are dropped. Give your own SDK the same shape: its
fakes go in `runtime_vocabulary::robot_only! { … }` (items only robot
builds compile) or `robot_select! { on: { … } off: { … } }` (one set of
items per build), so a release build carries none of them.

Deep links need no SDK: `open_link {url}` delivers a URL as the OS would,
to observers, interceptors and then the navigators.

## 3. Write the suite

Host-side tests live in `tests/robot.rs`, with `robot-test` as a
dev-dependency (`robot-test = { version = "1", registry = "idealyst" }`,
or `{ workspace = true }` inside the framework workspace):

```rust
use robot_test::{json, robot_test, App};

#[robot_test]
fn sign_in(app: &mut App) {
    app.call_function("permissions.set", json!({ "permission": "location", "status": "granted" }));
    app.navigate("/login");
    app.test_id("email").fill("me@example.com");
    app.test_id("submit").assert_disabled();   // waits for the rule to apply
    app.test_id("password").fill("hunter2");
    app.test_id("submit").click();             // waits for it to be enabled
    app.assert_path("/home");
    app.signal("cart_count").assert_eq(0);
}
```

**Everything auto-waits.** An action waits for its element to exist and
accept it, and an assertion retries until it holds. Both give up after
`App::set_timeout` (5 s by default) and panic with the last thing they
saw. Never sleep for a reactive update, a load or a navigation.

- **Locate:** `test_id`, `text` (label substring), `label` (exact), `role`
  (element kind, e.g. `"Toggle"`), then `.nth(i)`.
- **Act:**
  - `click`, `fill` / `type_text`, `set_toggle`, `set_slider`, `focus`,
    `blur`, `scroll_to`;
  - `drag`, `drag_with_hold`, `drag_native`;
  - `tap_native`, `dispatch_click`.
- **Read:** `state()`, `is_visible()`, `label()`, `count()`.
- **Assert:**
  - presence: `assert_visible`, `assert_hidden` (passes when nothing
    matches), `assert_in_viewport`, `assert_count`;
  - content: `assert_text`, `assert_text_contains`, `assert_value`;
  - state: `assert_checked`, `assert_enabled`, `assert_disabled`,
    `assert_focused`.
- **App:**
  - navigation: `navigate` / `navigate_with` / `back`, `assert_path`,
    `navigators()`, `open_link(url)`;
  - functions: `call_function(name, args)`, `try_call_function`,
    `functions()`;
  - components: `component(name).invoke(method, args)` / `.prop(name)`;
  - signals: `signal(name).assert_eq` / `.value()` / `.set(v)`;
  - diagnostics: `logs()`, `screenshot()`;
  - anything else: `call(verb, args)`.

Keep tests independent of run order. Each test should be the only one
mutating the state it asserts, and a robot function that resets the state
is the cheap way to get there.

## Clicks: in-framework vs real input

| | What runs | Use it to test |
|---|---|---|
| `click` (robot) / `dispatch_click()` | The element's press handler, or a synthetic tap to the nearest `on_touch`, inside the framework | App logic |
| `tap_native` / `drag_native` | Real platform input at the element's center, through the toolkit's event path (hit testing, overlays, gesture recognizers, the native disabled state) | That a user's tap actually lands |

`Locator::click()` in robot-test is **native-first**. It waits for the
element to be visible and enabled, then calls `tap_native`, falling back
to the robot's `click` only where the backend answers `unsupported`.

| Backend | `tap_native` mechanism |
|---|---|
| Web | DOM pointer and mouse events at `elementFromPoint` |
| macOS | `NSEvent`s posted to the app |
| Android | `MotionEvent`s to the decor view (no hit test, so the injected event is the whole check) |
| Windows | Window messages posted to the child window a real click lands on |
| iOS | `hit_test`: UIKit's hit test checks the point, then the press runs |
| Linux | `unsupported`, so `click()` falls back |

A native tap refuses the way a finger would fail, with a retryable error:
- the element is not visible;
- its center is outside the window (scroll it into view first);
- something covers it. The error names the element on top.

That last case is how a test catches an overlay or modal backdrop
swallowing taps. An `overlay` with a `Dismiss` or `Opaque` backdrop covers
the whole window on every backend. With `backdrop = None`, only the
overlay's own content takes taps and the page stays usable.

## 4. Run it on every platform

```text
idealyst test --web      <project>
idealyst test --macos    <project>
idealyst test --ios      <project>     # simulator
idealyst test --android  <project>     # emulator
idealyst test --linux    <project>     # on a Linux host
idealyst test --windows  <project>     # a Windows host, or IDEALYST_WINDOWS_REMOTE
```

`idealyst test` does all of this:
- builds the app fresh and launches it (`idealyst dev --local`);
- marks it test-driven, which is what `is_test_driven()` reads;
- waits for *this* project's app to register;
- runs `cargo test --test robot` against it.

`--filter <name>` narrows the run, `--test <file>` picks another test
target, and `--attach` drives an app you already started with
`idealyst dev`. Under a bare `cargo test`, with no app reachable, each
`#[robot_test]` skips instead of failing.

## Driving a live app from the MCP

The tools mirror the verbs, so an agent can try a flow before writing it
down:

1. `list_apps`: what is running.
2. `find_element {test_id}` → an `element_id`, with `visible`,
   `in_viewport` and `state`.
3. `get_state`, `get_snapshot`, `screenshot`, `get_logs`: read.
4. `click`, `tap_native`, `type_text`, `set_toggle`, `set_slider`,
   `set_scroll`, `drag_native`: act.
5. `list_functions` / `call_function {name, args}`: the app's and SDKs'
   controls. `call_function` waits for an async function to finish.
6. `open_link {url}`, `list_navigators` / `navigate`,
   `list_watched_signals` / `read_signal` / `write_signal`.
7. `call_verb` for any verb without its own tool.

A behavior a test cannot drive or observe through these is a gap in the
robot, not something to work around with sleeps or platform checks.
