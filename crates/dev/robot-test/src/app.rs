//! The author-facing test surface: a relay-connected [`App`] handle and the
//! Playwright-flavoured [`Locator`] it hands out. Assertions **panic** on
//! failure (caught per-test by the libtest harness), so a `#[robot_test]` body
//! reads like an ordinary Rust test — no `Result`/`?` ceremony.
//!
//! Every call goes over the Robot bridge ([`RobotClient`]), so the same code
//! drives a web, macOS, iOS, Android, Linux or Windows app — whichever one the
//! bridge reaches.
//!
//! # Auto-waiting
//!
//! As in Playwright, nothing here asserts on the first look. An action waits
//! until its element exists and accepts it (a disabled control refuses, so an
//! action waits for it to be enabled); an assertion retries until it holds.
//! Both give up after the app's timeout ([`App::set_timeout`], 5 s by
//! default) and panic with the last thing they saw. So a test never sleeps
//! to let a reactive update, an async load or a navigation land: it states
//! what it expects and the robot waits for it.
//!
//! A busy app is "not yet" too. When the app is slow to answer a query (the
//! relay reports `app response timed out`, or no reply reaches the client
//! within [`App::set_io_timeout`]), or the relay has no app for a moment (a
//! reload), every waiting path — locating, assertions, `count`, `label`,
//! `assert_path` — asks again until its timeout, and only then fails, with
//! the last error. Actions are the exception: an action the app did not
//! answer may still run once the app is free, so it is never sent twice; it
//! fails at once, naming which side timed out.

use crate::client::{CallError, RobotClient};

mod keys;
pub use keys::parse_key;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How long actions and assertions wait by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`Locator::context_click`] holds a press on a touch platform:
/// past UIKit's 0.5 s `minimumPressDuration`, Android's long-press timeout
/// and the framework's `DEFAULT_LONG_PRESS_THRESHOLD_MS` (500 ms), with
/// room for a busy UI thread. A good default for [`Locator::long_press`].
pub const LONG_PRESS: Duration = Duration::from_millis(800);

/// How long [`App::load_url`] waits, at least, for the app to boot again:
/// a page reload fetches and starts the wasm bundle, a native relaunch
/// cold-starts the app; both take longer than an action's reply.
pub const LOAD_URL_TIMEOUT: Duration = Duration::from_secs(60);

/// How often a waiting action or assertion looks again.
const POLL: Duration = Duration::from_millis(50);

/// A connection to the running app under test — the handle a `#[robot_test]`
/// body receives. Locate elements with [`App::test_id`] / [`App::text`] /
/// [`App::label`] / [`App::role`]; drive navigation, components, signals and
/// robot functions ([`App::call_function`])
/// from here too.
pub struct App {
    client: RobotClient,
    timeout: Duration,
    /// Whether `click` / `tap_native` scroll the element into view first
    /// ([`App::set_auto_scroll`]).
    auto_scroll: bool,
    /// The app's platform, asked once ([`App::platform`]).
    platform: Option<Platform>,
}

impl App {
    /// Connect to an app's Robot bridge at `addr` (a direct bridge or, more
    /// usually, the relay's TCP port).
    pub fn connect(addr: SocketAddr) -> anyhow::Result<Self> {
        Ok(Self::from_client(RobotClient::connect(addr)?))
    }

    /// Wrap an already-connected client (used by the harness, which connects +
    /// waits for readiness before handing the app to tests).
    pub(crate) fn from_client(client: RobotClient) -> Self {
        Self { client, timeout: DEFAULT_TIMEOUT, auto_scroll: true, platform: None }
    }

    /// How long actions and assertions wait before failing.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// How long ONE bridge call waits for its reply (default
    /// [`DEFAULT_IO_TIMEOUT`](crate::DEFAULT_IO_TIMEOUT), 8 s) — the relay
    /// is asked to wait for the app a little less, so a busy app is
    /// reported as the app timing out, not the connection. A waiting query
    /// that times out is retried until [`set_timeout`](Self::set_timeout)'s
    /// deadline; an action is not (see the module docs), so raise this
    /// when an action legitimately keeps the app busy for long.
    pub fn set_io_timeout(&mut self, timeout: Duration) {
        self.client
            .set_io_timeout(timeout)
            .unwrap_or_else(|e| panic!("set_io_timeout: the connection refused it: {e}"));
    }

    /// Call any bridge verb and return its result — the escape hatch for a
    /// verb this API has no method for. Panics on a bridge error; see
    /// [`try_call`](Self::try_call) to handle one.
    pub fn call(&mut self, verb: &str, args: Value) -> Value {
        self.try_call(verb, args).unwrap_or_else(|e| panic!("{verb} failed: {e}"))
    }

    /// [`call`](Self::call), returning the bridge's error instead of panicking.
    pub fn try_call(&mut self, verb: &str, args: Value) -> anyhow::Result<Value> {
        self.client.call(verb, args)
    }

    // --- locating ---

    /// Locate by `test_id` — the durable, refactor-proof hook (`.test_id("inc")`
    /// in your component).
    pub fn test_id(&mut self, id: &str) -> Locator<'_> {
        Locator::new(self, Sel::TestId(id.into()))
    }

    /// Locate by visible text (substring match against the live label).
    pub fn text(&mut self, substring: &str) -> Locator<'_> {
        Locator::new(self, Sel::Text(substring.into()))
    }

    /// Locate by exact label (a button's text, a field's accessible name).
    pub fn label(&mut self, label: &str) -> Locator<'_> {
        Locator::new(self, Sel::Label(label.into()))
    }

    /// Locate by primitive kind, e.g. `"Button"` / `"TextInput"` (the
    /// PascalCase `ElementKind` names).
    pub fn role(&mut self, kind: &str) -> Locator<'_> {
        Locator::new(self, Sel::Role(kind.into()))
    }

    /// Locate by a detached [`Selector`] — one built without the app, e.g.
    /// kept in a variable or returned from a helper.
    pub fn locate(&mut self, selector: impl Into<Selector>) -> Locator<'_> {
        Locator { app: self, sel: selector.into(), no_scroll: false }
    }

    /// How many elements currently match `kind` (e.g. count every `Button`).
    /// Does not wait for a count, but asks again while the app is busy.
    pub fn count(&mut self, kind: &str) -> usize {
        let desc = format!("count({kind:?})");
        self.read(&desc, "count_elements", json!({ "kind": kind })).as_u64().unwrap_or(0) as usize
    }

    // --- viewport ---

    /// The app's viewport now: its size in logical px and the active
    /// breakpoint (`"xs"` … `"xl"`) — what the app's responsive UI sees.
    pub fn viewport(&mut self) -> Viewport {
        let v = self.read("viewport()", "get_viewport", json!({}));
        serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("get_viewport: unexpected shape {v} ({e})"))
    }

    /// Resize the app's window so its viewport is `width × height` logical
    /// px, and wait until the app reports that size. Desktop apps (macOS,
    /// Windows, Linux) resize their own window; elsewhere this panics with
    /// the reason — see [`try_set_viewport`](Self::try_set_viewport) to
    /// skip instead.
    pub fn set_viewport(&mut self, width: f64, height: f64) {
        self.try_set_viewport(width, height).unwrap_or_else(|e| panic!("set_viewport({width}, {height}): {e}"))
    }

    /// [`set_viewport`](Self::set_viewport), returning why it could not
    /// (unsupported on this platform, a maximized window) instead of
    /// panicking. Still panics if the app never reaches the size.
    pub fn try_set_viewport(&mut self, width: f64, height: f64) -> Result<(), String> {
        let desc = format!("set_viewport({width}, {height})");
        // Idempotent, so a busy app is asked again.
        self.wait(&desc, |app| match app.query("set_viewport", json!({ "width": width, "height": height })) {
            Ok(_) => Ok(Ok(())),
            Err(Failure::Fatal(e)) => Ok(Err(e)),
            Err(other) => Err(other),
        })?;
        self.wait(&desc, |app| {
            let v: Viewport = serde_json::from_value(app.query("get_viewport", json!({}))?)
                .map_err(|e| Failure::Fatal(format!("get_viewport: unexpected shape ({e})")))?;
            if (v.width - width).abs() < 0.5 && (v.height - height).abs() < 0.5 {
                Ok(())
            } else {
                Err(Failure::NotYet(format!("the viewport is {} × {}", v.width, v.height)))
            }
        });
        Ok(())
    }

    // --- platform ---

    /// The platform the app under test runs on — what `idealyst test
    /// --<platform>` launched, or whatever app the bridge reached. For a
    /// test that runs on every platform but checks something one of them
    /// does differently; a test that only means something on some
    /// platforms declares them instead (`#[robot_test(platforms = [ios,
    /// android])]`), so it is skipped, not passed, elsewhere. Pair with
    /// [`viewport`](Self::viewport)`().breakpoint` for the form factor.
    /// Panics if the app can't say (an app built before `get_platform`).
    pub fn platform(&mut self) -> Platform {
        self.try_platform()
            .unwrap_or_else(|e| panic!("platform(): {e}"))
            .unwrap_or_else(|| panic!("platform(): the app is too old to report its platform (no `get_platform` verb)"))
    }

    /// [`platform`](Self::platform), `Ok(None)` when the app is too old to
    /// report one. Asked once per connection.
    pub fn try_platform(&mut self) -> Result<Option<Platform>, String> {
        if let Some(p) = &self.platform {
            return Ok(Some(p.clone()));
        }
        // A read: asked again while the app is busy.
        let answer = self.wait("platform()", |app| match app.query("get_platform", json!({})) {
            Ok(v) => Ok(Ok(v)),
            Err(Failure::Fatal(e)) => Ok(Err(e)),
            Err(other) => Err(other),
        });
        let v = match answer {
            Ok(v) => v,
            Err(e) if e.contains("unknown command") => return Ok(None),
            Err(e) => return Err(e),
        };
        let name = v["platform"].as_str().ok_or_else(|| format!("get_platform: unexpected shape {v}"))?;
        let p = Platform::from_id(name);
        self.platform = Some(p.clone());
        Ok(Some(p))
    }

    /// Whether [`Locator::click`], [`Locator::tap_native`] and the other
    /// real-input actions (`long_press`, `context_click`, `press_native`,
    /// `hover_native`, `drag_native`) first scroll an element that is out
    /// of view into it, as Playwright does — on by default. Off, one on an
    /// element outside the window or clipped by its scroll view waits, then
    /// fails, as it would for a user who doesn't scroll. One locator opts
    /// out with [`Locator::no_scroll`].
    pub fn set_auto_scroll(&mut self, on: bool) {
        self.auto_scroll = on;
    }

    // --- signals ---

    /// Assert on, read or write a watched signal by name. The app must expose
    /// it via `runtime_core::robot::watch_signal("name", sig)` (or
    /// `watch_signal_writable` to write it).
    pub fn signal(&mut self, name: &str) -> SignalAssert<'_> {
        SignalAssert { app: self, name: name.into() }
    }

    // --- navigation ---

    /// Every navigator in the app, outermost first.
    pub fn navigators(&mut self) -> Vec<NavigatorInfo> {
        let v = self.read("navigators()", "list_navigators", json!({}));
        serde_json::from_value(v).unwrap_or_else(|e| panic!("list_navigators: unexpected shape ({e})"))
    }

    fn try_navigators(&mut self) -> Result<Vec<NavigatorInfo>, Failure> {
        let v = self.query("list_navigators", json!({}))?;
        serde_json::from_value(v).map_err(|e| Failure::Fatal(format!("list_navigators: unexpected shape ({e})")))
    }

    /// Push `path` on the current navigator (the innermost one that takes
    /// robot commands, as a deep link would land) and wait for it to show.
    pub fn navigate(&mut self, path: &str) {
        self.navigate_with("push", path);
    }

    /// [`navigate`](Self::navigate) with another action: `"push"`,
    /// `"replace"`, `"reset"` or `"pop"` (`path` ignored for a pop).
    pub fn navigate_with(&mut self, action: &str, path: &str) {
        let nav = self.current_navigator();
        self.call("navigate", json!({ "nav_id": nav.nav_id, "action": action, "path": path }));
    }

    /// Go back on the current navigator.
    pub fn back(&mut self) {
        self.navigate_with("pop", "");
    }

    /// Open `url` as if the OS handed it to the running app (a deep link
    /// or universal link): observers see it, an interceptor may claim it,
    /// otherwise the navigators move to it — and waits until the WHOLE
    /// path shows, every nested navigator included, as a cold load of the
    /// same URL would open it. A navigator that mounts late (inside a
    /// `#[component(lazy)]` screen, or once a screen's data arrives) is
    /// moved as soon as it is up.
    ///
    /// Returns `false` when no navigator took the link (an interceptor
    /// claimed it, or no screen matches); `true` once the full path shows.
    /// Panics, naming the path it did reach, when the link was taken but
    /// stops short within the timeout (a segment no screen matches). See
    /// [`try_open_link`](Self::try_open_link) for the outcome without the
    /// panic.
    pub fn open_link(&mut self, url: &str) -> bool {
        let outcome = self.try_open_link(url);
        if outcome.routed && !outcome.complete {
            panic!(
                "open_link({url:?}): reached {:?}, not the whole path, after {:?}",
                outcome.reached, self.timeout
            );
        }
        outcome.routed
    }

    /// [`open_link`](Self::open_link), returning how far the link got
    /// instead of panicking. Still waits up to the timeout for the path
    /// to complete. Sending the link is an action, so a busy app fails it
    /// at once (it is never delivered twice); carrying it on to
    /// navigators that mount later only routes again, which is safe to
    /// repeat.
    pub fn try_open_link(&mut self, url: &str) -> LinkOutcome {
        let first = self.call("open_link", json!({ "url": url }));
        let mut outcome = LinkOutcome::from_answer(&first)
            .unwrap_or_else(|| panic!("open_link: unexpected shape {first}"));
        if !outcome.routed || outcome.complete {
            return outcome;
        }
        let deadline = Instant::now() + self.timeout;
        while Instant::now() < deadline {
            std::thread::sleep(POLL);
            match self.query("open_link", json!({ "url": url, "resume": true })) {
                Ok(v) => {
                    outcome = LinkOutcome::from_answer(&v)
                        .unwrap_or_else(|| panic!("open_link: unexpected shape {v}"));
                    if outcome.complete {
                        break;
                    }
                }
                Err(Failure::Fatal(e)) => panic!("open_link({url:?}): {e}"),
                Err(_) => {}
            }
        }
        outcome
    }

    /// Assert the current navigator's active path is `path`, waiting for it.
    pub fn assert_path(&mut self, path: &str) {
        let desc = format!("assert_path({path:?})");
        self.wait(&desc, |app| {
            let nav = Self::pick_navigator(app.try_navigators()?)?;
            if nav.active_path.as_deref() == Some(path) {
                Ok(())
            } else {
                Err(Failure::NotYet(format!("the active path is {:?}", nav.active_path)))
            }
        });
    }

    /// Assert the current navigator's screen has query parameter `key`
    /// set to `value` (decoded), waiting for it — e.g. that a filter is
    /// carried in the URL: `assert_query("asset_tag", "~7781")`. Screen
    /// state lives in the query, beside the path-only
    /// [`assert_path`](Self::assert_path).
    pub fn assert_query(&mut self, key: &str, value: &str) {
        let desc = format!("assert_query({key:?}, {value:?})");
        self.wait(&desc, |app| {
            let nav = Self::pick_navigator(app.try_navigators()?)?;
            if nav.query.get(key).map(String::as_str) == Some(value) {
                Ok(())
            } else {
                Err(Failure::NotYet(format!("the query is {:?}", nav.query)))
            }
        });
    }

    /// Assert the current navigator's `href` — active path plus encoded
    /// query, the URL web's address bar shows — is `href`, waiting for it.
    pub fn assert_href(&mut self, href: &str) {
        let desc = format!("assert_href({href:?})");
        self.wait(&desc, |app| {
            let nav = Self::pick_navigator(app.try_navigators()?)?;
            if nav.href.as_deref() == Some(href) {
                Ok(())
            } else {
                Err(Failure::NotYet(format!("the href is {:?}", nav.href)))
            }
        });
    }

    fn current_navigator(&mut self) -> NavigatorInfo {
        Self::pick_navigator(self.navigators()).unwrap_or_else(|e| panic!("{}", e.message()))
    }

    fn pick_navigator(navs: Vec<NavigatorInfo>) -> Result<NavigatorInfo, Failure> {
        navs.iter()
            .rev()
            .find(|n| n.controllable && n.is_current)
            .or_else(|| navs.iter().rev().find(|n| n.controllable))
            .cloned()
            .ok_or_else(|| Failure::NotYet("the app has no navigator that takes robot commands".into()))
    }

    // --- components ---

    /// The first mounted instance of the component named `name` (the
    /// `#[component]` fn's name), waiting for one to mount.
    pub fn component(&mut self, name: &str) -> Component<'_> {
        let desc = format!("component({name:?})");
        let instance_id = self.wait(&desc, |app| {
            let list = app.query("list_components", json!({}))?;
            list.as_array()
                .and_then(|l| l.iter().find(|c| c["name"] == name))
                .and_then(|c| c["instance_id"].as_u64())
                .ok_or_else(|| Failure::NotYet("no instance is mounted".into()))
        });
        Component { app: self, instance_id, desc }
    }

    // --- functions ---

    /// Call the robot function registered as `name` (a `#[robot_fn]`, or an
    /// SDK's test control such as `location.set`) with named `args`
    /// (`json!({ "latitude": 52.5, "longitude": 13.4 })`) and return its
    /// result. Waits for the function to be registered, and for an async
    /// one to finish, up to the app's timeout. Panics if it fails; see
    /// [`try_call_function`](Self::try_call_function) to handle that.
    pub fn call_function(&mut self, name: &str, args: Value) -> Value {
        self.try_call_function(name, args)
            .unwrap_or_else(|e| panic!("function {name:?} failed: {e}"))
    }

    /// [`call_function`](Self::call_function), returning the function's
    /// failure (its `Err`, or an argument that didn't decode) instead of
    /// panicking. Still panics when the function never registers or an
    /// async call outlasts the timeout.
    pub fn try_call_function(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let deadline = Instant::now() + self.timeout;
        // Only "not registered" is retried: the function did not run, so
        // asking again cannot run it twice.
        let first = loop {
            match self.try_call("call_function", json!({ "name": name, "args": args.clone() })) {
                Ok(v) => break v,
                Err(e) => {
                    let e = e.to_string();
                    if !e.contains("no function named") {
                        return Err(e);
                    }
                    if Instant::now() >= deadline {
                        panic!("function({name:?}) — still not registered after {:?}: {e}", self.timeout);
                    }
                    std::thread::sleep(POLL);
                }
            }
        };
        if first["done"] == true {
            return Ok(first["value"].clone());
        }
        let call = first["call"]
            .as_u64()
            .unwrap_or_else(|| panic!("call_function: unexpected shape {first}"));
        loop {
            // Polling is a read: a busy app is asked again.
            match self.query("poll_function", json!({ "call": call })) {
                Ok(v) if v["done"] == true => return Ok(v["value"].clone()),
                Err(Failure::Fatal(e)) => return Err(e),
                Ok(_) | Err(_) if Instant::now() >= deadline => {
                    panic!("function({name:?}) — still running after {:?}", self.timeout)
                }
                Ok(_) | Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// The functions the app registered with the robot.
    pub fn functions(&mut self) -> Vec<FunctionInfo> {
        serde_json::from_value(self.read("functions()", "list_functions", json!({})))
            .unwrap_or_else(|e| panic!("list_functions: unexpected shape: {e}"))
    }

    // --- booting at a URL, cookies ---

    /// Boot the app again at `url`, as a user opening that address would,
    /// and return once the new instance answers. Unlike
    /// [`open_link`](Self::open_link), which routes the running app, this
    /// starts it over: what the app reads at boot (the initial path, the
    /// cookies the server sees) is what `url` gives it, and its state starts
    /// fresh.
    ///
    /// On web the page navigates (`/x` resolves against the current page,
    /// so the same dev server serves it); a URL that differs only in its
    /// `#fragment` reloads. On Android the app is relaunched with the link
    /// (an app path becomes `https://localhost/x`, whose route is `/x`).
    /// iOS and desktop apps answer that they can't be relaunched by the
    /// dev session. Waits up to the larger of the app's timeout and
    /// [`LOAD_URL_TIMEOUT`]. Panics on failure; see
    /// [`try_load_url`](Self::try_load_url).
    pub fn load_url(&mut self, url: &str) -> String {
        self.try_load_url(url).unwrap_or_else(|e| panic!("load_url({url:?}) failed: {e}"))
    }

    /// [`load_url`](Self::load_url), returning the error instead of
    /// panicking. `Ok` is the address the app was loaded at.
    pub fn try_load_url(&mut self, url: &str) -> anyhow::Result<String> {
        let wait = self.timeout.max(LOAD_URL_TIMEOUT);
        let io = self.client.io_timeout();
        self.client.set_io_timeout(wait)?;
        let answer = self.client.call("load_url", json!({ "url": url }));
        self.client.set_io_timeout(io)?;
        let loaded = answer?
            .get("url")
            .and_then(|u| u.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("load_url: the answer has no url"))?;
        // The relay answered once the new instance said hello; its first
        // frame may still be building, so wait for it to answer a query.
        self.client.wait_ready(wait)?;
        Ok(loaded)
    }

    /// Set a cookie in the web page's browser, HttpOnly ones included; it
    /// applies from the next request, so set it before
    /// [`load_url`](Self::load_url) for the server to see it at boot. A
    /// cookie with neither `url` nor `domain` belongs to the page's origin.
    /// Web only: a native app's HTTP stack keeps its own cookie store, which
    /// the robot does not reach, and the call panics saying so.
    pub fn set_cookie(&mut self, cookie: Cookie) {
        let args = serde_json::to_value(&cookie).expect("a cookie serializes");
        self.call("set_cookie", args);
    }

    /// The cookies the browser sends to the page's address, HttpOnly ones
    /// included. Web only, as [`set_cookie`](Self::set_cookie).
    pub fn cookies(&mut self) -> Vec<Cookie> {
        let v = self.call("get_cookies", json!({}));
        serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("get_cookies: unexpected shape {v}: {e}"))
    }

    /// Delete every cookie in the page's browser. Web only.
    pub fn clear_cookies(&mut self) {
        self.call("clear_cookies", json!({}));
    }

    /// Delete the page origin's cookie named `name`. Web only.
    pub fn clear_cookie(&mut self, name: &str) {
        self.call("clear_cookies", json!({ "name": name }));
    }

    // --- whole-app ---

    /// Capture a screenshot; returns the saved path (or `"(in memory)"` if the
    /// relay didn't persist one). Panics on a bridge error.
    pub fn screenshot(&mut self) -> String {
        let v = self.call("screenshot", json!({}));
        v.get("path").and_then(|p| p.as_str()).unwrap_or("(in memory)").to_string()
    }

    /// The app's log lines (`log_info!` and friends), oldest first.
    pub fn logs(&mut self) -> Vec<Value> {
        match self.read("logs()", "get_logs", json!({})) {
            Value::Array(lines) => lines,
            other => panic!("get_logs: unexpected shape {other}"),
        }
    }

    /// Capture this app's **platform-native render tree** — each element's
    /// resolved geometry + visual props as the backend reports them, keyed by a
    /// stable element path. Use [`crate::parity::diff`] to compare two captures
    /// (e.g. web vs macOS) for cross-platform parity. Panics on a bridge error.
    pub fn capture_native(&mut self) -> crate::parity::Capture {
        crate::parity::capture_native(&mut self.client)
            .unwrap_or_else(|e| panic!("capture_native failed: {e}"))
    }

    // --- internals ---

    /// Run `attempt` until it returns `Ok` or the timeout passes; then panic
    /// with `desc` and the last error. A [`Failure::Fatal`] panics at once.
    fn wait<T>(&mut self, desc: &str, mut attempt: impl FnMut(&mut App) -> Result<T, Failure>) -> T {
        let deadline = Instant::now() + self.timeout;
        loop {
            match attempt(self) {
                Ok(v) => return v,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(last) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.timeout, last.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// One call, its failure sorted for a waiting caller: a busy or absent
    /// app is [`Failure::Busy`], anything the app answered is fatal.
    fn query(&mut self, verb: &str, args: Value) -> Result<Value, Failure> {
        self.client.call(verb, args).map_err(|e| match CallError::of(&e) {
            Some(ce) if ce.is_transient() => Failure::Busy(format!("{verb}: {ce}")),
            _ => Failure::Fatal(format!("{verb} failed: {e}")),
        })
    }

    /// A read that does not wait for any condition, but asks again while
    /// the app is busy — up to the timeout, then panics with the last error.
    fn read(&mut self, desc: &str, verb: &str, args: Value) -> Value {
        let args = &args;
        self.wait(desc, |app| match app.query(verb, args.clone()) {
            Err(Failure::NotYet(e)) => Err(Failure::Fatal(e)),
            other => other,
        })
    }

    /// The elements `sel` matches now, in document order. One call: every
    /// filter, `within` included, goes to the app as one query. The leaf
    /// filters are checked again here against what came back, so an app
    /// older than combined queries (which honoured only the first field)
    /// can't return a wider match than asked.
    fn find_all(&mut self, sel: &Selector) -> Result<Vec<Value>, Failure> {
        match self.query("find_all_elements", sel.to_query())? {
            Value::Array(els) => Ok(els.into_iter().filter(|el| sel.leaf_matches(el)).collect()),
            other => Err(Failure::Fatal(format!("find_all_elements: unexpected shape {other}"))),
        }
    }
}

/// Why one attempt of a waiting operation did not succeed yet.
enum Failure {
    /// The app was busy, or the relay had no app: ask again.
    Busy(String),
    /// The app answered, and the condition does not hold yet: look again.
    NotYet(String),
    /// Asking again can't help (an unknown kind, an unsupported verb, a
    /// malformed reply): fail now.
    Fatal(String),
}

impl Failure {
    fn message(&self) -> &str {
        match self {
            Failure::Busy(m) | Failure::NotYet(m) | Failure::Fatal(m) => m,
        }
    }
}

impl From<String> for Failure {
    fn from(m: String) -> Self {
        Failure::NotYet(m)
    }
}

/// The app's viewport, as [`App::viewport`] reports it.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct Viewport {
    /// Logical px.
    pub width: f64,
    /// Logical px.
    pub height: f64,
    /// The active breakpoint: `"xs"`, `"sm"`, `"md"`, `"lg"` or `"xl"`.
    pub breakpoint: String,
}

/// The platform an app runs on, as [`App::platform`] reports it. The
/// names are those of `idealyst test --<platform>` and
/// `#[robot_test(platforms = …)]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Web,
    Ios,
    Android,
    Macos,
    Linux,
    Windows,
    /// Any other host (`tvos`, `androidtv`, a custom backend's own name,
    /// `unknown`).
    Other(String),
}

impl Platform {
    /// From the app's `get_platform` name.
    pub fn from_id(id: &str) -> Self {
        match id {
            "web" => Platform::Web,
            "ios" => Platform::Ios,
            "android" => Platform::Android,
            "macos" => Platform::Macos,
            "linux" => Platform::Linux,
            "windows" => Platform::Windows,
            other => Platform::Other(other.to_string()),
        }
    }

    /// `web`, `ios`, `android`, `macos`, `linux`, `windows`, or the other
    /// host's own name.
    pub fn as_str(&self) -> &str {
        match self {
            Platform::Web => "web",
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Macos => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
            Platform::Other(name) => name,
        }
    }

    /// iOS or Android: a phone or tablet app.
    pub fn is_mobile(&self) -> bool {
        matches!(self, Platform::Ios | Platform::Android)
    }

    /// macOS, Linux or Windows: a desktop app.
    pub fn is_desktop(&self) -> bool {
        matches!(self, Platform::Macos | Platform::Linux | Platform::Windows)
    }

    /// Any of the native apps (mobile or desktop) — not the web.
    pub fn is_native(&self) -> bool {
        self.is_mobile() || self.is_desktop()
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The base selector a locator starts from.
#[derive(Clone, Debug)]
enum Sel {
    TestId(String),
    Text(String),
    Label(String),
    Role(String),
}

/// What to find — a test id, kind, label, text, or several of them at once,
/// optionally inside a container — without holding the app. Lazy: a
/// [`Locator`] built from it re-resolves on each action, so it survives
/// re-renders like a Playwright locator survives DOM churn.
///
/// Every criterion must hold (AND), and the whole selector is ONE query to
/// the app (`find_all_elements` with its fields and `within`), not one call
/// per filter. Build one detached to pass to [`Locator::within`] or
/// [`App::locate`] — `Selector::test_id("dialog")` — or detach a locator
/// with [`Locator::selector`].
#[derive(Clone, Debug, Default)]
pub struct Selector {
    test_id: Option<String>,
    kind: Option<String>,
    label: Option<String>,
    texts: Vec<String>,
    within: Option<Box<Selector>>,
    nth: Option<usize>,
    desc: String,
}

impl From<Sel> for Selector {
    fn from(sel: Sel) -> Self {
        let mut s = Selector::default();
        match sel {
            Sel::TestId(id) => {
                s.desc = format!("test_id({id:?})");
                s.test_id = Some(id);
            }
            Sel::Text(t) => {
                s.desc = format!("text({t:?})");
                s.texts.push(t);
            }
            Sel::Label(l) => {
                s.desc = format!("label({l:?})");
                s.label = Some(l);
            }
            Sel::Role(k) => {
                s.desc = format!("role({k:?})");
                s.kind = Some(k);
            }
        }
        s
    }
}

impl Selector {
    /// Elements with this `test_id`.
    pub fn test_id(id: &str) -> Self {
        Sel::TestId(id.into()).into()
    }

    /// Elements whose label contains `substring` (their visible text).
    pub fn text(substring: &str) -> Self {
        Sel::Text(substring.into()).into()
    }

    /// Elements whose label is exactly `label`.
    pub fn label(label: &str) -> Self {
        Sel::Label(label.into()).into()
    }

    /// Elements of this primitive kind (`"TextInput"`, `"Button"`, …).
    pub fn role(kind: &str) -> Self {
        Sel::Role(kind.into()).into()
    }

    /// Also require this `test_id` (replaces one already given).
    pub fn with_test_id(mut self, id: &str) -> Self {
        self.desc = format!("{}.with_test_id({id:?})", self.desc);
        self.test_id = Some(id.into());
        self
    }

    /// Also require the label to be exactly `label` (replaces one already
    /// given) — `role("TextInput").with_label("Email")` is the field, not
    /// its caption.
    pub fn with_label(mut self, label: &str) -> Self {
        self.desc = format!("{}.with_label({label:?})", self.desc);
        self.label = Some(label.into());
        self
    }

    /// Also require the label to contain `substring` (adds to any others).
    pub fn with_text(mut self, substring: &str) -> Self {
        self.desc = format!("{}.with_text({substring:?})", self.desc);
        self.texts.push(substring.into());
        self
    }

    /// Also require this kind (replaces one already given).
    pub fn with_role(mut self, kind: &str) -> Self {
        self.desc = format!("{}.with_role({kind:?})", self.desc);
        self.kind = Some(kind.into());
        self
    }

    /// Only inside `container`: descendants of an element it matches (never
    /// the container itself) — a dialog, a table row, a form. When the
    /// container selector has an [`nth`](Self::nth), only that one counts;
    /// otherwise any match does. Replaces an earlier `within`; to nest,
    /// give the container its own `within`.
    pub fn within(mut self, container: impl Into<Selector>) -> Self {
        let container = container.into();
        self.desc = format!("{}.within({})", self.desc, container.desc);
        self.within = Some(Box::new(container));
        self
    }

    /// The `index`th (0-based, document order) of the elements matched.
    pub fn nth(mut self, index: usize) -> Self {
        self.desc = format!("{}.nth({index})", self.desc);
        self.nth = Some(index);
        self
    }

    /// Only a `test_id`: the one case resolved by the app's last-wins index
    /// (`find_element`) rather than a scan.
    fn is_plain_test_id(&self) -> bool {
        self.test_id.is_some() && self.kind.is_none() && self.label.is_none() && self.texts.is_empty() && self.within.is_none()
    }

    /// The query object `find_element` / `find_all_elements` take. `nth` is
    /// the caller's to apply, except a container's, which goes along so the
    /// app picks that one container.
    fn to_query(&self) -> Value {
        let mut q = serde_json::Map::new();
        if let Some(id) = &self.test_id {
            q.insert("test_id".into(), json!(id));
        }
        if let Some(kind) = &self.kind {
            q.insert("kind".into(), json!(kind));
        }
        if let Some(label) = &self.label {
            q.insert("label".into(), json!(label));
        }
        match self.texts.as_slice() {
            [] => {}
            [one] => {
                q.insert("label_contains".into(), json!(one));
            }
            many => {
                q.insert("label_contains".into(), json!(many));
            }
        }
        if let Some(container) = &self.within {
            let mut c = container.to_query();
            if let (Some(n), Value::Object(map)) = (container.nth, &mut c) {
                map.insert("nth".into(), json!(n));
            }
            q.insert("within".into(), c);
        }
        Value::Object(q)
    }

    /// Whether an element JSON satisfies the leaf criteria (`within` is the
    /// app's to check — the element JSON doesn't carry ancestry).
    fn leaf_matches(&self, el: &Value) -> bool {
        let label = el["label"].as_str();
        self.test_id.as_deref().is_none_or(|t| el["test_id"].as_str() == Some(t))
            && self.kind.as_deref().is_none_or(|k| el["kind"].as_str() == Some(k))
            && self.label.as_deref().is_none_or(|l| label == Some(l))
            && self.texts.iter().all(|t| label.is_some_and(|l| l.contains(t.as_str())))
    }
}

impl From<Locator<'_>> for Selector {
    fn from(locator: Locator<'_>) -> Self {
        locator.sel
    }
}

/// What an element reports about itself (`get_state`): the text a field
/// shows, a toggle's checked state, whether it refuses input, whether it
/// holds focus. Fields an element doesn't have are `None`.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ElementState {
    /// A text field's text (`Value::String`) or a slider's value (`Value::Number`).
    pub value: Option<Value>,
    pub checked: Option<bool>,
    pub disabled: Option<bool>,
    pub focused: Option<bool>,
    /// A scroll view's content offset.
    pub scroll: Option<ScrollOffset>,
    /// Whether the element's `state hovered` style is on. `None` where the
    /// platform styles hover itself (CSS on web) or the style has no state
    /// variants.
    pub hovered: Option<bool>,
    /// Whether the element's `state pressed` style is on.
    pub pressed: Option<bool>,
    /// A text field's soft-keyboard kind as the app asked for it
    /// (`"default"`, `"email"`, `"phone"`, `"number"`, `"decimal"`,
    /// `"url"`).
    pub keyboard: Option<String>,
    /// The kind the native widget holds (UIKit's `keyboardType`, Android's
    /// `inputType`, the web's `inputmode`). `None` where the platform has
    /// no soft keyboard.
    pub native_keyboard: Option<String>,
}

/// An element's resolved style, as `get_style` reports it
/// ([`Locator::style`]). `source` is `"native"` when the toolkit resolved
/// it (the web's `getComputedStyle`) and `"framework"` when it is the rules
/// the framework handed the backend (every native backend). Absent fields:
/// the style doesn't set it.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct Style {
    #[serde(skip)]
    pub source: String,
    /// Text color, sRGB `[r, g, b, a]` in `0..=1`.
    pub color: Option<[f32; 4]>,
    pub background: Option<[f32; 4]>,
    pub opacity: Option<f32>,
    pub font_size: Option<f32>,
    /// CSS weight, `100..=900`.
    pub font_weight: Option<u16>,
    /// `"normal"` or `"italic"`.
    pub font_style: Option<String>,
    pub font_family: Option<String>,
    pub text_decoration: Option<TextDecoration>,
    /// `"left"`, `"center"`, `"right"` or `"justify"`.
    pub text_align: Option<String>,
    pub border_width: Option<f32>,
    pub border_color: Option<[f32; 4]>,
    pub corner_radius: Option<f32>,
}

impl Style {
    /// Struck through.
    pub fn is_line_through(&self) -> bool {
        matches!(self.text_decoration, Some(TextDecoration::LineThrough | TextDecoration::UnderlineLineThrough))
    }

    /// Underlined.
    pub fn is_underline(&self) -> bool {
        matches!(self.text_decoration, Some(TextDecoration::Underline | TextDecoration::UnderlineLineThrough))
    }

    /// Bold or heavier (weight ≥ 600).
    pub fn is_bold(&self) -> bool {
        self.font_weight.is_some_and(|w| w >= 600)
    }
}

/// [`Style::text_decoration`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum TextDecoration {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "underline")]
    Underline,
    #[serde(rename = "line-through")]
    LineThrough,
    #[serde(rename = "underline line-through")]
    UnderlineLineThrough,
}

/// A scroll view's content offset, logical px.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ScrollOffset {
    pub x: f64,
    pub y: f64,
}

/// A located (or to-be-located) element. Actions and assertions consume `self`
/// — chain straight off the locator: `app.test_id("inc").click()`. Narrow it
/// first with the filters, which all go to the app as one query:
/// `app.role("TextInput").with_label("Email").within(Selector::test_id("login")).fill("a@b.c")`.
pub struct Locator<'a> {
    app: &'a mut App,
    sel: Selector,
    /// [`Locator::no_scroll`]: tap where the element is, scrolling nothing.
    no_scroll: bool,
}

impl<'a> Locator<'a> {
    fn new(app: &'a mut App, sel: Sel) -> Self {
        Locator { app, sel: sel.into(), no_scroll: false }
    }

    /// The `index`th (0-based, document order) of the elements the selector
    /// matches — `app.role("TextInput").nth(1)` is the second field.
    pub fn nth(mut self, index: usize) -> Self {
        self.sel = self.sel.nth(index);
        self
    }

    /// Also require this `test_id`. See [`Selector::with_test_id`].
    pub fn with_test_id(mut self, id: &str) -> Self {
        self.sel = self.sel.with_test_id(id);
        self
    }

    /// Also require the exact label: `app.role("TextInput").with_label("Email")`
    /// is the field, not its caption. See [`Selector::with_label`].
    pub fn with_label(mut self, label: &str) -> Self {
        self.sel = self.sel.with_label(label);
        self
    }

    /// Also require the label to contain `substring`. See
    /// [`Selector::with_text`].
    pub fn with_text(mut self, substring: &str) -> Self {
        self.sel = self.sel.with_text(substring);
        self
    }

    /// Also require this kind. See [`Selector::with_role`].
    pub fn with_role(mut self, kind: &str) -> Self {
        self.sel = self.sel.with_role(kind);
        self
    }

    /// Only inside `container` — a dialog, a table row:
    /// `.within(Selector::test_id("dialog"))`. A locator held in a
    /// variable converts too (`.within(dialog)` after
    /// `let dialog = app.test_id("dialog").selector();`). See
    /// [`Selector::within`].
    pub fn within(mut self, container: impl Into<Selector>) -> Self {
        self.sel = self.sel.within(container);
        self
    }

    /// Detach this locator's selector from the app, to keep it or pass it
    /// to [`within`](Self::within).
    pub fn selector(self) -> Selector {
        self.sel
    }

    /// The element as the bridge reports it now, or why there is none.
    fn resolve(&mut self) -> Result<Value, Failure> {
        let index = self.sel.nth.unwrap_or(0);
        let matches = if self.sel.is_plain_test_id() && self.sel.nth.is_none() {
            // A single test_id lookup is one indexed hit (last mounted wins).
            let v = self.app.query("find_element", self.sel.to_query())?;
            if v.is_null() { Vec::new() } else { vec![v] }
        } else {
            self.app.find_all(&self.sel)?
        };
        match matches.into_iter().nth(index) {
            Some(v) => Ok(v),
            None if self.sel.nth.is_some() => Err(Failure::NotYet(format!("fewer than {} elements match", index + 1))),
            None => Err(Failure::NotYet("no element matches".into())),
        }
    }

    /// [`resolve`](Self::resolve) for a read that does not wait for the
    /// element: asks again only while the app is busy. `Ok(None)` = no match.
    fn resolve_now(&mut self, what: &str) -> Option<Value> {
        let desc = format!("{}.{what}", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            match self.resolve() {
                Ok(v) => return Some(v),
                Err(Failure::NotYet(_)) => return None,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(Failure::Busy(e)) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {e}", self.app.timeout)
                }
                Err(Failure::Busy(_)) => std::thread::sleep(POLL),
            }
        }
    }

    fn id_of(el: &Value) -> Result<u64, Failure> {
        el.get("id").and_then(|i| i.as_u64()).ok_or_else(|| Failure::Fatal(format!("an element with no id: {el}")))
    }

    /// Send an action verb. Its failure sorted for [`act`](Self::act): an
    /// unavailable action is fatal; an action the app did not answer may
    /// still run, so it is fatal too rather than sent twice; a relay with
    /// no app never delivered it, so that is retried; anything else the app
    /// answered (a disabled control refusing) is "not yet".
    fn send_action(&mut self, verb: &str, args: Value) -> Result<Value, Failure> {
        self.app.client.call(verb, args).map_err(|e| match CallError::of(&e) {
            Some(CallError::NoApp(m)) => Failure::Busy(m.clone()),
            Some(ce @ (CallError::AppTimedOut(_) | CallError::ClientTimedOut { .. })) => Failure::Fatal(format!(
                "{ce} — the app may still perform it, so it is not sent again \
                 (App::set_io_timeout raises the wait)"
            )),
            Some(CallError::Verb(m)) if m.contains("not available") => Failure::Fatal(m.clone()),
            _ => Failure::NotYet(e.to_string()),
        })
    }

    /// Run `verb` on the element, waiting until it exists and accepts it — a
    /// disabled control refuses, so this waits for it to be enabled.
    fn act(mut self, verb: &str, args: Value) {
        let desc = format!("{}.{verb}", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let attempt = self.resolve().and_then(|el| Self::id_of(&el)).and_then(|id| {
                let mut args = args.clone();
                if let Value::Object(map) = &mut args {
                    map.insert("element_id".into(), json!(id));
                }
                self.send_action(verb, args).map(|_| ())
            });
            match attempt {
                Ok(()) => return,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.app.timeout, e.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// Retry `check` against the resolved element until it holds or the
    /// timeout passes.
    fn expect(mut self, what: &str, mut check: impl FnMut(&Value) -> Result<(), String>) {
        let desc = format!("{}.{what}", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let last = match self.resolve() {
                Ok(el) => match check(&el) {
                    Ok(()) => return,
                    Err(e) => e,
                },
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) => e.message().to_string(),
            };
            if Instant::now() >= deadline {
                panic!("{desc} — still failing after {:?}: {last}", self.app.timeout);
            }
            std::thread::sleep(POLL);
        }
    }

    // --- actions ---

    /// Click / tap the element as a user would: once it is visible and
    /// enabled, scrolled into view if it is out of it
    /// ([`scroll_into_view`](Self::scroll_into_view); opt out with
    /// [`no_scroll`](Self::no_scroll) or [`App::set_auto_scroll`]), a real
    /// platform tap at its center (`tap_native`) — through the toolkit's
    /// hit testing, so an overlay on top makes it wait, then fail, as it
    /// would a user. On a backend that can't synthesize input it falls back
    /// to the element's own press handler
    /// ([`dispatch_click`](Self::dispatch_click)).
    pub fn click(self) {
        self.tap(true);
    }

    /// A real platform tap at the element's center, with no fallback:
    /// panics on a backend that can't synthesize input. Scrolls the element
    /// into view first, as [`click`](Self::click) does. Returns how it
    /// reached the element: `"native"` (platform events) or `"hit_test"`
    /// (the backend's hit test confirmed the point, then the press ran —
    /// iOS, where an app can't create touches).
    pub fn tap_native(self) -> String {
        self.tap(false)
    }

    /// Move a real mouse over the element (no button held), once it is
    /// visible and scrolled into view as [`click`](Self::click) does.
    /// Panics on a platform with no hovering pointer.
    pub fn hover_native(mut self) {
        self.pointer("hover_native", "hover_native", json!({}), None);
    }

    /// Press the primary button down on the element with real input and
    /// keep it down, once it is visible, enabled and scrolled into view as
    /// [`click`](Self::click) does; [`release_native`](Self::release_native)
    /// lifts it.
    pub fn press_native(mut self) {
        self.pointer("press_native", "press_native", json!({}), None);
    }

    /// Lift a [`press_native`](Self::press_native) at the element. Scrolls
    /// nothing: the press is lifted where it went down, and a scroll while
    /// it is held would move the content under the finger.
    pub fn release_native(self) {
        self.act("release_native", json!({}));
    }

    /// Run the element's press handler directly (the robot's `click`),
    /// skipping the platform's event path — for an element a user can't
    /// reach on purpose, or a backend with no native input.
    pub fn dispatch_click(self) {
        self.act("click", json!({}));
    }

    /// Drag with real platform input along `points` (relative to the
    /// element's top-left, as [`drag`](Self::drag) takes them), once the
    /// element is visible, enabled and scrolled into view as
    /// [`click`](Self::click) does. Panics on a backend that can't
    /// synthesize input.
    pub fn drag_native(mut self, points: &[(f32, f32)]) {
        let pts: Vec<[f32; 2]> = points.iter().map(|&(x, y)| [x, y]).collect();
        self.pointer("drag_native", "drag_native", json!({ "points": pts }), None);
    }

    fn tap(mut self, fallback: bool) -> String {
        let name = if fallback { "click" } else { "tap_native" };
        self.pointer(name, "tap_native", json!({}), fallback.then_some("click")).0
    }

    /// Send the pointer verb `send` (a real-input verb: `tap_native`,
    /// `press_native`, `hover_native`, `drag_native`, `context_click`) with
    /// `args` at the element, as a user would: once it is visible and
    /// enabled, scrolled into view first unless told not to
    /// ([`no_scroll`](Self::no_scroll), [`App::set_auto_scroll`]). A backend
    /// that can't synthesize the input answers "unsupported": with a
    /// `fallback` verb that runs instead, without one it fails. Returns the
    /// mechanism the app reports and the element's id.
    fn pointer(&mut self, name: &str, send: &str, args: Value, fallback: Option<&str>) -> (String, u64) {
        let desc = format!("{}.{name}", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let attempt = self.resolve().and_then(|el| {
                if el["visible"] == false {
                    return Err(Failure::NotYet("the element is not visible".into()));
                }
                if el["state"]["disabled"] == true {
                    return Err(Failure::NotYet("the element is disabled".into()));
                }
                let id = Self::id_of(&el)?;
                let scroll = self.app.auto_scroll && !self.no_scroll;
                // Out of the window: scroll first, as Playwright does.
                if scroll && el["in_viewport"] == false {
                    self.reveal(id)?;
                }
                let mut args = args.clone();
                if let Value::Object(map) = &mut args {
                    map.insert("element_id".into(), json!(id));
                }
                let mut sent = self.send_action(send, args.clone());
                // In the window but clipped by its scroll view (the point
                // lands on whatever is beside it) or past the window edge:
                // scroll, and send again if that moved anything.
                if let Err(Failure::NotYet(e)) = &sent {
                    if scroll && (e.contains("outside the window") || e.contains("covers the point")) && self.reveal(id)? {
                        sent = self.send_action(send, args);
                    }
                }
                match sent {
                    Ok(v) => Ok((v["mechanism"].as_str().unwrap_or("native").to_string(), id)),
                    Err(Failure::NotYet(e)) if e.contains("unsupported") => match fallback {
                        Some(verb) => {
                            self.send_action(verb, json!({ "element_id": id })).map(|_| ("robot".to_string(), id))
                        }
                        None => Err(Failure::Fatal(e)),
                    },
                    Err(e) => Err(e),
                }
            });
            match attempt {
                Ok(done) => return done,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.app.timeout, e.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// Long-press the element: a real press at its center held for `hold`,
    /// then lifted — what a touch platform's long-press recognizers (the
    /// framework's `long_press`, the dnd SDK's hold pickup, Android's own
    /// long click) fire on. The press goes in as [`click`](Self::click)'s
    /// does (visible, enabled, scrolled into view); real time passes while
    /// it is held. Returns the mechanism: `"native"` (platform input) or
    /// `"hit_test"` (iOS: the backend's hit test confirmed the point and the
    /// press went into the element's touch handlers, held the same way).
    /// [`LONG_PRESS`] clears every platform's long-press threshold.
    pub fn long_press(mut self, hold: Duration) -> String {
        let (how, id) = self.pointer("long_press", "press_native", json!({}), None);
        std::thread::sleep(hold);
        // Sent once, never retried: a release the app did not answer may
        // still land, and a second one would lift a press nobody made.
        if let Err(e) = self.app.client.call("release_native", json!({ "element_id": id })) {
            panic!("{}.long_press: lifting the press failed: {e}", self.sel.desc);
        }
        how
    }

    /// Open the element's context menu as a user of the platform does: a
    /// secondary (right) click on desktop and web — the `context_click`
    /// verb, which reaches the app's touch handlers as a
    /// `PointerButton::Secondary` press — and a [`long_press`](Self::long_press)
    /// of [`LONG_PRESS`] on iOS and Android, whose context gesture is a
    /// hold. Returns the mechanism the app reports.
    pub fn context_click(mut self) -> String {
        let platform = self.app.try_platform().unwrap_or_else(|e| panic!("context_click: {e}"));
        if platform.is_some_and(|p| p.is_mobile()) {
            return self.long_press(LONG_PRESS);
        }
        self.pointer("context_click", "context_click", json!({}), None).0
    }

    /// A secondary (right) click on the element on every platform — the
    /// `context_click` verb, with no long-press substitute on a phone (there
    /// it goes through the backend's hit test as a secondary press, as an
    /// iPad with a mouse delivers one). For a test of the secondary-button
    /// path itself; [`context_click`](Self::context_click) is what a user
    /// does.
    pub fn secondary_click(mut self) -> String {
        self.pointer("secondary_click", "context_click", json!({}), None).0
    }

    /// Act where the element is now: [`click`](Self::click),
    /// [`tap_native`](Self::tap_native) and the other real-input actions
    /// ([`long_press`](Self::long_press), [`context_click`](Self::context_click),
    /// [`press_native`](Self::press_native), [`hover_native`](Self::hover_native),
    /// [`drag_native`](Self::drag_native)) won't scroll it into view first,
    /// so one out of view waits, then fails. For a test of what a user who
    /// doesn't scroll can reach. See [`App::set_auto_scroll`].
    pub fn no_scroll(mut self) -> Self {
        self.no_scroll = true;
        self
    }

    /// Scroll every scroll view the element sits in — nested ones, either
    /// axis — by the least that shows the whole element, in its scroll
    /// views and in the window (the `scroll_into_view` verb). Already in
    /// view, nothing moves. Waits for the element to exist; an element a
    /// virtualized list hasn't mounted yet doesn't exist, so scroll the
    /// list itself to reach it.
    pub fn scroll_into_view(self) {
        self.act("scroll_into_view", json!({}));
    }

    /// Scroll element `id` into view before a tap: `Ok(true)` when
    /// something moved. An app older than the verb answers "unknown
    /// command" — nothing scrolled, and the tap goes ahead as before.
    fn reveal(&mut self, id: u64) -> Result<bool, Failure> {
        match self.send_action("scroll_into_view", json!({ "element_id": id })) {
            Ok(v) => Ok(v["scrolled"].as_array().is_some_and(|s| !s.is_empty())),
            Err(Failure::NotYet(e)) if e.contains("unknown command") => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Replace a text field's text (like Playwright `fill`).
    pub fn fill(self, text: &str) {
        self.act("type_text", json!({ "text": text }));
    }

    /// Alias of [`fill`](Self::fill).
    pub fn type_text(self, text: &str) {
        self.fill(text);
    }

    /// Set a toggle on/off.
    pub fn set_toggle(self, on: bool) {
        self.act("set_toggle", json!({ "value": on }));
    }

    /// Set a slider to `value`, in its own `min..max` range (clamped, and
    /// snapped to its `step`, as a drag would be).
    pub fn set_slider(self, value: f64) {
        self.act("set_slider", json!({ "value": value }));
    }

    /// Give a text field keyboard focus.
    pub fn focus(self) {
        self.act("focus", json!({}));
    }

    /// Take keyboard focus away from a text field.
    pub fn blur(self) {
        self.act("blur", json!({}));
    }

    /// Scroll a scroll view's content to offset `(x, y)`.
    pub fn scroll_to(self, x: f64, y: f64) {
        self.act("set_scroll", json!({ "x": x, "y": y }));
    }

    /// Drag one pointer along `points` — `[x, y]` in logical px relative
    /// to the element's top-left (draw a signature, drag a handle). On a
    /// slider, each point's x along the track sets the value. For a
    /// long-press pickup use [`drag_with_hold`](Self::drag_with_hold).
    pub fn drag(self, points: &[(f32, f32)]) {
        self.drag_with_hold(points, 0);
    }

    /// [`drag`](Self::drag), holding at the first point for `hold_ms`
    /// first — drag-and-drop on iOS/Android picks up after a 200 ms hold.
    pub fn drag_with_hold(self, points: &[(f32, f32)], hold_ms: u64) {
        let points: Vec<[f32; 2]> = points.iter().map(|&(x, y)| [x, y]).collect();
        self.act("drag", json!({ "points": points, "hold_ms": hold_ms }));
    }

    // --- reads (wait for the element to exist, then read once) ---

    /// The element's state now. Waits for the element to exist.
    pub fn state(mut self) -> ElementState {
        let desc = format!("{}.state()", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            match self.resolve() {
                Ok(el) => return state_of(&el),
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.app.timeout, e.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// The element's current label, or `None` if it has none / isn't present
    /// (does not wait for it; asks again while the app is busy).
    pub fn label(mut self) -> Option<String> {
        self.resolve_now("label()")?.get("label").and_then(|l| l.as_str()).map(str::to_string)
    }

    /// How many elements the selector matches now (does not wait for a
    /// count; asks again while the app is busy).
    pub fn count(self) -> usize {
        let desc = format!("{}.count()", self.sel.desc);
        let sel = self.sel;
        self.app.wait(&desc, |app| app.find_all(&sel)).len()
    }

    // --- assertions (retry until they hold) ---

    /// Assert the element can be seen: mounted, shown by the toolkit (not
    /// hidden by itself or an ancestor, not fully transparent) and with a
    /// non-empty box. Scrolled out of view still counts — see
    /// [`assert_in_viewport`](Self::assert_in_viewport).
    pub fn assert_visible(self) {
        self.expect("assert_visible()", |el| match el.get("visible") {
            Some(Value::Bool(false)) => Err("it is mounted but not visible".into()),
            _ => Ok(()),
        });
    }

    /// Assert nothing matching can be seen: no element matches, or the one
    /// that does is not visible (a closed overlay, a navigated-away screen).
    pub fn assert_hidden(mut self) {
        let desc = format!("{}.assert_hidden()", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let last = match self.resolve() {
                Ok(el) if el.get("visible") == Some(&Value::Bool(false)) => return,
                Err(Failure::NotYet(_)) => return,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Ok(_) => "the element is still visible".to_string(),
                Err(Failure::Busy(e)) => e,
            };
            if Instant::now() >= deadline {
                panic!("{desc} — still failing after {:?}: {last}", self.app.timeout);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Assert the element is visible and overlaps the window (not scrolled
    /// or placed out of view).
    pub fn assert_in_viewport(self) {
        self.expect("assert_in_viewport()", |el| match (el.get("visible"), el.get("in_viewport")) {
            (Some(Value::Bool(false)), _) => Err("it is not visible".into()),
            (_, Some(Value::Bool(true))) => Ok(()),
            (_, Some(Value::Bool(false))) => {
                Err("it is outside the window, or scrolled out of a scroll view or list that clips it".into())
            }
            _ => Err("its frame or the window size is unknown on this backend".into()),
        });
    }

    /// Whether the element is mounted and visible now (does not wait).
    pub fn is_visible(mut self) -> bool {
        self.resolve_now("is_visible()").is_some_and(|el| el.get("visible") != Some(&Value::Bool(false)))
    }

    /// Assert the element's label equals `expected`.
    pub fn assert_text(self, expected: &str) {
        let what = format!("assert_text({expected:?})");
        self.expect(&what, |el| {
            let actual = el.get("label").and_then(|l| l.as_str());
            (actual == Some(expected)).then_some(()).ok_or(format!("the label is {actual:?}"))
        });
    }

    /// Assert the element's label contains `needle`.
    pub fn assert_text_contains(self, needle: &str) {
        let what = format!("assert_text_contains({needle:?})");
        self.expect(&what, |el| {
            let actual = el.get("label").and_then(|l| l.as_str()).unwrap_or("");
            actual.contains(needle).then_some(()).ok_or(format!("the label is {actual:?}"))
        });
    }

    /// Assert a text field's text, or a slider's value (`assert_value(5.0)`).
    pub fn assert_value(self, expected: impl Into<Value>) {
        let expected = expected.into();
        let what = format!("assert_value({expected})");
        self.expect(&what, move |el| {
            let actual = state_of(el).value;
            if values_match(actual.as_ref(), &expected) {
                Ok(())
            } else {
                Err(format!("the value is {}", actual.map_or("absent".into(), |v| v.to_string())))
            }
        });
    }

    /// Assert a toggle is on (`true`) or off (`false`).
    pub fn assert_checked(self, expected: bool) {
        let what = format!("assert_checked({expected})");
        self.expect(&what, |el| {
            let actual = state_of(el).checked;
            (actual == Some(expected)).then_some(()).ok_or(format!("checked is {actual:?}"))
        });
    }

    /// A text field's soft-keyboard kind (`"email"`, …): the one the native
    /// widget holds where the platform has a soft keyboard, else the one
    /// the app asked for. Waits for the element.
    pub fn keyboard(self) -> Option<String> {
        let st = self.state();
        st.native_keyboard.or(st.keyboard)
    }

    /// Assert a text field's soft-keyboard kind (`assert_keyboard("email")`)
    /// — both the kind the app asked for and, where the platform has a soft
    /// keyboard, the one the native widget holds.
    pub fn assert_keyboard(self, expected: &str) {
        let what = format!("assert_keyboard({expected:?})");
        self.expect(&what, |el| {
            let st = state_of(el);
            let ok = st.keyboard.as_deref() == Some(expected)
                && st.native_keyboard.as_deref().is_none_or(|n| n == expected);
            ok.then_some(()).ok_or(format!("keyboard is {:?}, native {:?}", st.keyboard, st.native_keyboard))
        });
    }

    /// The element's resolved style (the `get_style` verb), once it exists.
    pub fn style(mut self) -> Style {
        let desc = format!("{}.style()", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            match self.read_style() {
                Ok(s) => return s,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.app.timeout, e.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// Assert on the element's resolved style until `check` holds or the
    /// timeout passes: `assert_style(|s| s.is_line_through())`.
    pub fn assert_style(mut self, check: impl Fn(&Style) -> bool) {
        let desc = format!("{}.assert_style(…)", self.sel.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let last = match self.read_style() {
                Ok(s) if check(&s) => return,
                Ok(s) => format!("the style is {s:?}"),
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) => e.message().to_string(),
            };
            if Instant::now() >= deadline {
                panic!("{desc} — still failing after {:?}: {last}", self.app.timeout);
            }
            std::thread::sleep(POLL);
        }
    }

    fn read_style(&mut self) -> Result<Style, Failure> {
        let el = self.resolve()?;
        let id = Self::id_of(&el)?;
        let out = self.send_action("get_style", json!({ "element_id": id }))?;
        style_of(&out).map_err(Failure::Fatal)
    }

    /// Assert the control refuses input.
    pub fn assert_disabled(self) {
        self.expect("assert_disabled()", |el| {
            let actual = state_of(el).disabled;
            (actual == Some(true)).then_some(()).ok_or(format!("disabled is {actual:?}"))
        });
    }

    /// Assert the control accepts input.
    pub fn assert_enabled(self) {
        self.expect("assert_enabled()", |el| {
            let actual = state_of(el).disabled;
            (actual == Some(false)).then_some(()).ok_or(format!("disabled is {actual:?}"))
        });
    }

    /// Assert whether a text field holds keyboard focus.
    pub fn assert_focused(self, expected: bool) {
        let what = format!("assert_focused({expected})");
        self.expect(&what, |el| {
            let actual = state_of(el).focused;
            (actual == Some(expected)).then_some(()).ok_or(format!("focused is {actual:?}"))
        });
    }

    /// Assert whether the element's `state hovered` style is on (waits for
    /// it, as every assertion does).
    pub fn assert_hovered(self, expected: bool) {
        let what = format!("assert_hovered({expected})");
        self.expect(&what, |el| {
            let actual = state_of(el).hovered;
            (actual == Some(expected)).then_some(()).ok_or(format!("hovered is {actual:?}"))
        });
    }

    /// Assert whether the element's `state pressed` style is on.
    pub fn assert_pressed(self, expected: bool) {
        let what = format!("assert_pressed({expected})");
        self.expect(&what, |el| {
            let actual = state_of(el).pressed;
            (actual == Some(expected)).then_some(()).ok_or(format!("pressed is {actual:?}"))
        });
    }

    /// Assert exactly `expected` elements match the selector.
    pub fn assert_count(self, expected: usize) {
        let desc = format!("{}.assert_count({expected})", self.sel.desc);
        let sel = self.sel;
        self.app.wait(&desc, |app| {
            let actual = app.find_all(&sel)?.len();
            if actual == expected { Ok(()) } else { Err(Failure::NotYet(format!("{actual} match"))) }
        });
    }
}

/// The `state` an element JSON carries (`{}` when it has none).
/// A `get_style` answer as a [`Style`].
fn style_of(out: &Value) -> Result<Style, String> {
    let mut style: Style = serde_json::from_value(out.get("style").cloned().unwrap_or(json!({})))
        .map_err(|e| format!("get_style answered {out}: {e}"))?;
    style.source = out.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string();
    Ok(style)
}

fn state_of(el: &Value) -> ElementState {
    el.get("state")
        .cloned()
        .map(|s| serde_json::from_value(s).unwrap_or_default())
        .unwrap_or_default()
}

/// A reported value against an expected one: numbers compare as numbers
/// (`5` matches a slider's `5.0`), anything else exactly.
fn values_match(actual: Option<&Value>, expected: &Value) -> bool {
    match (actual, expected) {
        (Some(a), e) if a.is_number() && e.is_number() => {
            (a.as_f64().unwrap_or(f64::NAN) - e.as_f64().unwrap_or(f64::NAN)).abs() < 1e-6
        }
        (Some(a), e) => a == e,
        (None, _) => false,
    }
}

/// One navigator, as `list_navigators` reports it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct NavigatorInfo {
    pub nav_id: u64,
    #[serde(default)]
    pub type_name: Option<String>,
    #[serde(default)]
    pub active_route: Option<String>,
    #[serde(default)]
    pub active_path: Option<String>,
    /// The active screen's query parameters, decoded (a repeated key
    /// reports its first value). Empty from an app too old to report it.
    #[serde(default)]
    pub query: std::collections::BTreeMap<String, String>,
    /// `active_path` plus the encoded query (`/assets?tag=~7781`). `None`
    /// from an app too old to report it.
    #[serde(default)]
    pub href: Option<String>,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub can_go_back: bool,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub controllable: bool,
}

/// A browser cookie: what [`App::set_cookie`] sets and [`App::cookies`]
/// returns. Unset fields take the browser's defaults when setting
/// (`url`/`domain`: the page's origin; a session cookie without
/// `expires`); `url` is only for setting and never comes back.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Hidden from page scripts (`document.cookie`); still sent to the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// `Strict`, `Lax` or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
    /// Seconds since the Unix epoch; a session cookie has none (the
    /// browser reports `-1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
}

impl Cookie {
    /// A cookie for the page's origin.
    pub fn new(name: &str, value: &str) -> Self {
        Self { name: name.to_string(), value: value.to_string(), ..Self::default() }
    }

    /// The same cookie, hidden from page scripts.
    pub fn http_only(mut self) -> Self {
        self.http_only = Some(true);
        self
    }
}

/// How far [`App::try_open_link`] got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkOutcome {
    /// Whether a navigator took the link — `false` when an interceptor
    /// claimed it or no screen matches.
    pub routed: bool,
    /// The deepest path the shown navigators reached.
    pub reached: String,
    /// Whether that is the link's whole path (and query, if it had one).
    pub complete: bool,
}

impl LinkOutcome {
    /// The `open_link` answer: `{reached, complete, routed}`, or the bare
    /// `true`/`false` of an app from before it reported progress (taken as
    /// complete when routed — such an app can't say more).
    fn from_answer(v: &Value) -> Option<Self> {
        if let Some(routed) = v.as_bool() {
            return Some(Self { routed, reached: String::new(), complete: routed });
        }
        Some(Self {
            routed: v["routed"].as_bool()?,
            reached: v["reached"].as_str()?.to_string(),
            complete: v["complete"].as_bool()?,
        })
    }
}

/// One robot function, as `list_functions` reports it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct FunctionInfo {
    pub name: String,
    /// Its doc comment.
    #[serde(default)]
    pub doc: String,
    #[serde(default)]
    pub args: Vec<FunctionArg>,
    /// The Rust return type as written.
    #[serde(default)]
    pub returns: String,
    #[serde(default, rename = "async")]
    pub is_async: bool,
}

/// One parameter of a [`FunctionInfo`].
#[derive(Clone, Debug, serde::Deserialize)]
pub struct FunctionArg {
    pub name: String,
    /// The Rust type as written.
    #[serde(rename = "type")]
    pub ty: String,
}

/// A mounted component instance — invoke its `#[method]`s, read its props.
pub struct Component<'a> {
    app: &'a mut App,
    instance_id: u64,
    desc: String,
}

impl Component<'_> {
    /// Run one of the component's `#[method]`s with named `args`
    /// (`json!({ "by": 3 })`).
    pub fn invoke(self, method: &str, args: Value) {
        self.app.call(
            "invoke_method",
            json!({ "instance_id": self.instance_id, "method": method, "args": args }),
        );
    }

    /// The live value of prop `name` (as the bridge reports it).
    pub fn prop(self, name: &str) -> Value {
        let desc = format!("{}.prop({name:?})", self.desc);
        let v = self.app.read(&desc, "get_component", json!({ "instance_id": self.instance_id }));
        v["props"]
            .as_array()
            .and_then(|props| props.iter().find(|p| p["name"] == name))
            .map(|p| p["value"].clone())
            .unwrap_or_else(|| panic!("{} has no prop {name:?}: {v}", self.desc))
    }
}

/// Assertion context for a watched signal — `app.signal("count").assert_eq(2)`.
pub struct SignalAssert<'a> {
    app: &'a mut App,
    name: String,
}

impl SignalAssert<'_> {
    fn read(&mut self) -> Result<String, String> {
        let v = self
            .app
            .try_call("read_signal", json!({ "name": self.name }))
            .map_err(|e| e.to_string())?;
        // Watched values come back as a JSON string of the value's `Debug` form;
        // unwrap the string layer so numbers/bools compare cleanly.
        Ok(match v {
            Value::String(s) => s,
            other => other.to_string(),
        })
    }

    fn expect(mut self, what: &str, mut check: impl FnMut(&str) -> bool) {
        let desc = format!("signal({:?}).{what}", self.name);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let last = match self.read() {
                Ok(v) if check(&v) => return,
                Ok(v) => format!("the value is {v:?}"),
                Err(e) => e,
            };
            if Instant::now() >= deadline {
                panic!("{desc} — still failing after {:?}: {last}", self.app.timeout);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Assert the signal's value equals `expected` (compared by display form, so
    /// `assert_eq(2)` matches an `i32` signal of `2`). Note: `String` signals
    /// come back `Debug`-quoted — assert against `"\"text\""` or use
    /// [`assert_contains`](Self::assert_contains).
    pub fn assert_eq(self, expected: impl std::fmt::Display) {
        let expected = expected.to_string();
        let what = format!("assert_eq({expected:?})");
        self.expect(&what, |v| v == expected);
    }

    /// Assert the signal's value (as text) contains `needle`.
    pub fn assert_contains(self, needle: &str) {
        let what = format!("assert_contains({needle:?})");
        self.expect(&what, |v| v.contains(needle));
    }

    /// The signal's current value as text (the `Debug` form, string layer
    /// unwrapped). Does not wait for a value; asks again while the app is
    /// busy.
    pub fn value(self) -> String {
        let desc = format!("signal({:?}).value()", self.name);
        let v = self.app.read(&desc, "read_signal", json!({ "name": self.name }));
        match v {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    /// Write the signal (it must be registered with `watch_signal_writable`).
    pub fn set(self, value: Value) {
        self.app.call("write_signal", json!({ "name": self.name, "value": value }));
    }
}
