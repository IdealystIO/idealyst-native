//! Cross-platform **local notifications** — show a notification now or
//! after a delay, update or cancel it by id — plus a raw **push-token**
//! seam for app-owned remote delivery.
//!
//! One author API, every backend, identical observable behavior. You build
//! a [`Notification`], then [`notify`] it immediately or [`schedule`] it
//! after a [`Duration`]; [`cancel`] / [`cancel_all`] take it back. The
//! platforms diverge in *mechanism* (UNUserNotificationCenter,
//! NotificationManager, the `Notification` Web API), not in the surface you
//! call.
//!
//! # Authorization goes through `permissions`
//!
//! Notifications need a runtime grant. This crate does **not** re-implement
//! the OS prompt — [`authorize`] is a thin convenience over
//! `permissions::request(Permission::Notifications)`, the shared grant
//! substrate. [`notify`] / [`schedule`] return [`NotifyError::NotAuthorized`]
//! when the grant is missing rather than silently dropping the post.
//!
//! # Push is a *seam*, not a delivery service
//!
//! [`push_token`] hands you the platform's remote-push token (an APNs device
//! token / an FCM token / a web-push `PushSubscription` JSON). Sending the
//! actual remote message is your server's job — this SDK deliberately owns
//! only the raw capability of *obtaining the token*. On some platforms the
//! token requires host wiring the app must supply (the iOS
//! `registerForRemoteNotifications` app-delegate callback, an FCM project, a
//! web service worker); where that's true [`push_token`] returns
//! [`NotifyError::NotSupported`] until the host installs it. See the README's
//! *Per-platform mechanism* table.
//!
//! ```ignore
//! use notifications::{authorize, notify, Notification};
//!
//! # async fn demo() -> Result<(), notifications::NotifyError> {
//! if authorize().await.is_granted() {
//!     notify(Notification::new("Saved", "Your note is safe")).await?;
//! }
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod recipes;

use std::collections::HashMap;
use std::time::Duration;

pub use permissions::PermissionStatus;
use permissions::{request, Permission};

// ---------------------------------------------------------------------------
// Public types.
// ---------------------------------------------------------------------------

/// The stable identifier of a delivered or pending notification.
///
/// A notification carries a string id so it can be **updated** (post again
/// with the same id to replace it) or **cancelled** ([`cancel`]). Pass an
/// explicit [`Notification::id`]; if you don't, one is generated at post
/// time and returned from [`notify`] / [`schedule`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NotificationId(pub String);

impl NotificationId {
    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NotificationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for NotificationId {
    fn from(s: String) -> Self {
        NotificationId(s)
    }
}

impl From<&str> for NotificationId {
    fn from(s: &str) -> Self {
        NotificationId(s.to_string())
    }
}

/// A notification to deliver — the builder you pass to [`notify`] /
/// [`schedule`].
///
/// `title` and `body` are required (use [`Notification::new`]); `subtitle`,
/// a stable [`id`](Notification::id), and a free-form `data` map are
/// optional. The `data` map is opaque key/value app payload carried with
/// the notification (e.g. a deep-link route to open on tap); it's surfaced
/// to your tap handler by the platform where supported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notification {
    /// The bold primary line.
    pub title: String,
    /// The body text under the title.
    pub body: String,
    /// An optional secondary line shown between title and body where the
    /// platform supports it (iOS/macOS `subtitle`; folded into the body on
    /// platforms without a distinct field).
    pub subtitle: Option<String>,
    /// A stable id for update/cancel. `None` → a fresh id is generated at
    /// post time.
    pub id: Option<NotificationId>,
    /// Opaque app payload carried with the notification.
    pub data: HashMap<String, String>,
}

impl Notification {
    /// A notification with just a `title` and `body`. Chain the builder
    /// setters for the rest.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Notification {
            title: title.into(),
            body: body.into(),
            subtitle: None,
            id: None,
            data: HashMap::new(),
        }
    }

    /// Set the secondary line.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Set the stable id (so a later post with the same id replaces this
    /// one, and [`cancel`] can target it).
    pub fn id(mut self, id: impl Into<NotificationId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add a single key/value to the opaque payload.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    /// Replace the whole payload map.
    pub fn data(mut self, data: HashMap<String, String>) -> Self {
        self.data = data;
        self
    }
}

/// The platform's remote-push registration token, returned by
/// [`push_token`].
///
/// Its meaning is platform-specific — an APNs device token, an FCM
/// registration token, or a web-push `PushSubscription` serialized as JSON.
/// It's an opaque string you forward to *your* server, which performs the
/// actual remote delivery. This SDK never sends a remote push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushToken(pub String);

impl PushToken {
    /// The token as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a notification op failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyError {
    /// The notification permission isn't granted. Call [`authorize`] (or
    /// `permissions::request`) first and gate on the result.
    NotAuthorized,
    /// The underlying platform API failed (the message describes it).
    Backend(String),
    /// This operation isn't available on this platform — e.g. delay-based
    /// [`schedule`] on web (no native scheduling API), or [`push_token`]
    /// before the host has installed the remote-push wiring it needs.
    NotSupported,
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyError::NotAuthorized => write!(f, "notifications not authorized"),
            NotifyError::Backend(msg) => write!(f, "notification backend error: {msg}"),
            NotifyError::NotSupported => {
                write!(f, "notification operation not supported on this platform")
            }
        }
    }
}

impl std::error::Error for NotifyError {}

// ---------------------------------------------------------------------------
// Public API. Each fn delegates to the per-target `imp`; the surface is
// uniform, only the mechanism behind `imp` differs.
// ---------------------------------------------------------------------------

/// Request the notification permission, showing the OS prompt when it hasn't
/// been asked yet. A thin convenience over
/// `permissions::request(Permission::Notifications)`; gate posting on the
/// returned [`PermissionStatus`] (`status.is_granted()`).
pub async fn authorize() -> PermissionStatus {
    request(Permission::Notifications).await
}

/// Deliver `n` immediately as a local notification.
///
/// Returns the [`NotificationId`] it was posted under — the one from
/// [`Notification::id`] if set, else a freshly generated id. Posting again
/// with the same id replaces the existing notification.
pub async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    imp::notify(n).await
}

/// Deliver `n` after `after` has elapsed (a single, one-shot delay).
///
/// This is the deliberately-simple scheduling primitive: a delay, mapped to
/// the platform's interval/calendar trigger. Calendar repeats, time-of-day
/// rules, and richer recurrence are a later layer, not baked in here.
///
/// Web has no native delayed-notification API; `schedule` returns
/// [`NotifyError::NotSupported`] there (post from your service worker /
/// server instead).
pub async fn schedule(n: Notification, after: Duration) -> Result<NotificationId, NotifyError> {
    imp::schedule(n, after).await
}

/// Cancel the pending/delivered notification with `id`. A no-op if no such
/// notification exists.
pub async fn cancel(id: &NotificationId) {
    imp::cancel(id).await
}

/// Cancel every pending and delivered notification this app posted.
pub async fn cancel_all() {
    imp::cancel_all().await
}

/// The platform's remote-push token for *app-owned* delivery — an APNs
/// device token / FCM token / web-push `PushSubscription` JSON.
///
/// Forward it to your server, which sends the actual remote push. Returns
/// [`NotifyError::NotSupported`] on platforms where obtaining the token
/// needs host wiring the app hasn't installed (see the crate docs and
/// README *push seam* notes).
pub async fn push_token() -> Result<PushToken, NotifyError> {
    imp::push_token().await
}

// ---------------------------------------------------------------------------
// id helper — a stable, dependency-free unique id for the "no explicit id"
// case. Monotonic process counter + a coarse time component; collision-free
// within a process, which is all `notify`/`schedule` need (the platform
// keys notifications by this string).
// ---------------------------------------------------------------------------

pub(crate) fn generated_id() -> NotificationId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    NotificationId(format!("idealyst-{n}"))
}

/// The id a notification will be posted under: its explicit id, or a
/// freshly generated one. Shared by every backend so behavior is uniform.
pub(crate) fn resolve_id(n: &Notification) -> NotificationId {
    n.id.clone().unwrap_or_else(generated_id)
}

// ---------------------------------------------------------------------------
// Platform implementation. Exactly one `imp` compiles per target; every
// unsupported native target (desktop Windows / Linux, the host running
// `cargo test`) falls back to the stub below. The public surface above is
// uniform — only `imp` bodies differ.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
#[path = "android.rs"]
mod imp;

// Fallback for every target with no native local-notification model
// (desktop Windows / Linux, the test host). Authorization there reports
// `Unsupported` (usable), so we honor that: `notify` / `schedule` succeed
// as no-ops returning the resolved id, and `push_token` reports
// `NotSupported`. This keeps host unit tests of the shared id/builder logic
// runnable without a real notification center.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
mod imp {
    use super::{resolve_id, Notification, NotificationId, NotifyError, PushToken};
    use std::time::Duration;

    pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
        // No host notification center; report the id it would carry so
        // callers (and tests) see consistent behavior.
        Ok(resolve_id(&n))
    }

    pub(super) async fn schedule(
        n: Notification,
        _after: Duration,
    ) -> Result<NotificationId, NotifyError> {
        Ok(resolve_id(&n))
    }

    pub(super) async fn cancel(_id: &NotificationId) {}

    pub(super) async fn cancel_all() {}

    pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
        Err(NotifyError::NotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_sets_fields() {
        let n = Notification::new("Title", "Body")
            .subtitle("Sub")
            .id("note-1")
            .with("route", "/inbox");
        assert_eq!(n.title, "Title");
        assert_eq!(n.body, "Body");
        assert_eq!(n.subtitle.as_deref(), Some("Sub"));
        assert_eq!(n.id, Some(NotificationId("note-1".into())));
        assert_eq!(n.data.get("route").map(String::as_str), Some("/inbox"));
    }

    #[test]
    fn resolve_id_prefers_explicit() {
        let n = Notification::new("a", "b").id("stable");
        assert_eq!(resolve_id(&n), NotificationId("stable".into()));
    }

    #[test]
    fn resolve_id_generates_when_absent() {
        let n = Notification::new("a", "b");
        let a = resolve_id(&n);
        let b = resolve_id(&n);
        // Two unset notifications get distinct generated ids.
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("idealyst-"));
    }

    #[test]
    fn notification_id_conversions() {
        assert_eq!(NotificationId::from("x"), NotificationId("x".into()));
        assert_eq!(
            NotificationId::from(String::from("y")),
            NotificationId("y".into())
        );
        assert_eq!(NotificationId("z".into()).to_string(), "z");
    }

    #[test]
    fn error_display() {
        assert_eq!(
            NotifyError::NotAuthorized.to_string(),
            "notifications not authorized"
        );
        assert!(NotifyError::Backend("boom".into())
            .to_string()
            .contains("boom"));
    }

    /// On a host with the **fallback** `imp` (desktop Windows / Linux / a
    /// CI runner — *not* macOS, whose `apple` backend messages the real
    /// `UNUserNotificationCenter` and aborts in a non-bundled test binary)
    /// posts are no-op successes and there's no push token — exercising the
    /// shared id/builder path end-to-end through the public API.
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "android"
    )))]
    #[tokio::test]
    async fn host_notify_returns_resolved_id() {
        let id = notify(Notification::new("Hi", "there").id("greeting"))
            .await
            .unwrap();
        assert_eq!(id, NotificationId("greeting".into()));

        let gen = notify(Notification::new("Hi", "again")).await.unwrap();
        assert!(gen.as_str().starts_with("idealyst-"));

        // cancel / cancel_all are no-ops, must not panic.
        cancel(&id).await;
        cancel_all().await;

        assert_eq!(push_token().await, Err(NotifyError::NotSupported));
    }

    /// `authorize()` resolves on a fallback host (no native permission model
    /// → `Unsupported`, which `is_usable()` accepts). Skipped on macOS,
    /// where it would message a real notification center the unbundled test
    /// binary can't open.
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "android"
    )))]
    #[tokio::test]
    async fn host_authorize_is_usable() {
        assert!(authorize().await.is_usable());
    }
}
