//! The windows on screen, and notifications when they change.

use std::cell::RefCell;

use serde_json::Value;

use crate::find::{malformed, opt_i32, opt_string, Bounds};
use crate::listeners::{deliver, Listeners};
use crate::{BoxFuture, Result};

/// What a window is (`AccessibilityWindowInfo.getType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WindowKind {
    /// An app's window.
    Application,
    /// The on-screen keyboard.
    InputMethod,
    /// A system window: the status bar, the navigation bar, a system
    /// dialog, and on Android 16 the desktop-windowing app handle.
    System,
    /// An accessibility service's own overlay.
    AccessibilityOverlay,
    /// The divider between two apps in split screen.
    SplitScreenDivider,
    /// The magnification overlay.
    MagnificationOverlay,
    /// A type this SDK does not know, as Android numbers it.
    Other(i32),
}

impl WindowKind {
    pub(crate) fn from_android(t: i32) -> Self {
        match t {
            1 => WindowKind::Application,
            2 => WindowKind::InputMethod,
            3 => WindowKind::System,
            4 => WindowKind::AccessibilityOverlay,
            5 => WindowKind::SplitScreenDivider,
            6 => WindowKind::MagnificationOverlay,
            other => WindowKind::Other(other),
        }
    }
}

/// One window on screen ([`windows`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WindowInfo {
    /// Android's id for the window, as [`Node::window_id`](crate::Node::window_id)
    /// and [`WindowEvent::window_id`] give it.
    pub id: i32,
    /// What the window is.
    pub kind: WindowKind,
    /// Its place in the stack: a higher layer is drawn over a lower one.
    pub layer: i32,
    /// Where it is on screen, in screenshot pixels.
    pub bounds: Bounds,
    /// The package of the app that draws it, when Android lets the service
    /// read the window's content.
    pub package: Option<String>,
    /// Its title, if it has one (an app's window is usually titled with its
    /// app's label).
    pub title: Option<String>,
    /// It has input focus.
    pub focused: bool,
    /// It is the window the user is using (the one with focus, or the one
    /// last touched).
    pub active: bool,
    /// It is a picture-in-picture window. API 26+; `false` below.
    pub picture_in_picture: bool,
    /// The display it is on. API 30+; `None` below.
    pub display_id: Option<i32>,
}

impl WindowInfo {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn from_json(v: &Value) -> Result<Self> {
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Ok(Self {
            id: opt_i32(v, "id").ok_or_else(|| malformed("window", v))?,
            kind: WindowKind::from_android(opt_i32(v, "type").ok_or_else(|| malformed("window", v))?),
            layer: opt_i32(v, "layer").unwrap_or(0),
            bounds: Bounds::from_json(v.get("bounds").unwrap_or(&Value::Null))?,
            package: opt_string(v, "package"),
            title: opt_string(v, "title"),
            focused: flag("focused"),
            active: flag("active"),
            picture_in_picture: flag("picture_in_picture"),
            display_id: opt_i32(v, "display_id"),
        })
    }

    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn list_from_json(v: &Value) -> Result<Vec<Self>> {
        v.as_array()
            .ok_or_else(|| malformed("window list", v))?
            .iter()
            .map(Self::from_json)
            .collect()
    }
}

/// The windows on the default display, top of the stack first
/// (`AccessibilityService.getWindows`): the app in front, any dialog or
/// system UI over it, the status and navigation bars, the keyboard.
pub fn windows() -> BoxFuture<Vec<WindowInfo>> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        let fut = crate::android::call_async("windows", &serde_json::json!({}));
        Box::pin(async move { WindowInfo::list_from_json(&fut.await?) })
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    crate::ready(Err(crate::unsupported("windows")))
}

/// What a [`WindowEvent`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowEventKind {
    /// The set of windows, or one of them, changed
    /// (`TYPE_WINDOWS_CHANGED`): what changed is listed, empty below API 28.
    WindowsChanged(Vec<WindowChange>),
    /// A window's content changed state: an activity, dialog, popup or
    /// menu opened (`TYPE_WINDOW_STATE_CHANGED`). `package` and
    /// `class_name` say which.
    StateChanged,
}

/// One kind of change in a [`WindowEventKind::WindowsChanged`]
/// (`AccessibilityEvent.WINDOWS_CHANGE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WindowChange {
    /// A window appeared.
    Added,
    /// A window went away.
    Removed,
    /// A window's title changed.
    Title,
    /// A window moved or was resized.
    Bounds,
    /// A window moved in the stack.
    Layer,
    /// The active window changed.
    Active,
    /// The focused window changed.
    Focused,
    /// The window with accessibility focus changed.
    AccessibilityFocused,
    /// A window's parent changed.
    Parent,
    /// A window's child windows changed.
    Children,
    /// A window entered or left picture-in-picture.
    PictureInPicture,
}

impl WindowChange {
    const ALL: [(i64, WindowChange); 11] = [
        (0x1, WindowChange::Added),
        (0x2, WindowChange::Removed),
        (0x4, WindowChange::Title),
        (0x8, WindowChange::Bounds),
        (0x10, WindowChange::Layer),
        (0x20, WindowChange::Active),
        (0x40, WindowChange::Focused),
        (0x80, WindowChange::AccessibilityFocused),
        (0x100, WindowChange::Parent),
        (0x200, WindowChange::Children),
        (0x400, WindowChange::PictureInPicture),
    ];

    /// `AccessibilityEvent.getWindowChanges()`, decoded. Unknown bits are
    /// dropped.
    pub(crate) fn from_bits(bits: i64) -> Vec<WindowChange> {
        Self::ALL
            .iter()
            .filter(|(bit, _)| bits & bit != 0)
            .map(|(_, c)| *c)
            .collect()
    }
}

/// A change to the windows on screen ([`on_window_change`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WindowEvent {
    /// What happened.
    pub kind: WindowEventKind,
    /// The package the event came from, when Android says.
    pub package: Option<String>,
    /// The class that changed state (an activity's, a dialog's), when
    /// Android says.
    pub class_name: Option<String>,
    /// The window it concerns, when Android says.
    pub window_id: Option<i32>,
    /// The display it happened on. API 30+; `None` below.
    pub display_id: Option<i32>,
}

/// `AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED`.
const TYPE_WINDOW_STATE_CHANGED: i64 = 0x20;
/// `AccessibilityEvent.TYPE_WINDOWS_CHANGED`.
const TYPE_WINDOWS_CHANGED: i64 = 0x40_0000;

impl WindowEvent {
    /// The platform half's event JSON; `None` for an event type that is not
    /// a window change.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn from_json(v: &Value) -> Option<Self> {
        let kind = match v.get("type").and_then(Value::as_i64)? {
            TYPE_WINDOWS_CHANGED => WindowEventKind::WindowsChanged(WindowChange::from_bits(
                v.get("changes").and_then(Value::as_i64).unwrap_or(0),
            )),
            TYPE_WINDOW_STATE_CHANGED => WindowEventKind::StateChanged,
            _ => return None,
        };
        // Android numbers "no window" -1 and "no display" -1.
        let id = |k: &str| opt_i32(v, k).filter(|n| *n >= 0);
        Some(Self {
            kind,
            package: opt_string(v, "package"),
            class_name: opt_string(v, "class_name"),
            window_id: id("window_id"),
            display_id: id("display_id"),
        })
    }
}

runtime_tls::thread_local! {
    static LISTENERS: RefCell<Listeners<WindowEvent>> = const { RefCell::new(Listeners::new()) };
}

/// Call `listener` whenever the windows on screen change: a window
/// appears, goes away, moves or changes focus
/// ([`WindowEventKind::WindowsChanged`]), or an activity, dialog or menu
/// opens in one ([`WindowEventKind::StateChanged`]). Read [`windows`] for
/// the state after the change.
///
/// Android holds back a burst of events of one type and delivers the last,
/// at most one every 100 ms. Delivery stops when the returned subscription
/// drops. Call on the UI thread. Off Android the listener is kept and never
/// called.
#[must_use = "notifications stop when the subscription is dropped"]
pub fn on_window_change(listener: impl Fn(WindowEvent) + 'static) -> WindowSubscription {
    let (id, first) = LISTENERS.with(|l| l.borrow_mut().add(listener));
    if first {
        crate::platform_listening("windows", true);
    }
    WindowSubscription { id }
}

/// Keeps an [`on_window_change`] listener registered; dropping it
/// unregisters.
#[derive(Debug)]
pub struct WindowSubscription {
    id: u64,
}

impl Drop for WindowSubscription {
    fn drop(&mut self) {
        if LISTENERS.with(|l| l.borrow_mut().remove(self.id)) {
            crate::platform_listening("windows", false);
        }
    }
}

/// Deliver a window event to every listener.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn dispatch(event: WindowEvent) {
    deliver(LISTENERS.with(|l| l.borrow().snapshot()), event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::rc::Rc;

    #[test]
    fn a_window_parses_with_its_kind_and_optional_fields() {
        let w = WindowInfo::from_json(&json!({
            "id": 31, "type": 3, "layer": 12, "bounds": [0, 0, 2560, 96],
            "title": "Handle menu", "focused": false, "active": true, "display_id": 0,
        }))
        .unwrap();
        assert_eq!(w.id, 31);
        assert_eq!(w.kind, WindowKind::System);
        assert_eq!(w.layer, 12);
        assert_eq!(w.bounds.width(), 2560);
        assert_eq!(w.package, None);
        assert_eq!(w.title.as_deref(), Some("Handle menu"));
        assert!(w.active && !w.focused && !w.picture_in_picture);
        assert_eq!(w.display_id, Some(0));
    }

    #[test]
    fn window_kinds_follow_android_numbering() {
        let kinds: Vec<_> = (1..=7).map(WindowKind::from_android).collect();
        assert_eq!(
            kinds,
            vec![
                WindowKind::Application,
                WindowKind::InputMethod,
                WindowKind::System,
                WindowKind::AccessibilityOverlay,
                WindowKind::SplitScreenDivider,
                WindowKind::MagnificationOverlay,
                WindowKind::Other(7),
            ]
        );
    }

    #[test]
    fn a_window_without_an_id_or_type_is_malformed() {
        assert!(WindowInfo::from_json(&json!({ "type": 1, "bounds": [0, 0, 1, 1] })).is_err());
        assert!(WindowInfo::from_json(&json!({ "id": 1, "bounds": [0, 0, 1, 1] })).is_err());
        assert!(WindowInfo::list_from_json(&json!({})).is_err());
    }

    #[test]
    fn window_change_bits_decode_and_unknown_bits_drop() {
        assert_eq!(WindowChange::from_bits(0), vec![]);
        assert_eq!(
            WindowChange::from_bits(0x1 | 0x8 | 0x400 | 0x10_0000),
            vec![WindowChange::Added, WindowChange::Bounds, WindowChange::PictureInPicture]
        );
    }

    #[test]
    fn events_parse_by_type_and_other_types_are_ignored() {
        let changed = WindowEvent::from_json(&json!({
            "type": TYPE_WINDOWS_CHANGED, "changes": 0x2, "window_id": 40, "display_id": -1,
        }))
        .unwrap();
        assert_eq!(changed.kind, WindowEventKind::WindowsChanged(vec![WindowChange::Removed]));
        assert_eq!(changed.window_id, Some(40));
        assert_eq!(changed.display_id, None);

        let state = WindowEvent::from_json(&json!({
            "type": TYPE_WINDOW_STATE_CHANGED, "package": "com.android.systemui",
            "class_name": "android.app.AlertDialog", "window_id": -1,
        }))
        .unwrap();
        assert_eq!(state.kind, WindowEventKind::StateChanged);
        assert_eq!(state.package.as_deref(), Some("com.android.systemui"));
        assert_eq!(state.window_id, None);

        // TYPE_VIEW_CLICKED.
        assert_eq!(WindowEvent::from_json(&json!({ "type": 1 })), None);
        assert_eq!(WindowEvent::from_json(&json!({})), None);
    }

    #[test]
    fn window_listeners_hear_events_until_their_subscription_drops() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sub = {
            let seen = seen.clone();
            on_window_change(move |e| seen.borrow_mut().push(e.package))
        };
        let event = |p: &str| WindowEvent {
            kind: WindowEventKind::StateChanged,
            package: Some(p.into()),
            class_name: None,
            window_id: None,
            display_id: None,
        };
        dispatch(event("a"));
        drop(sub);
        dispatch(event("b"));
        assert_eq!(*seen.borrow(), vec![Some("a".to_string())]);
    }
}
