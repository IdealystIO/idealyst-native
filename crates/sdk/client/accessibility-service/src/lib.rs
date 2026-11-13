//! **The app's own Android accessibility service** — see and operate the
//! whole screen with nobody at the device.
//!
//! An app that depends on this crate ships an `AccessibilityService`. Once
//! someone switches it on in Settings > Accessibility, the app can take
//! screenshots of whatever is on screen (another app included), and tap,
//! long-press, swipe, press system keys and type into the focused field
//! anywhere on it. It can also find an element on screen by its id, text,
//! package or class and press it, list the windows on screen, and hear when
//! they change. It is built for remote support of unattended devices: an
//! MDM agent streaming a kiosk tablet's screen to an admin console and
//! playing the admin's clicks back as taps, and pressing a system prompt's
//! button when nobody is at the tablet to.
//!
//! ```ignore
//! use std::time::Duration;
//! use accessibility_service::{GlobalAction, ImageFormat, Key, Query};
//!
//! if !accessibility_service::is_enabled() {
//!     accessibility_service::open_settings()?; // one-time switch-on
//! }
//! let shot = accessibility_service::screenshot(ImageFormat::Jpeg { quality: 70 }).await?;
//! send_frame(shot.width, shot.height, &shot.data);
//!
//! // A click on that frame, in its pixels:
//! accessibility_service::tap(812.0, 440.0).await?;
//! accessibility_service::swipe((1200.0, 900.0), (1200.0, 300.0), 300).await?;
//! accessibility_service::global_action(GlobalAction::Back)?;
//!
//! // Type into whatever field has focus, as a keyboard would:
//! accessibility_service::type_text("Main gate")?;
//! accessibility_service::press_key(Key::Enter)?;
//!
//! // Press a button by what it is, in any app or system dialog:
//! let q = Query::new().package("com.android.systemui").view_id("android:id/button1");
//! let found = accessibility_service::wait_for(&q, Duration::from_secs(5)).await?;
//! accessibility_service::click(&found[0]).await?;
//! ```
//!
//! # Coordinates
//!
//! A screenshot is the default display as it is shown, in its current
//! rotation, and gestures take points in that same space: a point on the
//! screenshot is the point to tap. [`Screenshot::rotation`] says how the
//! display is turned from its natural orientation, for a viewer that wants
//! to know. [`Node::bounds`] and [`WindowInfo::bounds`] are in the same
//! space.
//!
//! # Switching the service on
//!
//! Android lets only a person (in Settings) or `adb` switch an
//! accessibility service on; an app cannot, not even a device owner. Once
//! per device:
//!
//! ```text
//! adb shell settings put secure enabled_accessibility_services \
//!     <applicationId>/io.idealyst.accessibilityservice.RustAccessibilityService
//! ```
//!
//! On Android 13+, Settings greys the switch out for an app installed
//! outside an app store until someone allows "restricted settings" in the
//! app's info page; the `adb` route is not affected. A device owner can
//! limit which services may be switched on at all with `device-admin`'s
//! `set_permitted_accessibility_services`.
//!
//! # Errors
//!
//! Every call returns [`Error::NotEnabled`] until the service is switched
//! on, and [`Error::Unsupported`] off Android or below the API level a call
//! needs (named per function). [`screenshot`] says when to retry rather
//! than failing silently ([`Error::TooSoon`]), and on API 34+ refuses a
//! frame whose active window is secure with [`Error::SecureWindow`].
//! [`click`] on an element that has left the screen is
//! [`Error::StaleNode`], and [`wait_for`] that runs out of time is
//! [`Error::Timeout`].
//!
//! # Manifest
//!
//! The crate contributes the `<service>`, its configuration
//! (`res/xml/accessibility_service.xml`) and two strings to every app that
//! depends on it (`[package.metadata.idealyst.android]` in its
//! `Cargo.toml`). The service is inert until switched on. Settings shows
//! `accessibility_service_label` and `accessibility_service_description`;
//! an app replaces them:
//!
//! ```toml
//! [package.metadata.idealyst.app.android.strings]
//! accessibility_service_label = "Example remote support"
//! accessibility_service_description = "Lets your administrator see and operate this tablet remotely."
//! ```

#![deny(missing_docs)]

use std::future::Future;
use std::pin::Pin;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
mod android;
mod enabled;
mod find;
mod listeners;
mod policy;
mod reply;
mod windows;

pub use enabled::{on_enabled_change, EnabledSubscription};
pub use find::{click, find, wait_for, Bounds, Node, Query, TextMatch, NODE_HANDLES, WAIT_POLL};
pub use windows::{
    on_window_change, windows, WindowChange, WindowEvent, WindowEventKind, WindowInfo, WindowKind,
    WindowSubscription,
};

/// Compile-checked usage recipes (docs / MCP catalog). Present only under the
/// `catalog` feature.
#[cfg(feature = "catalog")]
pub mod recipes;

/// Why an accessibility-service call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The service is not switched on (or not bound yet).
    #[error("the accessibility service is not switched on")]
    NotEnabled,
    /// [`screenshot`]: Android allows about three screenshots a second.
    /// Retry after `retry_after_ms`.
    #[error("screenshot requested too soon; retry in {retry_after_ms} ms")]
    TooSoon {
        /// How long to wait before the next request.
        retry_after_ms: u32,
    },
    /// [`screenshot`]: the active window forbids capture (`FLAG_SECURE`).
    /// API 34+; see [`screenshot`].
    #[error("a secure window is on screen and cannot be captured")]
    SecureWindow,
    /// A gesture was cancelled before it finished — typically by a touch on
    /// the device, or another gesture dispatched over it.
    #[error("the gesture was cancelled")]
    GestureCancelled,
    /// Android declined the action: a global action it did not accept, a
    /// field that is not editable, a gesture it would not dispatch.
    #[error("refused: {0}")]
    Refused(String),
    /// [`set_focused_text`]: no input field has focus.
    #[error("no input field has focus")]
    NoFocusedInput,
    /// [`click`]: the element is no longer on screen, or was found too long
    /// ago ([`NODE_HANDLES`]). Find it again.
    #[error("the element is no longer on screen; find it again")]
    StaleNode,
    /// [`wait_for`]: nothing matched before the timeout.
    #[error("nothing matched before the timeout")]
    Timeout,
    /// An argument the platform cannot take: a point off the screen, a
    /// duration outside 1 ms – 60 s, a JPEG quality above 100.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Not available on this platform, or below the API level the call
    /// needs.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The platform failed the call; the message is its own.
    #[error("{0}")]
    Platform(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// A boxed future resolving to a [`Result`].
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = Result<T>>>>;

/// How the display is turned from its natural orientation
/// (`Display.getRotation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotation {
    /// The natural orientation.
    Deg0,
    /// Turned 90°.
    Deg90,
    /// Upside down.
    Deg180,
    /// Turned 270°.
    Deg270,
}

/// The encoding [`screenshot`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageFormat {
    /// Raw pixels, 4 bytes each (R, G, B, A), rows top to bottom with no
    /// padding: `width × height × 4` bytes.
    Rgba,
    /// JPEG at `quality` 0–100, encoded on a background thread on the
    /// device. About a tenth of the raw size at 70 for a typical UI.
    Jpeg {
        /// 0–100, as `Bitmap.compress` takes it.
        quality: u8,
    },
}

/// One screenshot of the default display.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Screenshot {
    /// Width in pixels, in the display's current rotation.
    pub width: u32,
    /// Height in pixels, in the display's current rotation.
    pub height: u32,
    /// How the display was turned when the screenshot was taken.
    pub rotation: Rotation,
    /// The encoding of [`data`](Self::data): what was asked for.
    pub format: ImageFormat,
    /// The image.
    pub data: Vec<u8>,
}

/// A system key or action ([`global_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GlobalAction {
    /// Back.
    Back,
    /// Home. In lock task mode Android blocks it without telling the
    /// caller: the call succeeds and the locked app stays in front.
    Home,
    /// The recent-apps overview.
    Recents,
    /// Pull down the notification shade.
    Notifications,
    /// Pull down quick settings.
    QuickSettings,
    /// Lock the screen. API 28+.
    LockScreen,
}

impl GlobalAction {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    fn wire_name(self) -> &'static str {
        match self {
            GlobalAction::Back => "back",
            GlobalAction::Home => "home",
            GlobalAction::Recents => "recents",
            GlobalAction::Notifications => "notifications",
            GlobalAction::QuickSettings => "quick_settings",
            GlobalAction::LockScreen => "lock_screen",
        }
    }
}

/// The service is switched on and bound right now. `false` off Android.
pub fn is_enabled() -> bool {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        match android::call("is_enabled", &serde_json::json!({})) {
            Ok(v) => v.as_bool().unwrap_or(false),
            Err(e) => {
                log::warn!("accessibility_service: is_enabled failed: {e}");
                false
            }
        }
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    false
}

/// Open Settings > Accessibility, where a person switches the service on.
pub fn open_settings() -> Result<()> {
    sync_call("open_settings", serde_json::json!({})).map(drop)
}

/// A screenshot of the default display, in its current rotation. API 30+.
///
/// Android allows one request about every third of a second; a request
/// sooner than that fails with [`Error::TooSoon`] saying how long to wait,
/// without reaching Android. The pixels are converted (and JPEG-encoded)
/// on a background thread, never the UI thread.
///
/// A window that forbids capture (`FLAG_SECURE`: a banking app, a browser's
/// incognito tab) comes out black in a whole-screen capture, and Android
/// reports success. On API 34+ the SDK also asks Android about the active
/// window and fails with [`Error::SecureWindow`] instead of returning that
/// black frame. The question is answered by the front app's own UI thread,
/// so the SDK waits at most half a second for it after the capture is in;
/// a frame taken while a busy app is still closing a secure window can go
/// out unchecked. Below API 34 a secure window always arrives black.
pub fn screenshot(format: ImageFormat) -> BoxFuture<Screenshot> {
    if let ImageFormat::Jpeg { quality } = format {
        if let Err(e) = policy::check_quality(quality) {
            return ready(Err(e));
        }
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        android::screenshot(format)
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    ready(Err(unsupported("screenshot")))
}

/// Tap at `(x, y)`, in screenshot pixels. Resolves when Android reports the
/// gesture done, so the next one can follow in order.
pub fn tap(x: f32, y: f32) -> BoxFuture<()> {
    gesture((x, y), (x, y), policy::TAP_MS)
}

/// Press at `(x, y)` for `duration_ms` (1 ms – 60 s; a long press needs
/// more than the device's long-press timeout, usually 400–500 ms).
pub fn long_press(x: f32, y: f32, duration_ms: u32) -> BoxFuture<()> {
    gesture((x, y), (x, y), duration_ms)
}

/// Drag from `from` to `to` in a straight line over `duration_ms`
/// (1 ms – 60 s). A short duration flings; a long one drags.
pub fn swipe(from: (f32, f32), to: (f32, f32), duration_ms: u32) -> BoxFuture<()> {
    gesture(from, to, duration_ms)
}

fn gesture(from: (f32, f32), to: (f32, f32), duration_ms: u32) -> BoxFuture<()> {
    let checked = policy::check_point(from.0, from.1)
        .and_then(|()| policy::check_point(to.0, to.1))
        .and_then(|()| policy::check_duration(duration_ms));
    if let Err(e) = checked {
        return ready(Err(e));
    }
    let args = serde_json::json!({
        "x1": from.0, "y1": from.1, "x2": to.0, "y2": to.1, "duration_ms": duration_ms,
    });
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        let fut = android::call_async("gesture", &args);
        Box::pin(async move { fut.await.map(drop) })
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    {
        let _ = args;
        ready(Err(unsupported("gesture")))
    }
}

/// Perform a system key or action. [`Error::Refused`] when Android does not
/// accept it. Acceptance is not effect: lock task mode blocks Home
/// silently, and the call still succeeds.
pub fn global_action(action: GlobalAction) -> Result<()> {
    sync_call("global_action", serde_json::json!({ "action": action.wire_name() })).map(drop)
}

/// Replace the text of the input-focused field with `text`. To type
/// into the field as a keyboard does, at its cursor, use [`type_text`].
/// [`Error::NoFocusedInput`] when no field has focus.
pub fn set_focused_text(text: &str) -> Result<()> {
    sync_call("set_focused_text", serde_json::json!({ "text": text })).map(drop)
}

/// A key [`press_key`] presses in the focused field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    /// Delete the character before the cursor, or the selection.
    Backspace,
    /// Delete the character after the cursor, or the selection.
    Delete,
    /// The field's own action when it has one (Search, Go, Next, Done),
    /// else a new line.
    Enter,
    /// Move focus to the next field.
    Tab,
    /// Move the cursor one character left.
    Left,
    /// Move the cursor one character right.
    Right,
    /// Move the cursor up a line.
    Up,
    /// Move the cursor down a line.
    Down,
    /// The start of the line.
    Home,
    /// The end of the line.
    End,
    /// Escape: closes what the field opened (a suggestion list).
    Escape,
}

impl Key {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    fn wire_name(self) -> &'static str {
        match self {
            Key::Backspace => "backspace",
            Key::Delete => "delete",
            Key::Enter => "enter",
            Key::Tab => "tab",
            Key::Left => "left",
            Key::Right => "right",
            Key::Up => "up",
            Key::Down => "down",
            Key::Home => "home",
            Key::End => "end",
            Key::Escape => "escape",
        }
    }
}

/// Type `text` into the focused field at its cursor, replacing the
/// selection, as a keyboard does. Works in password fields, whose text the
/// service cannot read. API 33+ ([`Error::Unsupported`] below it);
/// [`Error::NoFocusedInput`] when no field has focus.
pub fn type_text(text: &str) -> Result<()> {
    sync_call("type_text", serde_json::json!({ "text": text })).map(drop)
}

/// Press `key` in the focused field. API 33+ ([`Error::Unsupported`]
/// below it); [`Error::NoFocusedInput`] when no field has focus.
pub fn press_key(key: Key) -> Result<()> {
    sync_call("press_key", serde_json::json!({ "key": key.wire_name() })).map(drop)
}

/// Tell the platform half to start or stop reporting `what` (`"enabled"`,
/// `"windows"`): called when a listener list goes from or to empty.
fn platform_listening(what: &str, on: bool) {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        // A failure here (no JVM context yet) means no change is reported;
        // the next first registration retries.
        let args = serde_json::json!({ "what": what, "on": on });
        if let Err(e) = android::call("listen", &args) {
            log::warn!("accessibility_service: listen {what} {on} failed: {e}");
        }
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    let _ = (what, on);
}

fn sync_call(method: &str, args: serde_json::Value) -> Result<serde_json::Value> {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        android::call(method, &args)
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    {
        let _ = args;
        Err(unsupported(method))
    }
}

#[cfg_attr(target_os = "android", allow(dead_code))]
fn unsupported(method: &str) -> Error {
    Error::Unsupported(format!(
        "accessibility_service::{method} is Android only"
    ))
}

fn ready<T: 'static>(r: Result<T>) -> BoxFuture<T> {
    Box::pin(std::future::ready(r))
}
