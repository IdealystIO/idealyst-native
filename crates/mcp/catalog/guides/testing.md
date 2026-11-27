+++
title = "Testing apps (robot E2E)"
order = 62
tags = ["testing", "e2e", "robot", "idea_test", "robot_test", "suites", "workers", "parallel", "robot_fn", "tap_native", "call_function", "idealyst test", "mcp", "device matrix", "orientation", "firefox", "webkit", "clock", "net"]
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
| `file-picker` | `file_picker::register_robot_controls()` | `file_picker.queue {files: [{name, mime?, base64? \| text?}]}`, `file_picker.cancel_next`, `file_picker.requests`, `file_picker.clear` |
| `camera` | `camera::register_robot_controls()` | `camera.fake {width, height, rgba}` (every `open` streams it), `camera.clear`, `camera.status` |

A fake replaces the platform for the SDK's whole surface until it is
cleared. For example, after `location.set`, `current()` resolves the fix
with no permission prompt, every live `watch` receives it, and the
device's own updates are dropped. Give your own SDK the same shape: its
fakes go in `runtime_vocabulary::robot_only! { … }` (items only robot
builds compile) or `robot_select! { on: { … } off: { … } }` (one set of
items per build), so a release build carries none of them.

Two fakes are in every robot build, with nothing to register:

- **The clock** (`clock.set`, `clock.advance`, `clock.freeze`,
  `clock.zone`, `clock.clear`, `clock.now`): the app's wall clock, both
  the framework's (`epoch_millis`, a date picker's "today") and the
  `datetime` SDK's. Set it to the moment a time rule applies instead of
  waiting for it. Timers and the monotonic clock keep real time, and the
  app's server keeps its own clock.
- **The network** (`net.intercept`, `net.held`, `net.release`,
  `net.remove`, `net.clear`): hold, delay or fail the app's HTTP requests
  by URL (`*` matches any run), server functions (`/_srv/<name>`)
  included, to test what the UI does while a request is in flight or
  after it fails. WebSockets and SSE are not matched.

- **External URLs** (`urls.capture`, `urls.opened`,
  `urls.opened_in_place`, `urls.clear`): the harness turns capture on
  before each test, so `open_url` and `open_url_in_place` record the URL
  instead of leaving the app. A phone app that leaves is suspended, and
  every call after that times out with "the app is in the background".
  Assert on what it opened with `app.opened_urls()` /
  `app.wait_opened_url(".pdf")`; `app.opened_urls_in_place()` lists the
  ones that left in place (a hosted checkout in the app's own tab).

robot-test wraps the fakes:

```rust
app.clock().set_utc("2026-11-17T22:30:00Z").freeze();
app.clock().advance(Duration::from_secs(2 * 3600));
let search = app.net().hold("/_srv/*search");
app.net().wait_held(search);                  // the request is in flight
app.net().release(search);
app.net().fail_status("/_srv/save", 503, "down");
app.file_picker().queue_files(["tests/fixtures/template.pdf"]);
app.camera().fake_image("tests/fixtures/face.jpg");
```

Clear what a test set (`app.clock().clear()`, `app.net().clear()`) so the
next test starts from the real device.

Deep links need no SDK: `open_link {url}` delivers a URL as the OS would,
to observers, interceptors and then the navigators. It opens the whole
path the way a cold load of that URL does: a nested navigator that mounts
only later (inside a `#[component(lazy)]` screen, or once its screen's
data arrives) is moved as soon as it is up, and `App::open_link` waits
for that — then fails naming the path it reached if a segment matches no
screen.

## 3. Write the suite

Host-side tests live in `tests/robot.rs`, with `robot-test` as a
dev-dependency (`robot-test = { version = "3.3", registry = "idealyst" }`,
or `{ workspace = true }` inside the framework workspace):

```rust
use robot_test::{json, idea_test, App};

#[idea_test]
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
error reads `app response timed out: …` — the relay is up, the app is busy;
a timeout that says the app is in the background means it left the screen
and won't answer)
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
  - `context_click()` (a right click on web and desktop, a long press on
    iOS and Android, so it opens a row's context menu the platform's
    way), `long_press(hold)` (`robot_test::LONG_PRESS` is a safe hold),
    `secondary_click()` (always the right click);
  - `press_key("Enter")` (focus the element, then press), and
    `app.press_key("ControlOrMeta+K")` to whatever has focus. See below.
- **Read:** `state()`, `is_visible()`, `label()`, `count()`, `keyboard()`
  (a text field's soft-keyboard kind, `"email"`, …), `style()` (the
  resolved style as a typed `Style`: color, weight, `text_decoration`, …).
- **Assert:**
  - presence: `assert_visible`, `assert_hidden` (passes when nothing
    matches), `assert_in_viewport`, `assert_count`;
  - content: `assert_text`, `assert_text_contains`, `assert_value`;
  - state: `assert_checked`, `assert_enabled`, `assert_disabled`,
    `assert_focused`, `assert_keyboard("email")`;
  - style: `assert_style(|s| s.is_line_through())` (waits, like every
    assertion).
- **App:**
  - navigation: `navigate` / `navigate_with` / `back`, `assert_path`,
    `assert_query(key, value)` (screen state carried in the URL's query,
    e.g. a filter) / `assert_href(href)`, `navigators()` (with `query` and
    `href`), `open_link(url)` / `try_open_link(url)`;
  - booting: `load_url(url)` / `try_load_url(url)` starts the app over
    at `url` (the page reloads there on web; Android relaunches with the
    link) and waits for it to answer;
  - cookies (web): `set_cookie(Cookie::new("device", "k1").http_only())`,
    `cookies()`, `clear_cookie(name)`, `clear_cookies()`. A cookie set
    before `load_url` reaches the server on the boot request;
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
#[idea_test(platforms = [ios, android])]   // or `platforms = native`
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

### Suites

Group tests into named suites to run a slice of the file — a quick smoke
pass, one feature's tests:

```rust
#[idea_test(suites = smoke)]
fn the_home_screen_loads(app: &mut App) { /* … */ }

#[idea_test(suites = [smoke, checkout], platforms = mobile)]
fn paying_opens_the_number_pad(app: &mut App) { /* … */ }
```

```text
idealyst test --web --suite smoke <project>            # a suite
idealyst test --web --suite smoke,checkout <project>   # any of several
idealyst test --list-suites <project>                  # each suite and its tests
```

Names are lowercase words joined by single `_`. A suite is a label: a run
without `--suite` runs every test. `--suite` combines with `--filter` and
the platform rules; a name no test declares fails the run with the list of
suites. A suite member's body sits in a module like a platform-bound
test's (`<fn>::idea_suites_checkout__smoke`), so write `crate::…` paths.

Keep tests independent of run order. Each test should be the only one
mutating the state it asserts, and a robot function that resets the state
is the cheap way to get there — wrapped in a fixture, so no test forgets it.

### The suite in a package of its own

An integration test of the app package builds the app's whole library for
the host: a suite that only needs `robot-test` then pays a host build of
every UI crate, and a host rebuild after each app edit. Put the suite in a
workspace package of its own (one that does not depend on the app) and
point the app at it:

```toml
# the app's Cargo.toml
[package.metadata.idealyst.test]
package = "my-app-e2e"   # whose tests/<test>.rs is the suite
test = "main"            # the target (default `robot`)

# my-app-e2e/Cargo.toml — what a bare `cargo test -p my-app-e2e` drives
[package.metadata.idealyst.test]
app = "../app"           # or { main = "../app", checkin = "../app-checkin" }
```

`--test-package` / `--test` override the app's settings for one run. The
suite runs in its package's directory, which is also its
`CARGO_MANIFEST_DIR`, as under `cargo test`. The suite package's `app`
is for a bare `cargo test` only: it picks the running app whose project is
that path (the runner hands its suite the app's address itself).

### Device classes: phone, tablet, desktop

Target a kind of device rather than a platform. `form` is the app's own
breakpoint (`phone` xs/sm, `tablet` md, `desktop` lg/xl); `input` is the
primary pointer (`touch` coarse, `pointer` fine):

```rust
#[idea_test(form = phone)]
fn the_tab_bar_is_at_the_bottom(app: &mut App) { … }

#[idea_test(form = [phone, tablet], input = touch)]
fn a_long_press_opens_the_menu(app: &mut App) { … }
```

The class is checked against what the app reports when the test runs, so
one test covers an iPhone, a Pixel and a phone-sized browser. Where it
doesn't match, the test is listed as `n/a` with the reason (`needs phone;
the app is a tablet (md, 820x1180, touch)`), never as a pass.
`app.device_class()` reads it directly. Prefer a class to `platforms = …`:
keep platform lists for what is genuinely one platform's (a native
keyboard, a browser cookie).

### Changing the screen

A resize or a rotation reflows the app, and that reflow is its own place
for bugs. A test does it as a step:

```rust
app.set_orientation(Orientation::Landscape);   // waits until the app has turned
app.test_id("panel").assert_visible();
app.set_viewport(1100.0, 900.0);               // desktop window, web page
```

- `set_orientation` turns the screen as a person turns the device. An
  iPhone app turns itself; under `idealyst test` the runner turns the
  rest: an Android display (adb), an iPad simulator (Simulator.app), a web
  page (width and height swapped). A desktop window has no orientation.
- `set_viewport` resizes a desktop window or a web page. A phone or
  tablet's size is the device's.
- `try_set_orientation` / `try_set_viewport` return why instead of
  panicking.
- Whatever a test changed is put back after it, also after a failure, so
  the next test starts on the run's own screen. Don't restore it by hand.

### Two apps in one test

`app.open_page(url)` opens another tab of the same web app with its own
cookies (a second user signed in beside the first; `open_page_shared`
shares them), `App::connect_project(dir)` reaches the running app of
another project (a kiosk beside the main app), and `app.apps()` /
`select_app(id)` pick among the apps on one relay. Each returned `App` is
driven like the first.

### Fixtures: per-test setup

A fixture is setup a test asks for by type. Implement `robot_test::Fixture`
and name it as a parameter after the app, by `&` (or `&mut`): its `setup`
runs before the body, its `teardown` after it — also when the test fails.
Several run in parameter order and tear down in reverse; one can build on
another by calling its `setup`.

```rust
use robot_test::{json, idea_test, App, Fixture};

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

#[idea_test]
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

#[idea_test]
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
`long_press`, `context_click`, `press_native`, `hover_native` and
`drag_native` scroll the same way first. `.no_scroll()` on the locator, or
`app.set_auto_scroll(false)`, acts where the element is;
`scroll_into_view()` scrolls without tapping. On the web the page itself
scrolls too, when the app's root is taller than the window and no
`scroll_view` holds it; a native window doesn't scroll, so a screen taller
than a phone's window needs a `scroll_view` for what is below the fold to
be reachable — by a user or a test. Only mounted
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

A context menu is a right click with a mouse and a long press with a
finger. Write the row's `on_touch` to take both (a press whose
`pointer_button().opens_context_menu()` opens the menu, any other press
goes to a `long_press` recognizer) and test it with `context_click()`. On
iOS and Linux, which can't inject input, the robot holds the press (and
sends the right click) through the framework's own touch handlers.

A native tap refuses the way a finger would fail, with a retryable error:
- the element is not visible;
- its center is outside the window, or a scroller clips it away (a sheet's
  button below the fold). `click()` scrolls it into view first;
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
text edit). Either way Backspace deletes, an unconsumed Escape closes the
topmost overlay with an `on_dismiss`, and Tab / Shift+Tab move focus to the
next / previous field in the platform's own order — assert it with
`assert_focused`. Which controls are Tab stops is the platform's: a button
is one on web, never on iOS.

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
- builds that tree once and tests it to the end (`--no-watch`): a save
  during the run, yours or another session's in a shared checkout, does
  not rebuild or reload the app under the suite;
- runs that session as `idealyst dev --isolated test`: its build dirs,
  log, lock and relay files are its own, so it runs beside your
  `idealyst dev` of the same project, and its build cache stays warm
  between runs. `--isolated <key>` picks another key, `--no-isolated`
  turns it off. A `--web` run of a project without a server can't be
  isolated (that shape stages into the project's own `pkg/`), so it runs
  unisolated;
- runs one session per key: while another run (or an `idealyst dev
  --isolated test`) holds the key, the run is refused before anything
  builds, naming that session's pid, who started it and when. To run in
  parallel — a CI matrix, several agents — give each run its own key, or
  pass `--isolated auto` to take the first free one of `test`, `test-2`,
  … (each key has its own build cache, so a key's first run builds cold).
  A run that crashed doesn't hold its key: its process is gone;
- marks it test-driven, which is what `is_test_driven()` reads;
- waits for *this* project's app to register;
- runs the suite against it, once per size, device or simulator;
- runs the `--after` hooks, and prints a result per run.

`--filter <name>` narrows the run, `--suite <names>` runs only those
suites (`--list-suites` lists them), `--workers N` runs N copies at once
(below), `--test <file>` picks another test target, and `--attach` drives an app you already started with
`idealyst dev` (never the app of another `idealyst test` run). Under a bare `cargo test`, with no app reachable, each
`#[idea_test]` skips instead of failing. The runner asks the app its
platform and runs only the `platforms = …` tests that belong to it, naming
the rest on a `[test] not on <platform>: …` line.

### Parallel workers

`idealyst test --web --workers 4 <project>` runs the suite on four copies
of the app at once, each test on whichever copy is free, with one report.
The app builds once; each worker has its own port, robot relay, browser
and, for a full-stack project, its own server. Give each worker's server
its own data with `{worker}` in an `--env` value, and create it in a
`--before` hook, which runs once per worker:

```text
idealyst test --web --workers 4 \
  --env 'DATABASE_URL=postgres://app:app@db/app_robot_{worker}' \
  --before 'scripts/reset-db.sh "$DATABASE_URL"' <project>
```

Tests and hooks see the worker's number as `IDEALYST_TEST_WORKER` (from
0; a plain run is worker 0). Each test runs in its own process, so tests
can't share state through the test binary — they shouldn't anyway. Web
only: a native app is one per device (use the device matrix with `--jobs`).

### The device matrix

Declare once which device each class runs as on each platform, and a bare
`idealyst test <project>` runs every class on every platform this machine
can:

```toml
[package.metadata.idealyst.test.devices]
phone   = { web = "chromium:iphone-15", ios = "iPhone 17 @ iOS 26", android = "pixel_7 @ 34" }
tablet  = { web = "firefox:ipad-air",   ios = "iPad Air 11-inch (M3) @ iOS 26", android = "pixel_tablet @ 34" }
desktop = { web = "1280x800", macos = "1280x800", linux = "1280x800", windows = "1280x800" }
```

- `web`: `[engine:]preset` or `[engine:]WxH`. Engines: `chromium` (the
  default, everything emulated), `firefox` (headless, WebDriver BiDi, no
  geckodriver) and `webkit` (Safari through `safaridriver`, macOS only, a
  real window, size only; `safaridriver --enable` once).
- `ios`: a simulator name, `@ iOS <version>` to pin the runtime.
- `android`: `<hardware profile> @ <API level>` (runs on the runner's own
  `idealyst_<profile>_<api>` AVD, made when missing, never on another AVD
  that shares the profile), an AVD name, or `serial:<adb serial>`.
- `macos` / `linux` / `windows`: the window size.

Before building, the runner prints the plan: each class × platform runs,
or is skipped with the reason (not a target in
`[package.metadata.idealyst.app] targets`, the wrong host, a runtime or
system image not installed with the command that installs it, the wrong
CPU architecture). The reasons repeat in the summary. `--plan` prints it
and exits. A simulator or AVD that doesn't exist is created from what is
installed (`--no-create` reports it instead), and a device the run booted
is shut down after its row (`--keep-devices` leaves it). Phone and tablet
rows start upright, whatever the device booted in.

```text
idealyst test <project>                            # the whole matrix
idealyst test --devices phone --platforms ios,web <project>
idealyst test --plan <project>                     # what would run, and why not
idealyst test --junit results.xml <project>        # a suite per run, a case per test
idealyst test --jobs 2 <project>                   # two platforms at once
```

`--jobs N` runs N platforms at once (each on its own port and isolation
key, its suites' output in a log file the run names). Builds and
emulators are heavy: raise it only on a machine with room, and never
several Android emulators on a laptop. Tests see their class as
`IDEALYST_TEST_CLASS`, their platform as `IDEALYST_TEST_PLATFORM`, and a
web row's engine as `IDEALYST_TEST_ENGINE` (set on web rows only). A
native row waits for the app's build however long it takes; the
readiness wait after it covers only launching and connecting.

The flags below replace the matrix for one run.

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

The tools reach the same verbs, so an agent can try a flow before writing
it down:

1. `list_apps`: what is running.
2. `find_element {test_id}` → an `element_id`, with `visible`,
   `in_viewport` and `state`; `mode: "all"` or `"count"` for every match.
3. `inspect_element {element_id, what}` (`state`, `frame`, `children`,
   `native`, …), `get_snapshot`, `screenshot`, `get_logs`: read.
4. `click`, `type_text`, `set_toggle`, `set_slider`, `set_scroll`, and
   `pointer_native {element_id, action}` for real platform input (`tap`,
   `drag`, `hover`, …): act. `viewport {width, height}` resizes a
   desktop window and `viewport {orientation}` turns an iPhone app; the
   other screens are turned by the test runner (`set_orientation` in a
   suite).
5. `list_functions` / `call_function {name, args}`: the app's and SDKs'
   controls. `call_function` waits for an async function to finish.
6. `open_link {url}`, `list_navigators` / `navigate`,
   `list_watched_signals` / `read_signal` / `write_signal`.
7. `call_verb` for any verb without its own tool.

A behavior a test cannot drive or observe through these is a gap in the
robot, not something to work around with sleeps or platform checks.
