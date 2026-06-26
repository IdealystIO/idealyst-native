//! The author-facing test surface: a relay-connected [`App`] handle and the
//! Playwright-flavoured [`Locator`] it hands out. Assertions **panic** on
//! failure (caught per-test by the libtest harness), so a `#[robot_test]` body
//! reads like an ordinary Rust test — no `Result`/`?` ceremony.
//!
//! Every call goes over the Robot bridge ([`RobotClient`]), so the same code
//! drives a web, macOS, iOS, or Android app — whichever one the relay has.

use crate::client::RobotClient;
use serde_json::{json, Value};
use std::net::SocketAddr;

/// A connection to the running app under test — the handle a `#[robot_test]`
/// body receives. Locate elements with [`App::test_id`] / [`App::text`] /
/// [`App::role`], read signals with [`App::signal`].
pub struct App {
    client: RobotClient,
}

impl App {
    /// Connect to an app's Robot bridge at `addr` (a direct bridge or, more
    /// usually, the relay's TCP port).
    pub fn connect(addr: SocketAddr) -> anyhow::Result<Self> {
        Ok(Self {
            client: RobotClient::connect(addr)?,
        })
    }

    /// Wrap an already-connected client (used by the harness, which connects +
    /// waits for readiness before handing the app to tests).
    pub(crate) fn from_client(client: RobotClient) -> Self {
        Self { client }
    }

    /// Locate by `test_id` — the durable, refactor-proof hook (`.test_id("inc")`
    /// in your component).
    pub fn test_id(&mut self, id: &str) -> Locator<'_> {
        Locator::new(self, Sel::TestId(id.into()), format!("test_id({id:?})"))
    }

    /// Locate by visible text (substring match against the live label).
    pub fn text(&mut self, substring: &str) -> Locator<'_> {
        Locator::new(
            self,
            Sel::Text(substring.into()),
            format!("text({substring:?})"),
        )
    }

    /// Locate by primitive kind, e.g. `"Button"` / `"TextInput"` (the
    /// PascalCase `ElementKind` names).
    pub fn role(&mut self, kind: &str) -> Locator<'_> {
        Locator::new(self, Sel::Role(kind.into()), format!("role({kind:?})"))
    }

    /// Assert on a watched signal by name. The app must expose it via
    /// `runtime_core::robot::watch_signal("name", sig)` (a bare `signal!` is not
    /// readable by name).
    pub fn signal(&mut self, name: &str) -> SignalAssert<'_> {
        SignalAssert {
            app: self,
            name: name.into(),
        }
    }

    /// Capture a screenshot; returns the saved path (or `"(in memory)"` if the
    /// relay didn't persist one). Panics on a bridge error.
    pub fn screenshot(&mut self) -> String {
        let v = self
            .client
            .call("screenshot", json!({}))
            .unwrap_or_else(|e| panic!("screenshot failed: {e}"));
        v.get("path")
            .and_then(|p| p.as_str())
            .unwrap_or("(in memory)")
            .to_string()
    }

    /// How many elements currently match `kind` (e.g. count every `Button`).
    pub fn count(&mut self, kind: &str) -> usize {
        let v = self
            .client
            .call("count_elements", json!({ "kind": kind }))
            .unwrap_or_else(|e| panic!("count_elements({kind:?}) failed: {e}"));
        v.as_u64().unwrap_or(0) as usize
    }

    // --- internals shared with Locator ---

    fn find(&mut self, sel: &Sel) -> Option<Value> {
        let v = self
            .client
            .call("find_element", sel.to_args())
            .unwrap_or_else(|e| panic!("find_element failed: {e}"));
        if v.is_null() {
            None
        } else {
            Some(v)
        }
    }

    fn act(&mut self, verb: &str, element_id: u64, mut args: Value) {
        if let Value::Object(map) = &mut args {
            map.insert("element_id".into(), json!(element_id));
        }
        self.client
            .call(verb, args)
            .unwrap_or_else(|e| panic!("{verb} failed: {e}"));
    }
}

/// One element selector. Lazy: a [`Locator`] re-resolves on each action, so it
/// survives re-renders like a Playwright locator survives DOM churn.
#[derive(Clone)]
enum Sel {
    TestId(String),
    Text(String),
    Role(String),
}

impl Sel {
    fn to_args(&self) -> Value {
        match self {
            Sel::TestId(s) => json!({ "test_id": s }),
            Sel::Text(s) => json!({ "label_contains": s }),
            Sel::Role(k) => json!({ "kind": k }),
        }
    }
}

/// A located (or to-be-located) element. Actions and assertions consume `self`
/// — chain straight off the locator: `app.test_id("inc").click()`.
pub struct Locator<'a> {
    app: &'a mut App,
    sel: Sel,
    desc: String,
}

impl<'a> Locator<'a> {
    fn new(app: &'a mut App, sel: Sel, desc: String) -> Self {
        Locator { app, sel, desc }
    }

    /// Resolve to the element id, panicking with a clear message if absent.
    fn resolve_id(&mut self) -> u64 {
        let sel = self.sel.clone();
        match self.app.find(&sel) {
            Some(v) => v
                .get("id")
                .and_then(|i| i.as_u64())
                .unwrap_or_else(|| panic!("{} resolved to an element with no id: {v}", self.desc)),
            None => panic!("{} matched no element", self.desc),
        }
    }

    /// Click / press the element.
    pub fn click(mut self) {
        let id = self.resolve_id();
        self.app.act("click", id, json!({}));
    }

    /// Type text into a text input (sets the value, like Playwright `fill`).
    pub fn type_text(mut self, text: &str) {
        let id = self.resolve_id();
        self.app.act("type_text", id, json!({ "text": text }));
    }

    /// Set a toggle on/off.
    pub fn set_toggle(mut self, on: bool) {
        let id = self.resolve_id();
        self.app.act("set_toggle", id, json!({ "value": on }));
    }

    /// Set a slider to `value` (its backend's scale, typically 0.0–1.0).
    pub fn set_slider(mut self, value: f64) {
        let id = self.resolve_id();
        self.app.act("set_slider", id, json!({ "value": value }));
    }

    /// Assert the element is present in the live tree.
    pub fn assert_visible(self) {
        let sel = self.sel.clone();
        if self.app.find(&sel).is_none() {
            panic!("{}.assert_visible() — element not found", self.desc);
        }
    }

    /// Assert the element is absent from the live tree.
    pub fn assert_hidden(self) {
        let sel = self.sel.clone();
        if self.app.find(&sel).is_some() {
            panic!("{}.assert_hidden() — element is present", self.desc);
        }
    }

    /// Assert the element's live label equals `expected` (exact match).
    pub fn assert_text(self, expected: &str) {
        let sel = self.sel.clone();
        let el = self
            .app
            .find(&sel)
            .unwrap_or_else(|| panic!("{}.assert_text({expected:?}) — element not found", self.desc));
        let actual = el.get("label").and_then(|l| l.as_str());
        if actual != Some(expected) {
            panic!(
                "{}.assert_text({expected:?}) — actual label was {:?}",
                self.desc, actual
            );
        }
    }

    /// Assert the element's live label contains `needle`.
    pub fn assert_text_contains(self, needle: &str) {
        let sel = self.sel.clone();
        let el = self.app.find(&sel).unwrap_or_else(|| {
            panic!("{}.assert_text_contains({needle:?}) — element not found", self.desc)
        });
        let actual = el.get("label").and_then(|l| l.as_str()).unwrap_or("");
        if !actual.contains(needle) {
            panic!(
                "{}.assert_text_contains({needle:?}) — actual label was {actual:?}",
                self.desc
            );
        }
    }

    /// The element's current label, or `None` if it has none / isn't present.
    pub fn label(self) -> Option<String> {
        let sel = self.sel.clone();
        self.app
            .find(&sel)?
            .get("label")
            .and_then(|l| l.as_str())
            .map(|s| s.to_string())
    }
}

/// Assertion context for a watched signal — `app.signal("count").assert_eq(2)`.
pub struct SignalAssert<'a> {
    app: &'a mut App,
    name: String,
}

impl<'a> SignalAssert<'a> {
    fn read(&mut self) -> String {
        let v = self
            .app
            .client
            .call("read_signal", json!({ "name": self.name }))
            .unwrap_or_else(|e| panic!("read_signal({:?}) failed: {e}", self.name));
        // Watched values come back as a JSON string of the value's `Debug` form;
        // unwrap the string layer so numbers/bools compare cleanly.
        match v {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    /// Assert the signal's value equals `expected` (compared by display form, so
    /// `assert_eq(2)` matches an `i32` signal of `2`). Note: `String` signals
    /// come back `Debug`-quoted — assert against `"\"text\""` or use
    /// [`assert_contains`](Self::assert_contains).
    pub fn assert_eq(mut self, expected: impl std::fmt::Display) {
        let expected = expected.to_string();
        let actual = self.read();
        if actual != expected {
            panic!(
                "signal({:?}).assert_eq({expected:?}) — actual value was {actual:?}",
                self.name
            );
        }
    }

    /// Assert the signal's value (as text) contains `needle`.
    pub fn assert_contains(mut self, needle: &str) {
        let actual = self.read();
        if !actual.contains(needle) {
            panic!(
                "signal({:?}).assert_contains({needle:?}) — actual value was {actual:?}",
                self.name
            );
        }
    }

    /// The signal's current value as text (the `Debug` form, string layer
    /// unwrapped).
    pub fn value(mut self) -> String {
        self.read()
    }
}
