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
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How long actions and assertions wait by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

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
        Self { client, timeout: DEFAULT_TIMEOUT }
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
        Locator { app: self, sel: selector.into() }
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
    /// otherwise the navigators move to it. Returns whether a navigator
    /// took it (or already showed it). Follow with
    /// [`assert_path`](Self::assert_path).
    pub fn open_link(&mut self, url: &str) -> bool {
        self.call("open_link", json!({ "url": url })) == true
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
}

impl<'a> Locator<'a> {
    fn new(app: &'a mut App, sel: Sel) -> Self {
        Locator { app, sel: sel.into() }
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
    /// enabled, a real platform tap at its center (`tap_native`) — through
    /// the toolkit's hit testing, so an overlay on top or an element
    /// scrolled out of the window makes it wait, then fail, as it would a
    /// user. On a backend that can't synthesize input it falls back to the
    /// element's own press handler ([`dispatch_click`](Self::dispatch_click)).
    pub fn click(self) {
        self.tap(true);
    }

    /// A real platform tap at the element's center, with no fallback:
    /// panics on a backend that can't synthesize input. Returns how it
    /// reached the element: `"native"` (platform events) or `"hit_test"`
    /// (the backend's hit test confirmed the point, then the press ran —
    /// iOS, where an app can't create touches).
    pub fn tap_native(self) -> String {
        self.tap(false)
    }

    /// Move a real mouse over the element (no button held). Panics on a
    /// platform with no hovering pointer.
    pub fn hover_native(self) {
        self.act("hover_native", json!({}));
    }

    /// Press the primary button down on the element with real input and
    /// keep it down; [`release_native`](Self::release_native) lifts it.
    pub fn press_native(self) {
        self.act("press_native", json!({}));
    }

    /// Lift a [`press_native`](Self::press_native) at the element.
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
    /// element's top-left, as [`drag`](Self::drag) takes them). Panics on a
    /// backend that can't synthesize input.
    pub fn drag_native(self, points: &[(f32, f32)]) {
        let pts: Vec<[f32; 2]> = points.iter().map(|&(x, y)| [x, y]).collect();
        self.act("drag_native", json!({ "points": pts }));
    }

    fn tap(mut self, fallback: bool) -> String {
        let desc = format!("{}.{}", self.sel.desc, if fallback { "click" } else { "tap_native" });
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
                match self.send_action("tap_native", json!({ "element_id": id })) {
                    Ok(v) => Ok(v["mechanism"].as_str().unwrap_or("native").to_string()),
                    Err(Failure::NotYet(e)) if e.contains("unsupported") => {
                        if !fallback {
                            return Err(Failure::Fatal(e));
                        }
                        self.send_action("click", json!({ "element_id": id })).map(|_| "robot".to_string())
                    }
                    Err(e) => Err(e),
                }
            });
            match attempt {
                Ok(how) => return how,
                Err(Failure::Fatal(e)) => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {}", self.app.timeout, e.message())
                }
                Err(_) => std::thread::sleep(POLL),
            }
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
            (_, Some(Value::Bool(false))) => Err("it is outside the window".into()),
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
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub can_go_back: bool,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub controllable: bool,
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
