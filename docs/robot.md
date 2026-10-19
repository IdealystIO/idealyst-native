# The robot bridge

The robot is the framework's built-in automation surface: an E2E test, the
MCP server or the Inspector finds elements in a running app, acts on them and
reads them back — with the same verbs on every backend. It is compiled into
dev builds only (`robot` features); release builds carry none of it.

The goal is Playwright-style tests that run unchanged on web, iOS, Android,
macOS, Linux and Windows: one suite, every platform, and any difference in
the results is a real difference between the backends. A behavior a test
cannot drive or observe through the robot is a gap in the robot's surface,
and is treated as one.

## Where it lives

- `crates/runtime/vocabulary/src/robot.rs` — the element registry (every
  builtin primitive registers on mount and deregisters on teardown), the
  [`Robot`] API, and `bridge::invoke_command`, the verb dispatcher.
- `robot_methods.rs` (components and `#[method]`s), `robot_watch.rs`
  (`watch_signal`), `robot_props.rs`, `robot_ext.rs` (SDK nodes),
  `robot_fn.rs` (functions a test calls by name, `#[robot_fn]`).
- `crates/runtime/shared/src/robot/` — the transport (TCP bridge, relay
  client) and the registry-independent verbs (`get_logs`, `screenshot`, …).
- Clients: `crates/dev/robot-test` (`#[robot_test]` host-side tests),
  `crates/dev/robot-e2e` (in-app suites, which drive `Robot` directly: their
  `click` is the robot's `click`, with no native tier), the MCP server.

## In the app

What an app does to be testable compiles in every build, so it needs no
`cfg`: `test_id = "…"` on an element, `robot::watch_signal("name", sig)`
(and `watch_signal_writable`), `robot_fn::register(f::robot())`, an SDK's
`register_robot_controls()`, and `robot::is_test_driven()`. Outside a robot
build each is inert — `is_test_driven()` is `false`. An app can't gate them
itself: `idealyst dev` and `idealyst test` turn on the framework's `robot`
features, never the app crate's own, so an app-side
`#[cfg(feature = "robot")]` is off in dev builds too.

## Elements

`find_element` / `find_all_elements` / `count_elements` take one of
`test_id`, `label`, `label_contains` or `kind` (the first present, in that
order; with none, every element matches). An unknown `kind` is an error.
`count_elements` returns the number of matches; `find_all_elements` a list
of elements, and `find_element` the first, shaped

```json
{"id": 7, "kind": "Toggle", "test_id": "wifi", "label": null,
 "visible": true, "in_viewport": true,
 "state": {"checked": true, "disabled": false}}
```

`visible` and `in_viewport` are on every element; `state` on elements that
have any (below). `get_snapshot` nodes carry the same fields.

### Visibility

`visible` follows Playwright's rule: the toolkit shows the element and its
laid-out box is not empty. "Shown" is the backend's own answer
(`IntrospectionOps::is_shown`): in a window, not hidden by itself or an
ancestor, not fully transparent. Scrolled-away content is still visible;
`in_viewport` says whether its box overlaps the window (absent when the
frame or the window size is unknown).

| Backend | "Shown" means |
|---|---|
| Web | `isConnected` and `checkVisibility({opacityProperty, visibilityProperty})` |
| macOS | in a window, not `isHiddenOrHasHiddenAncestor`, `alphaValue` × layer opacity up the chain above ~0 |
| iOS | in a window, no `isHidden` up the chain, `alpha` × layer opacity above ~0 |
| Android | `isShown` (attached, `VISIBLE` up the chain), `getAlpha` up the chain above ~0 |
| Linux | `is_drawable` (visible and mapped), `opacity` up the parent chain above ~0 |
| Windows | reachable from a root through no hidden node, opacity product above ~0 — the painters' walk |
| Terminal | not yet: the robot falls back on the box alone |

An element with no answer and no frame (a headless host) is visible:
mounted is all that's known.

### State

`get_state {element_id}` returns the element's own state — what a user
would see on screen — with no help from the app:

| Field | Primitives | Meaning |
|---|---|---|
| `value` | `text_input`, `text_area` (string), `slider` (number) | The text the field shows; the slider's value, snapped to `step` |
| `checked` | `toggle` | What the switch shows |
| `disabled` | toggle, slider, text input/area, pressable, button | Whether it refuses input |
| `focused` | `text_input` | Whether it holds keyboard focus |
| `scroll` | `scroll_view` | `{x, y}` content offset, read from the native scroller |

It is the primitive's view, not the author's: an uncontrolled toggle reports
what it was last flipped to, a controlled one the author's value after each
write-back. Fields a primitive doesn't have are left out; a view, text or
image reports `{}`.

## Actions behave like a user

`click`, `type_text`, `set_toggle`, `set_slider`, `focus`, `blur`,
`set_scroll` and `drag` act on a control the way a person would:

- **A disabled control refuses.** The action fails with
  `action 'set_toggle' refused: the element is disabled`, and nothing
  reaches the author — so a test can assert that `disabled` blocks input.
- **The native widget moves first, then its own change callback reports
  it.** `set_toggle` flips the switch; `set_slider` clamps to the track,
  snaps to `step` and moves the thumb; `type_text` puts the text in the
  field. The author's `on_change` sees what a real interaction would
  produce.
- **`click`** runs a pressable/button/link's press, or delivers a synthetic
  touch (Began → Ended at the element's center) to the nearest view with an
  `on_touch` handler, bubbling to ancestors.

These actions run inside the app, through the framework: they test the
framework's logic and the backend's widget updates, not the OS's event
delivery or hit testing. For that, use the native tier below.

## Native input: `tap_native`, `drag_native`

`tap_native {element_id}` taps the element's center with real platform
input, through the toolkit's own event path — hit testing, overlays,
gesture recognizers, the native disabled state and the app pointer sink
all take part. `drag_native {element_id, points}` does the same for a
path (points relative to the element's top-left, as `drag` takes them).

It refuses as a finger would fail, with a retryable error: the element is
not visible, its center is outside the window ("scroll the element into
view"), or the backend's hit test finds something else on top ("another
element covers the point … — View test_id=\"scrim\" would receive the
tap"). It answers `{mechanism}`:

| Backend | Mechanism | How |
|---|---|---|
| Web | `native` | DOM `pointerdown`/`mousedown`/…/`click` at `elementFromPoint` (moves and the up go to the down target, a touch's implicit capture); a focusable target takes focus |
| macOS | `native` | `NSEvent` mouse events posted to the app (`postEvent:`), so tracking loops get their mouse-up |
| Android | `native` | `MotionEvent`s to the decor view's `dispatchTouchEvent` |
| Windows | `native` | `WM_LBUTTONDOWN`/`WM_MOUSEMOVE`/`WM_LBUTTONUP` posted to the child window a real click lands on |
| iOS | `hit_test` | An app can't create touches: UIKit's hit test checks the point, then the element's press runs as `click` |
| Linux | — | Not yet (XTest); `unsupported` |

The hit test (`IntrospectionOps::hit_contains`) is the toolkit's own:
`elementFromPoint`, `-[NSView hitTest:]`, `-[UIWindow hitTest:]`, the
Windows scene's hit walk. Android has none (Android delivers a touch to
the deepest clickable view, which geometry can't tell), so there the
injected event is the whole check.

robot-test's `Locator::click()` is native-first: it waits for the element
to be visible and enabled, then `tap_native`, falling back to the robot's
`click` only where the backend answers `unsupported`. `tap_native()` has
no fallback; `dispatch_click()` is the robot's `click` on purpose.

## Functions

Some things a test needs have no element to act on: seed data, fake a
location, take the device offline, read what notifications the app posted.
The app or an SDK registers them as robot functions, and a test calls them
by name.

```rust
/// Fill the inbox with `count` sample messages.
#[robot_fn(name = "demo.seed")]
fn seed_inbox(count: u32) -> Result<Vec<String>, String> { … }

// At boot, on the UI thread (no cfg: inert outside robot builds):
runtime_vocabulary::robot_fn::register(seed_inbox::robot());
```

`#[robot_fn]` keeps the function as written and adds `<name>::robot()`, its
record (name, doc comment, parameters, return type). The name defaults to
the function's; SDKs namespace theirs. Arguments arrive as a JSON object
keyed by parameter name, each decoded with `serde` (a missing `Option` is
`None`); the return value is encoded back, and a `Result`'s `Err` fails the
call with its `Display`. Free functions only, no generics or borrowed
parameters. Only robot builds compile the adapter, so a release build
carries none of it.

| Verb | Answers |
|---|---|
| `list_functions` | `[{name, doc, args: [{name, type}], returns, async}]` |
| `call_function {name, args}` | `{done: true, value}`, or for an `async fn` `{done: false, call}` |
| `poll_function {call}` | `{done: false}` while it runs; then `{done: true, value}` (or its error), once |

An async function runs on the backend's executor and never holds the
bridge: every transport answers each request within its own timeout, and a
function may take longer. Functions run on the UI thread outside any
`World`, like an event handler, and the robot settles after each call.

### SDK test controls

| SDK | Register | Functions |
|---|---|---|
| `location` | `location::register_robot_controls()` | `location.set {latitude, longitude, accuracy_m?, …}`, `location.clear` |
| `connectivity` | `connectivity::register_robot_controls()` | `connectivity.set {online, transport?}`, `connectivity.clear` |
| `notifications` | `notifications::register_robot_controls()` | `notifications.log`, `notifications.clear_log` |
| `permissions` | `permissions::register_robot_controls()` | `permissions.set {permission, status}`, `permissions.clear`, `permissions.requests` |

Anything that asks the OS — a permission prompt, a sensor, the network —
should be fakeable, so a test assumes the answer it needs instead of
meeting a dialog it can't drive. A fake replaces the platform for the
SDK's whole surface: `location.set`
makes `current()` resolve the fix (with no permission prompt) and sends it
to every live `watch`, and the platform's own updates are dropped until
`location.clear`. The fakes and their hooks compile only into robot builds
(`robot_only!` / `robot_select!`). Each SDK's README has the details.

Deep links need no SDK: `open_link {url}` delivers a URL as the OS would
(below).

## Other verbs

| Area | Verbs |
|---|---|
| Tree | `get_snapshot`, `get_children`, `get_parent`, `count_elements` |
| Geometry / rendering | `get_frame`, `get_absolute_frame`, `get_device_frame`, `introspect_native`, `introspect_subtree`, `screenshot` |
| Navigation | `list_navigators`, `get_navigator_state`, `navigate {nav_id, action: push\|replace\|reset\|pop, path}`, `open_link {url}` (a deep / universal link, as the OS delivers one: observers, interceptors, then the navigators; answers whether a navigator took it) |
| Components | `list_components` (mounted instances), `get_component {instance_id}` (live props), `invoke_method {instance_id, method, args}` |
| Signals | `list_watched_signals`, `read_signal`, `write_signal` (`watch_signal_writable` only), `get_signal_history` |
| Functions | `list_functions`, `call_function`, `poll_function` (above) |
| App | `get_logs`, `clear_logs`, `get_identity`, `ping` |

### Screenshots on web

`screenshot` takes a `source`; a web app answers three, and names the one it
used in the result:

- `dom` — `backend-web/src/robot_screenshot.rs` copies the DOM (live input
  state, scroll offsets and 2D canvas pixels included) into an SVG
  `<foreignObject>` and rasterizes it. No user step; WebGL, plugin content
  (a PDF viewer) and exact font rendering can differ.
- `tab` — a frame of the tab's screen-share stream (`dev-http`'s
  `tab_capture.js`). A browser opens `getDisplayMedia` only from a click,
  so the user starts it from the dev overlay's "Share this tab with the
  agent" button; until then `tab` errors and the button lights up. Only a
  browser tab — this one, where capture handles can tell — is accepted.
- `cdp` — `robot_relay::cdp`: the relay captures the driven tab with
  `Page.captureScreenshot`, for a tab in a Chromium the dev session
  launched with `--remote-debugging-port` (the headless web client). It
  finds the tab by the nonce the page sent in its `hello` and left on
  `window.__idealyst_robot_nonce`.
- `auto` — `cdp` if reachable, else `tab` if shared, else `dom`.

## Clients

### robot-test (`#[robot_test]`)

Host-side tests over the bridge (`crates/dev/robot-test`), run by
`idealyst test`. Playwright-shaped, and **auto-waiting**: an action waits for
its element to exist and accept it (a disabled control refuses, so it waits
for it to be enabled), an assertion retries until it holds — both for up to
`App::set_timeout` (5 s), then panic with the last thing they saw. A test
never sleeps for a reactive update, a load or a navigation to land.

```rust
#[robot_test]
fn sign_in(app: &mut App) {
    app.navigate("/login");
    app.test_id("email").fill("me@example.com");
    app.test_id("submit").assert_disabled();     // waits for the rule to apply
    app.test_id("password").fill("hunter2");
    app.test_id("submit").click();               // waits for it to be enabled
    app.assert_path("/home");
    app.role("Toggle").nth(0).assert_checked(false);
}
```

- Locate: `test_id`, `text` (label substring), `label` (exact), `role` (kind),
  then `.nth(i)`.
- Act: `click` (native tap first, see above), `tap_native`,
  `dispatch_click`, `fill` / `type_text`, `set_toggle`, `set_slider`,
  `focus`, `blur`, `scroll_to`, `drag`, `drag_with_hold`, `drag_native`.
- Read: `state()` ([`ElementState`]), `is_visible()`, `label()`, `count()`.
- Assert: `assert_visible`, `assert_hidden`, `assert_in_viewport`, `assert_text`,
  `assert_text_contains`, `assert_value`, `assert_checked`,
  `assert_enabled`, `assert_disabled`, `assert_focused`, `assert_count`.
- App: `navigate` / `navigate_with` / `back` / `assert_path` /
  `navigators()`, `open_link(url)`, `call_function(name, args)` /
  `try_call_function` / `functions()` (waits for the function to register
  and for an async one to finish), `component(name).invoke(method, args)` / `.prop(name)`,
  `signal(name).assert_eq` / `.value()` / `.set(v)`, `logs()`,
  `screenshot()`, and `call(verb, args)` for any other verb.

`assert_visible` needs the element mounted and `visible`; `assert_hidden`
passes when nothing matches or the match is not visible (a closed overlay,
a navigated-away screen).

### Running a suite on each platform

`idealyst test --web | --macos | --ios | --android | --linux | --windows
<project>` builds and launches the app (`idealyst dev --local`), waits for
THIS project's app to register — a registration from another project or an
earlier run is ignored — and runs `cargo test --test robot` against it. The
dev session's output goes to a log file named on failure. `--windows` runs
on a remote machine with `IDEALYST_WINDOWS_REMOTE` (docs/windows.md);
`--linux` needs a Linux host. `--attach` drives an app already running.

The same suite passes unchanged on web, macOS, iOS (simulator), Android
(emulator) and Windows — `crates/dev/robot-e2e/examples/robot-e2e-demo`.

An app with an in-app self-test should stand it down when a driver owns
it: `runtime_vocabulary::robot::is_test_driven()`. `idealyst test` sets
`IDEALYST_TEST_DRIVER`, which the tooling carries to every platform — a
child process inherits it; Android gets it through the manifest
(`IdealystEnv`), iOS through `simctl`, a remote Windows app through its
launch environment, and a web page as a global the dev server injects.

### MCP server

Every verb is reachable: dedicated tools for the element verbs (`find_element`, `click`,
`tap_native`, `drag_native`, `get_state`, `set_scroll`, …), `list_navigators` / `get_navigator_state` /
`navigate`, `list_watched_signals` / `read_signal` / `write_signal`,
`list_component_instances` / `get_component_instance` / `invoke_method`
(`list_components` is the *catalog*), `open_link`, `list_functions` /
`call_function` (which polls an async function to completion), and
`call_verb` for any verb without
its own tool (`get_device_frame`, `get_signal_history`, an SDK's custom verb).
