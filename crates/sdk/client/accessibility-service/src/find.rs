//! Finding elements on screen by what they are, and pressing them.
//!
//! The search runs in the platform half, on a background thread: it walks
//! every interactive window's node tree, and each step is a call into the
//! app that owns the window. Only the matches cross back, each with a handle
//! the platform half keeps so [`click`] can act on that same element later.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::{policy, BoxFuture, Error, Result};

/// What [`find`] looks for. Every criterion set must hold; a query with none
/// matches every element on screen.
///
/// ```ignore
/// use accessibility_service::Query;
///
/// // The "Start now" button of Android's screen-capture prompt:
/// let q = Query::new()
///     .package("com.android.systemui")
///     .view_id("android:id/button1");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Query {
    /// The view's resource name. `"android:id/button1"` matches exactly; a
    /// name with no `:` (`"button1"`) matches that id in any package.
    pub view_id: Option<String>,
    /// The element's text.
    pub text: Option<TextMatch>,
    /// The element's content description: what a screen reader says for an
    /// icon or image button that has no text.
    pub description: Option<TextMatch>,
    /// The package of the app that owns the window.
    pub package: Option<String>,
    /// The view's class. `"android.widget.Button"` matches exactly; a name
    /// with no `.` (`"Button"`) matches the class's simple name.
    pub class_name: Option<String>,
}

/// How [`Query::text`] and [`Query::description`] compare. Both ignore
/// case: a button styled in capitals reports its text as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextMatch {
    /// The whole text, ignoring case.
    Exact(String),
    /// Anywhere in the text, ignoring case.
    Contains(String),
}

impl Query {
    /// A query with no criteria.
    pub fn new() -> Self {
        Self::default()
    }

    /// Match the view's resource name; see [`Query::view_id`](Self#structfield.view_id).
    pub fn view_id(mut self, id: impl Into<String>) -> Self {
        self.view_id = Some(id.into());
        self
    }

    /// Match the whole text, ignoring case.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(TextMatch::Exact(text.into()));
        self
    }

    /// Match text containing `part`, ignoring case.
    pub fn text_contains(mut self, part: impl Into<String>) -> Self {
        self.text = Some(TextMatch::Contains(part.into()));
        self
    }

    /// Match the whole content description, ignoring case.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(TextMatch::Exact(text.into()));
        self
    }

    /// Match a content description containing `part`, ignoring case.
    pub fn description_contains(mut self, part: impl Into<String>) -> Self {
        self.description = Some(TextMatch::Contains(part.into()));
        self
    }

    /// Match the package of the app that owns the window.
    pub fn package(mut self, package: impl Into<String>) -> Self {
        self.package = Some(package.into());
        self
    }

    /// Match the view's class; see [`Query::class_name`](Self#structfield.class_name).
    pub fn class_name(mut self, class: impl Into<String>) -> Self {
        self.class_name = Some(class.into());
        self
    }

    /// The query as the platform half reads it: unset criteria are left out.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn to_json(&self) -> Value {
        let mut o = Map::new();
        let text_match = |m: &TextMatch| match m {
            TextMatch::Exact(s) => json!({ "exact": s }),
            TextMatch::Contains(s) => json!({ "contains": s }),
        };
        if let Some(v) = &self.view_id {
            o.insert("view_id".into(), json!(v));
        }
        if let Some(m) = &self.text {
            o.insert("text".into(), text_match(m));
        }
        if let Some(m) = &self.description {
            o.insert("description".into(), text_match(m));
        }
        if let Some(v) = &self.package {
            o.insert("package".into(), json!(v));
        }
        if let Some(v) = &self.class_name {
            o.insert("class_name".into(), json!(v));
        }
        Value::Object(o)
    }
}

/// A rectangle on screen, in screenshot pixels: `right` and `bottom` are
/// exclusive, as Android's `Rect`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Bounds {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// One past the right edge.
    pub right: i32,
    /// One past the bottom edge.
    pub bottom: i32,
}

impl Bounds {
    /// Width in pixels (0 when empty).
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }

    /// Height in pixels (0 when empty).
    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }

    /// The centre, the point a [`tap`](crate::tap) on it would use.
    pub fn center(&self) -> (f32, f32) {
        (
            (self.left as f32 + self.right as f32) / 2.0,
            (self.top as f32 + self.bottom as f32) / 2.0,
        )
    }

    pub(crate) fn from_json(v: &Value) -> Result<Self> {
        let n = |i: usize| -> Result<i32> {
            v.get(i)
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| malformed("bounds", v))
        };
        Ok(Self {
            left: n(0)?,
            top: n(1)?,
            right: n(2)?,
            bottom: n(3)?,
        })
    }
}

/// One element on screen, as [`find`] saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Node {
    /// Where it is on screen, in screenshot pixels.
    pub bounds: Bounds,
    /// Its text, if it has any.
    pub text: Option<String>,
    /// Its content description, if it has one.
    pub description: Option<String>,
    /// Its view's resource name (`"android:id/button1"`), if it has one.
    pub view_id: Option<String>,
    /// The package of the app that owns its window.
    pub package: Option<String>,
    /// Its view's class (`"android.widget.Button"`).
    pub class_name: Option<String>,
    /// It reacts to a click itself. A label inside a clickable row is not
    /// clickable; [`click`] taps it, which presses the row.
    pub clickable: bool,
    /// It is enabled.
    pub enabled: bool,
    /// The window it is in ([`WindowInfo::id`](crate::WindowInfo::id)).
    pub window_id: i32,
    /// The platform half's handle on the element, for [`click`].
    handle: u64,
}

impl Node {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn from_json(v: &Value) -> Result<Self> {
        Ok(Self {
            bounds: Bounds::from_json(v.get("bounds").unwrap_or(&Value::Null))?,
            text: opt_string(v, "text"),
            description: opt_string(v, "description"),
            view_id: opt_string(v, "view_id"),
            package: opt_string(v, "package"),
            class_name: opt_string(v, "class_name"),
            clickable: v.get("clickable").and_then(Value::as_bool).unwrap_or(false),
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            window_id: opt_i32(v, "window_id").unwrap_or(-1),
            handle: v
                .get("handle")
                .and_then(Value::as_u64)
                .ok_or_else(|| malformed("node", v))?,
        })
    }

    /// A list of nodes, as `find` and `wait_for` reply.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn list_from_json(v: &Value) -> Result<Vec<Self>> {
        v.as_array()
            .ok_or_else(|| malformed("node list", v))?
            .iter()
            .map(Self::from_json)
            .collect()
    }
}

pub(crate) fn opt_string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub(crate) fn opt_i32(v: &Value, key: &str) -> Option<i32> {
    v.get(key)
        .and_then(Value::as_i64)
        .and_then(|n| i32::try_from(n).ok())
}

pub(crate) fn malformed(what: &str, v: &Value) -> Error {
    Error::Platform(format!("malformed {what} from the platform: {v}"))
}

/// Every element on screen that matches `query`, in every interactive
/// window (the app in front, a system dialog over it, the status bar, the
/// keyboard), each window's elements in tree order. Empty when none match.
///
/// Android lists an element only while its window is on screen and the app
/// drawing it reports it to accessibility; a view an app hides from
/// accessibility services is not found.
pub fn find(query: &Query) -> BoxFuture<Vec<Node>> {
    search(query, None)
}

/// Like [`find`], but waits up to `timeout` for at least one match, checking
/// again every [`WAIT_POLL`]. [`Error::Timeout`] when none appears in time.
/// A zero `timeout` checks once.
///
/// The timeout bounds a search in progress too: an app whose UI thread is
/// busy answers each step of the walk slowly, so at the deadline the walk
/// stops and returns what it has matched so far (`Timeout` if nothing).
pub fn wait_for(query: &Query, timeout: Duration) -> BoxFuture<Vec<Node>> {
    search(query, Some(timeout))
}

/// How often [`wait_for`] checks the screen again.
pub const WAIT_POLL: Duration = Duration::from_millis(policy::WAIT_POLL_MS as u64);

fn search(query: &Query, timeout: Option<Duration>) -> BoxFuture<Vec<Node>> {
    let mut args = json!({
        "query": query.to_json(),
        "poll_ms": policy::WAIT_POLL_MS,
        "node_handles": policy::NODE_HANDLES,
    });
    if let Some(t) = timeout {
        args["timeout_ms"] = json!(u64::try_from(t.as_millis()).unwrap_or(u64::MAX));
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        let fut = crate::android::call_async("find", &args);
        Box::pin(async move { Node::list_from_json(&fut.await?) })
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    {
        let _ = args;
        crate::ready(Err(crate::unsupported(if timeout.is_some() {
            "wait_for"
        } else {
            "find"
        })))
    }
}

/// Press `node`. Android is asked to click the element itself
/// (`ACTION_CLICK`), which works wherever it is and whatever is drawn over
/// it; when the element does not take a click (a label inside a clickable
/// row, or a view that ignores the action) the SDK taps the centre of its
/// bounds instead, as read at the time of the click. Resolves when the
/// click is done.
///
/// [`Error::StaleNode`] when the element is no longer on screen. The SDK
/// keeps the most recent [`NODE_HANDLES`] elements found; an older one is
/// stale too, so find again.
pub fn click(node: &Node) -> BoxFuture<()> {
    let args = json!({ "node": node.handle, "tap_ms": policy::TAP_MS });
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        let fut = crate::android::call_async("click", &args);
        Box::pin(async move { fut.await.map(drop) })
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    {
        let _ = args;
        crate::ready(Err(crate::unsupported("click")))
    }
}

/// How many found elements [`click`] can still act on: the most recent
/// ones, across every [`find`] and [`wait_for`].
pub const NODE_HANDLES: usize = policy::NODE_HANDLES;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_sends_only_the_criteria_it_sets() {
        assert_eq!(Query::new().to_json(), json!({}));
        let q = Query::new()
            .package("com.android.systemui")
            .view_id("android:id/button1")
            .text_contains("start")
            .description("Close")
            .class_name("Button");
        assert_eq!(
            q.to_json(),
            json!({
                "package": "com.android.systemui",
                "view_id": "android:id/button1",
                "text": { "contains": "start" },
                "description": { "exact": "Close" },
                "class_name": "Button",
            })
        );
        assert_eq!(Query::new().text("Start now").to_json(), json!({ "text": { "exact": "Start now" } }));
    }

    #[test]
    fn a_node_parses_with_its_handle_and_missing_fields_as_none() {
        let n = Node::from_json(&json!({
            "handle": 7,
            "window_id": 12,
            "bounds": [100, 200, 300, 260],
            "text": "Start now",
            "view_id": "android:id/button1",
            "package": "com.android.systemui",
            "class_name": "android.widget.Button",
            "clickable": true,
            "enabled": true,
        }))
        .unwrap();
        assert_eq!(n.handle, 7);
        assert_eq!(n.window_id, 12);
        assert_eq!(n.bounds, Bounds { left: 100, top: 200, right: 300, bottom: 260 });
        assert_eq!(n.bounds.center(), (200.0, 230.0));
        assert_eq!(n.text.as_deref(), Some("Start now"));
        assert_eq!(n.description, None);
        assert!(n.clickable && n.enabled);
    }

    #[test]
    fn a_node_without_a_handle_or_bounds_is_malformed() {
        assert!(Node::from_json(&json!({ "bounds": [0, 0, 1, 1] })).is_err());
        assert!(Node::from_json(&json!({ "handle": 1 })).is_err());
        assert!(Node::from_json(&json!({ "handle": 1, "bounds": [0, 0, 1] })).is_err());
        assert!(Node::list_from_json(&json!({ "handle": 1 })).is_err());
        assert_eq!(Node::list_from_json(&json!([])), Ok(vec![]));
    }

    #[test]
    fn empty_bounds_have_no_size() {
        let b = Bounds { left: 10, top: 10, right: 5, bottom: 10 };
        assert_eq!((b.width(), b.height()), (0, 0));
    }
}
