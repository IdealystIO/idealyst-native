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

use crate::client::RobotClient;
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
        Locator::new(self, Sel::TestId(id.into()), format!("test_id({id:?})"))
    }

    /// Locate by visible text (substring match against the live label).
    pub fn text(&mut self, substring: &str) -> Locator<'_> {
        Locator::new(self, Sel::Text(substring.into()), format!("text({substring:?})"))
    }

    /// Locate by exact label (a button's text, a field's accessible name).
    pub fn label(&mut self, label: &str) -> Locator<'_> {
        Locator::new(self, Sel::Label(label.into()), format!("label({label:?})"))
    }

    /// Locate by primitive kind, e.g. `"Button"` / `"TextInput"` (the
    /// PascalCase `ElementKind` names).
    pub fn role(&mut self, kind: &str) -> Locator<'_> {
        Locator::new(self, Sel::Role(kind.into()), format!("role({kind:?})"))
    }

    /// How many elements currently match `kind` (e.g. count every `Button`).
    pub fn count(&mut self, kind: &str) -> usize {
        self.call("count_elements", json!({ "kind": kind })).as_u64().unwrap_or(0) as usize
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
        let v = self.call("list_navigators", json!({}));
        serde_json::from_value(v).unwrap_or_else(|e| panic!("list_navigators: unexpected shape ({e})"))
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
            let nav = app.current_navigator();
            if nav.active_path.as_deref() == Some(path) {
                Ok(())
            } else {
                Err(format!("the active path is {:?}", nav.active_path))
            }
        });
    }

    fn current_navigator(&mut self) -> NavigatorInfo {
        let navs = self.navigators();
        navs.iter()
            .rev()
            .find(|n| n.controllable && n.is_current)
            .or_else(|| navs.iter().rev().find(|n| n.controllable))
            .cloned()
            .unwrap_or_else(|| panic!("the app has no navigator that takes robot commands"))
    }

    // --- components ---

    /// The first mounted instance of the component named `name` (the
    /// `#[component]` fn's name), waiting for one to mount.
    pub fn component(&mut self, name: &str) -> Component<'_> {
        let desc = format!("component({name:?})");
        let instance_id = self.wait(&desc, |app| {
            let list = app.call("list_components", json!({}));
            list.as_array()
                .and_then(|l| l.iter().find(|c| c["name"] == name))
                .and_then(|c| c["instance_id"].as_u64())
                .ok_or_else(|| "no instance is mounted".to_string())
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
            match self.try_call("poll_function", json!({ "call": call })) {
                Ok(v) if v["done"] == true => return Ok(v["value"].clone()),
                Ok(_) if Instant::now() >= deadline => {
                    panic!("function({name:?}) — still running after {:?}", self.timeout)
                }
                Ok(_) => std::thread::sleep(POLL),
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// The functions the app registered with the robot.
    pub fn functions(&mut self) -> Vec<FunctionInfo> {
        serde_json::from_value(self.call("list_functions", json!({})))
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
        match self.call("get_logs", json!({})) {
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
    /// with `desc` and the last error.
    fn wait<T>(&mut self, desc: &str, mut attempt: impl FnMut(&mut App) -> Result<T, String>) -> T {
        let deadline = Instant::now() + self.timeout;
        loop {
            match attempt(self) {
                Ok(v) => return v,
                Err(last) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {last}", self.timeout)
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// The elements `sel` matches now, in document order.
    fn find_all(&mut self, sel: &Sel) -> Vec<Value> {
        match self.call("find_all_elements", sel.to_args()) {
            Value::Array(els) => els,
            other => panic!("find_all_elements: unexpected shape {other}"),
        }
    }
}

/// One element selector. Lazy: a [`Locator`] re-resolves on each action, so it
/// survives re-renders like a Playwright locator survives DOM churn.
#[derive(Clone)]
enum Sel {
    TestId(String),
    Text(String),
    Label(String),
    Role(String),
}

impl Sel {
    fn to_args(&self) -> Value {
        match self {
            Sel::TestId(s) => json!({ "test_id": s }),
            Sel::Text(s) => json!({ "label_contains": s }),
            Sel::Label(s) => json!({ "label": s }),
            Sel::Role(k) => json!({ "kind": k }),
        }
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
}

/// A scroll view's content offset, logical px.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
pub struct ScrollOffset {
    pub x: f64,
    pub y: f64,
}

/// A located (or to-be-located) element. Actions and assertions consume `self`
/// — chain straight off the locator: `app.test_id("inc").click()`.
pub struct Locator<'a> {
    app: &'a mut App,
    sel: Sel,
    nth: Option<usize>,
    desc: String,
}

impl<'a> Locator<'a> {
    fn new(app: &'a mut App, sel: Sel, desc: String) -> Self {
        Locator { app, sel, nth: None, desc }
    }

    /// The `index`th (0-based, document order) of the elements the selector
    /// matches — `app.role("TextInput").nth(1)` is the second field.
    pub fn nth(mut self, index: usize) -> Self {
        self.desc = format!("{}.nth({index})", self.desc);
        self.nth = Some(index);
        self
    }

    /// The element as the bridge reports it now, or why there is none.
    fn resolve(&mut self) -> Result<Value, String> {
        let index = self.nth.unwrap_or(0);
        let matches = match (&self.sel, self.nth) {
            // A single test_id lookup is one indexed hit (last mounted wins).
            (Sel::TestId(_), None) => {
                let v = self.app.call("find_element", self.sel.to_args());
                if v.is_null() { Vec::new() } else { vec![v] }
            }
            _ => self.app.find_all(&self.sel),
        };
        match matches.into_iter().nth(index) {
            Some(v) => Ok(v),
            None if self.nth.is_some() => Err(format!("fewer than {} elements match", index + 1)),
            None => Err("no element matches".into()),
        }
    }

    fn id_of(el: &Value) -> Result<u64, String> {
        el.get("id").and_then(|i| i.as_u64()).ok_or_else(|| format!("an element with no id: {el}"))
    }

    /// Run `verb` on the element, waiting until it exists and accepts it — a
    /// disabled control refuses, so this waits for it to be enabled.
    fn act(mut self, verb: &str, args: Value) {
        let desc = format!("{}.{verb}", self.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let attempt = self.resolve().and_then(|el| Self::id_of(&el)).and_then(|id| {
                let mut args = args.clone();
                if let Value::Object(map) = &mut args {
                    map.insert("element_id".into(), json!(id));
                }
                self.app.try_call(verb, args).map(|_| ()).map_err(|e| e.to_string())
            });
            match attempt {
                Ok(()) => return,
                // Unavailable actions don't come and go: say so at once.
                Err(e) if e.contains("not available") => panic!("{desc}: {e}"),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {e}", self.app.timeout)
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// Retry `check` against the resolved element until it holds or the
    /// timeout passes.
    fn expect(mut self, what: &str, mut check: impl FnMut(&Value) -> Result<(), String>) {
        let desc = format!("{}.{what}", self.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let last = match self.resolve() {
                Ok(el) => match check(&el) {
                    Ok(()) => return,
                    Err(e) => e,
                },
                Err(e) => e,
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
        let desc = format!("{}.{}", self.desc, if fallback { "click" } else { "tap_native" });
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let attempt = self.resolve().and_then(|el| {
                if el["visible"] == false {
                    return Err("the element is not visible".to_string());
                }
                if el["state"]["disabled"] == true {
                    return Err("the element is disabled".to_string());
                }
                let id = Self::id_of(&el)?;
                match self.app.try_call("tap_native", json!({ "element_id": id })) {
                    Ok(v) => Ok(v["mechanism"].as_str().unwrap_or("native").to_string()),
                    Err(e) if fallback && e.to_string().contains("unsupported") => self
                        .app
                        .try_call("click", json!({ "element_id": id }))
                        .map(|_| "robot".to_string())
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            });
            match attempt {
                Ok(how) => return how,
                Err(e) if e.contains("not available") || (!fallback && e.contains("unsupported")) => {
                    panic!("{desc}: {e}")
                }
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {e}", self.app.timeout)
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
        let desc = format!("{}.state()", self.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            match self.resolve() {
                Ok(el) => return state_of(&el),
                Err(e) if Instant::now() >= deadline => {
                    panic!("{desc} — still failing after {:?}: {e}", self.app.timeout)
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    /// The element's current label, or `None` if it has none / isn't present
    /// (does not wait).
    pub fn label(mut self) -> Option<String> {
        self.resolve().ok()?.get("label").and_then(|l| l.as_str()).map(str::to_string)
    }

    /// How many elements the selector matches now (does not wait).
    pub fn count(self) -> usize {
        self.app.find_all(&self.sel).len()
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
        let desc = format!("{}.assert_hidden()", self.desc);
        let deadline = Instant::now() + self.app.timeout;
        while let Ok(el) = self.resolve() {
            if el.get("visible") == Some(&Value::Bool(false)) {
                return;
            }
            if Instant::now() >= deadline {
                panic!("{desc} — the element is still visible after {:?}", self.app.timeout);
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
        self.resolve().is_ok_and(|el| el.get("visible") != Some(&Value::Bool(false)))
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

    /// Assert exactly `expected` elements match the selector.
    pub fn assert_count(self, expected: usize) {
        let desc = format!("{}.assert_count({expected})", self.desc);
        let deadline = Instant::now() + self.app.timeout;
        loop {
            let actual = self.app.find_all(&self.sel).len();
            if actual == expected {
                return;
            }
            if Instant::now() >= deadline {
                panic!("{desc} — {actual} match after {:?}", self.app.timeout);
            }
            std::thread::sleep(POLL);
        }
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
        let v = self.app.call("get_component", json!({ "instance_id": self.instance_id }));
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
    /// unwrapped). Does not wait.
    pub fn value(mut self) -> String {
        let name = self.name.clone();
        self.read().unwrap_or_else(|e| panic!("signal({name:?}).value(): {e}"))
    }

    /// Write the signal (it must be registered with `watch_signal_writable`).
    pub fn set(self, value: Value) {
        self.app.call("write_signal", json!({ "name": self.name, "value": value }));
    }
}
