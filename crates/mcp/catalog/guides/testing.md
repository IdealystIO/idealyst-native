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

idea-ui components take the same id as `test_id = Some("…")` and put it on
the element a test acts on: the text input of `Field`, `Textarea`,
`Autocomplete`, `DateInput`, `DateTimeInput` and `TimeInput`; the pressable
of `Button` (in every state, including while `loading`), `IconButton`,
`Switch`, `Checkbox`, `Radio`, `Link`, `Chip`, `ListItem`, `MenuItem`,
`Collapsible`'s header, and the trigger of `Select`, `SubMenu`, `DatePicker`,
`DateTimePicker` and `DateRangePicker`; `Tag`'s close button; `Slider`'s
drag area. Rows inside a component take theirs on the row's data:
`SelectOption`, `SegmentOption`, `Tab`, `RadioOption` and `MenuEntry` each
have `.test_id("…")`. No idea-ui feature is needed.

```rust
ui! {
    Field(label = Some("Email".into()), value = email, test_id = Some("email"))
    Button(label = "Sign in", on_click = submit, loading = rx!(busy.get()), test_id = Some("submit"))
}
```

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

A busy app is "not yet" too. When the app is slow to answer a lookup (the
error reads `app response timed out: …` — the relay is up, the app is busy)
or no reply arrives within `App::set_io_timeout` (8 s by default; the error
says the client gave up waiting on the relay), locating, assertions,
`count()`, `label()` and `assert_path` ask again until the timeout. An
action the app did not answer is not sent again, because the app may still
perform it: it fails at once. Raise `set_io_timeout` if an action keeps the
app busy for long on purpose.

- **Locate:** `test_id`, `text` (label substring), `label` (exact), `role`
  (element kind, e.g. `"Toggle"`), then narrow with `.with_label(..)`,
  `.with_text(..)`, `.with_role(..)`, `.with_test_id(..)`,
  `.within(container)` and `.nth(i)`. Every filter must hold, and the
  whole chain is one query to the app:

  ```rust
  use robot_test::Selector;

  // The Email field, not its "Email" caption, and only the one in the dialog.
  app.role("TextInput").with_label("Email").within(Selector::test_id("sign-in")).fill("me@example.com");

  // A container kept in a variable: detach it with `.selector()`.
  let row = app.role("View").with_text("Invoice 42").selector();
  app.role("Button").with_label("Delete").within(row).click();
  ```
- **Act:**
  - `click`, `fill` / `type_text`, `set_toggle`, `set_slider`, `focus`,
    `blur`, `scroll_to`, `scroll_into_view`;
  - `drag`, `drag_with_hold`, `drag_native`;
  - `tap_native`, `dispatch_click`;
  - `press_key("Enter")` (focus the element, then press), and
    `app.press_key("ControlOrMeta+K")` to whatever has focus. See below.
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
  - platform: `platform()` (`Platform::Ios`, …; `is_mobile()`,
    `is_desktop()`, `is_native()`) — for a check one platform does
    differently. A test that only applies on some platforms declares them
    instead (below);
  - viewport: `viewport()` (width, height, breakpoint), `set_viewport(w, h)`
    on desktop apps (it panics elsewhere; `try_set_viewport` returns why, so
    a test can skip — web sizes come from the runner's `--viewport`, phones'
    from the device);
  - diagnostics: `logs()`, `screenshot()`;
  - anything else: `call(verb, args)`.

A test that only means something on some platforms (a numeric keypad, a
safe-area inset, a native list cell) says which, and is never reported
passed anywhere else:

```rust
#[robot_test(platforms = [ios, android])]   // or `platforms = native`
fn amount_field_opens_the_number_pad(app: &mut App) { /* … */ }
```

Names: `web`, `ios`, `android`, `macos`, `linux`, `windows`; groups
`mobile`, `desktop`, `native`. Don't gate with an early `return` on
`app.platform()`: that test shows as passed where it never ran. A declared
test is named `<fn>::robot_only_ios_android` and `#[ignore]`d with the
reason; `idealyst test` runs it where it belongs and counts it `filtered
out` (or leaves it `ignored`) elsewhere, and a bare `cargo test` shows it
`ignored`. Its body sits in a module that imports the file's items with
`use super::*`, so write `crate::…` paths, not `super::…`.

Keep tests independent of run order. Each test should be the only one
mutating the state it asserts, and a robot function that resets the state
is the cheap way to get there — wrapped in a fixture, so no test forgets it.

### Fixtures: per-test setup

A fixture is setup a test asks for by type. Implement `robot_test::Fixture`
and name it as a parameter after the app, by `&` (or `&mut`): its `setup`
runs before the body, its `teardown` after it — also when the test fails.
Several run in parameter order and tear down in reverse; one can build on
another by calling its `setup`.

```rust
use robot_test::{json, robot_test, App, Fixture};

/// Signed in as a known user.
struct SignedIn { email: String }

impl Fixture for SignedIn {
    fn setup(app: &mut App) -> Self {
        let email = "ada@example.com".to_string();
        app.call_function("test.sign_in", json!({ "email": email }));
        SignedIn { email }
    }

    fn teardown(&mut self, app: &mut App) {
        app.call_function("test.sign_out", json!({}));
    }
}

#[robot_test]
fn shows_the_account(app: &mut App, user: &SignedIn) {
    app.navigate("/account");
    app.test_id("account-email").assert_text(&user.email);
}
```

**Recipe: fresh server state per test in a full-stack app.** Give the
server a reset it only answers in a test run, call it from a `#[robot_fn]`
in the app, and wrap that in a fixture.

```rust
// Shared code (the app crate): a #[server] fn whose body runs on the server.
#[server]
pub async fn reset_test_data() -> Result<(), ServerError> {
    // Refuse outside a test run: the server only has this variable when
    // the test runner's environment option sets it.
    if std::env::var_os("APP_TEST_RESET").is_none() {
        return Err(ServerError::failed("reset is test-only"));
    }
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    state.db.truncate_all().await.map_err(|e| ServerError::failed(e.to_string()))?;
    state.db.seed_fixtures().await.map_err(|e| ServerError::failed(e.to_string()))
}

// The app, registered at boot (inert outside robot builds):
/// Wipe and reseed the server's data.
#[robot_fn(name = "test.reset")]
async fn test_reset() -> Result<(), String> {
    reset_test_data().await.map_err(|e| e.to_string())
}
// runtime_vocabulary::robot_fn::register(test_reset::robot());
```

```rust
// tests/robot.rs
struct FreshServer;

impl Fixture for FreshServer {
    fn setup(app: &mut App) -> Self {
        app.call_function("test.reset", json!({}));
        FreshServer
    }
}

#[robot_test]
fn starts_with_the_seeded_todos(app: &mut App, _: &FreshServer) {
    app.test_id("todo-row").assert_count(3);
}
```

Run it with the variable set for the server, and anything the server needs
started first, through the runner's options (below):
`idealyst test --web --env APP_TEST_RESET=1 --before "docker compose up -d db" <project>`.
`call_function` waits for the async reset to finish before the test body
runs. Point the server at a test database, never the one you develop
against.

## Clicks: in-framework vs real input

| | What runs | Use it to test |
|---|---|---|
| `click` (robot) / `dispatch_click()` | The element's press handler, or a synthetic tap to the nearest `on_touch`, inside the framework | App logic |
| `tap_native` / `drag_native` | Real platform input at the element's center, through the toolkit's event path (hit testing, overlays, gesture recognizers, the native disabled state) | That a user's tap actually lands |

`Locator::click()` in robot-test is **native-first**. It waits for the
element to be visible and enabled, scrolls it into view if it is out of it,
then calls `tap_native`, falling back to the robot's `click` only where the
backend answers `unsupported`.

Scrolling works like Playwright's: every scroll view the element sits in,
nested ones and either axis, moves the least that shows the whole element,
and nothing moves when it is already in view. `click()` and `tap_native()`
do it before tapping an element outside the window, and again when the tap
lands on a neighbour because the element's scroll view clips it — a phone's
tab strip with its last tabs off to the right needs no manual scrolling.
`.no_scroll()` on the locator, or `app.set_auto_scroll(false)`, taps where
the element is; `scroll_into_view()` scrolls without tapping. Only mounted
elements can be reached: a virtualized list's off-screen rows don't exist
yet, so scroll the list itself (an app `#[method]` or robot function over
its `scroll_to_index`).

| Backend | `tap_native` mechanism |
|---|---|
| Web | DOM pointer and mouse events at `elementFromPoint`; on a page with touch input (`--device`, `--mobile`), touch events too, with the mouse events and click after the finger lifts |
| macOS | `NSEvent`s posted to the app |
| Android | `MotionEvent`s to the decor view (no hit test, so the injected event is the whole check) |
| Windows | Window messages posted to the child window a real click lands on |
| iOS | `hit_test`: UIKit's hit test checks the point, then the press runs |
| Linux | `unsupported`, so `click()` falls back |

A native tap refuses the way a finger would fail, with a retryable error:
- the element is not visible;
- its center is outside the window (`click()` scrolls it into view first);
- something covers it. The error names the element on top.

That last case is how a test catches an overlay or modal backdrop
swallowing taps. An `overlay` with a `Dismiss` or `Opaque` backdrop covers
the whole window on every backend. With `backdrop = None`, only the
overlay's own content takes taps and the page stays usable.

## Keys

`app.test_id("password").press_key("Enter")` focuses the field and presses
Enter, so a sign-in form submits the way a user's Enter submits it (the
field's `on_key_down` sees `"Enter"`). `app.press_key("Escape")` and
`app.press_key("ControlOrMeta+K")` go to whatever has focus, or the window.
Key names are Playwright's (`Enter`, `Escape`, `Tab`, `Backspace`,
`ArrowDown`, `a`, `Shift+Tab`, `Meta+K`); `ControlOrMeta` is Command on
Apple platforms and Control elsewhere. Both return the mechanism:
`"native"` on web, macOS, Windows and Android (real key events),
`"dispatch"` on iOS and Linux, where the framework's key paths run instead
(the focused field's `on_key_down`, `on_key` listeners, then the field's
text edit). Either way Backspace deletes and an unconsumed Escape closes the
topmost overlay with an `on_dismiss`; Tab moves focus only on `"native"`.

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
- builds the `tests/robot.rs` target;
- runs the `--before` hooks;
- builds the app fresh and launches it (`idealyst dev --local`) on a free
  port, so a full-stack app's server is the run's own, never the one your
  `idealyst dev` is using;
- runs that session as `idealyst dev --isolated test`: its build dirs,
  log, lock and relay files are its own, so it runs beside your
  `idealyst dev` of the same project, and its build cache stays warm
  between runs. `--isolated <key>` picks another key, `--no-isolated`
  turns it off. A `--web` run of a project without a server can't be
  isolated (that shape stages into the project's own `pkg/`), so it runs
  unisolated;
- marks it test-driven, which is what `is_test_driven()` reads;
- waits for *this* project's app to register;
- runs the suite against it, once per size, device or simulator;
- runs the `--after` hooks, and prints a result per run.

`--filter <name>` narrows the run, `--test <file>` picks another test
target, and `--attach` drives an app you already started with
`idealyst dev`. Under a bare `cargo test`, with no app reachable, each
`#[robot_test]` skips instead of failing. The runner asks the app its
platform and runs only the `platforms = …` tests that belong to it, naming
the rest on a `[test] not on <platform>: …` line.

### Sizes, devices and simulators

```text
idealyst test --web --viewport 390x844,820x1180,1440x900 <project>
idealyst test --web --device iphone-14,ipad-air          <project>
idealyst test --web --mobile --viewport 360x740          <project>
idealyst test --macos --viewport 800x600,1440x900        <project>
idealyst test --ios --simulator "iPhone 15,iPad Air 11-inch (M2)" <project>
```

Each size, device or simulator is one full run of the suite, and the run
fails if any of them fails.
- **Web** builds and launches once and resizes the page between sizes.
  The size is exact: `--viewport 390x844` gives `innerWidth` 390.
- **`--device <name>`** (web) emulates a phone or tablet: its size, pixel
  ratio, touch input and user agent. Presets: `iphone-se`, `iphone-14`,
  `iphone-14-pro-max`, `iphone-15`, `pixel-7`, `galaxy-s23`, `ipad-mini`,
  `ipad-air`, `ipad-pro-12`. `--mobile` does the same at each `--viewport`
  size, at 3x. With touch input, `tap_native` taps with touch events.
- **Desktop apps** (`--macos`, `--linux`, `--windows`) relaunch at each
  size.
- **iOS and Android** use the device's own screen. `--simulator` (iOS)
  and `--emulator` (Android, an AVD name or adb serial) run on each named
  device in turn. Without them, iOS uses the first booted simulator and
  Android the first online device.

A test can branch on what it runs at:

| Variable | Value |
|---|---|
| `IDEALYST_TEST_VIEWPORT` | `390x844` (web and desktop) |
| `IDEALYST_TEST_DEVICE` | the `--device` preset, `--simulator` or `--emulator` name |
| `IDEALYST_TEST_PORT` | the port the run's server listens on |

### Services and environment

```text
idealyst test --web \
  --before "docker compose up -d db" \
  --env DATABASE_URL=postgres://localhost:5433/test \
  --after "docker compose down" <project>
```

- `--env KEY=VALUE` (repeatable) reaches the app on every platform
  (passed as `idealyst dev --app-env`: a desktop app's environment, an iOS
  simulator app's, an Android app's `IdealystEnv`, a web page's
  `window.IDEALYST_APP_ENV`); app code reads it on any platform with
  `runtime_core::app_env("KEY")`. It is also set for the dev session, so a
  full-stack app's server sees it, and for the tests and hooks.
- `--before <cmd>` runs in the project directory before the app launches.
  It must exit, so a service it starts has to detach. A non-zero exit
  stops the run before anything launches.
- `--after <cmd>` runs after the suite, pass or fail. A non-zero exit
  fails the run.
- Hooks see the `--env` values and `IDEALYST_TEST_PORT`. `--port <n>`
  picks the port instead of a free one.

Setup inside the suite, such as seeding data through a robot function,
belongs in the tests. The hooks are for what must exist before the app
starts.

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
