//! Cross-platform **local notifications** and their **responses** — show a
//! notification now or after a delay, with action buttons and a reply
//! field, update or cancel it by id, and hear what the user did with it —
//! plus a raw **push-token** seam for app-owned remote delivery.
//!
//! One author API, every backend, identical observable behavior. You build
//! a [`Notification`], then [`notify`] it immediately or [`schedule`] it
//! after a [`Duration`]; [`cancel`] / [`cancel_all`] take it back;
//! [`on_response`] hears the tap, the action button, the reply or the
//! dismissal. The platforms diverge in *mechanism*
//! (UNUserNotificationCenter, NotificationManager, WinRT toasts, the
//! freedesktop D-Bus service, the `Notification` Web API), not in the
//! surface you call.
//!
//! # What is shared, what is per-platform
//!
//! The shared fields are what every platform can show: title, body,
//! subtitle, a payload, [`Action`]s (buttons, optionally with a text
//! field), a [`Priority`], a [`Sound`], a group and an image. Where a
//! platform cannot honor one, the README's table says what happens — it is
//! dropped, never faked. Everything else a platform can do is reached
//! through that platform's hook ([`Notification::apple`],
//! [`Notification::android`], [`Notification::windows`],
//! [`Notification::web`], [`Notification::linux`]), which hands you the
//! platform's own notification object before it is posted.
//!
//! # Responses
//!
//! Every response is a [`NotificationResponse`]: which notification, what
//! the user did ([`ResponseAction`]), the reply text, and the payload. A
//! response to a notification is delivered once, on the UI thread, to every
//! [`on_response`] handler. Call [`init`] (or [`on_response`]) at boot:
//! the tap that launched the app is held until the first handler exists.
//!
//! # Authorization goes through `permissions`
//!
//! Notifications need a runtime grant. This crate does **not** re-implement
//! the OS prompt — [`authorize`] is a thin convenience over
//! `permissions::request(Permission::Notifications)`, the shared grant
//! substrate.
//!
//! # Push is a *seam*, not a delivery service
//!
//! [`push_token`] hands you the platform's remote-push token (an APNs device
//! token / an FCM token / a web-push `PushSubscription` JSON). Sending the
//! actual remote message is your server's job. Where obtaining the token
//! needs host wiring that is not installed, [`push_token`] returns
//! [`NotifyError::NotSupported`]. See the README's *Per-platform mechanism*
//! table.
//!
//! ```ignore
//! use notifications::{authorize, notify, on_response, Action, Notification, ResponseAction};
//!
//! // At boot: hear taps and button presses, including the launch tap.
//! let _sub = on_response(|r| match &r.action {
//!     ResponseAction::Open => log::info!("opened {}", r.id),
//!     ResponseAction::Action(id) if id == "reply" => log::info!("reply: {:?}", r.text),
//!     _ => {}
//! });
//!
//! # async fn demo() -> Result<(), notifications::NotifyError> {
//! if authorize().await.is_granted() {
//!     notify(
//!         Notification::new("Sam", "Lunch?")
//!             .action(Action::new("reply", "Reply").text_input("Send", "Message"))
//!             .action(Action::new("mute", "Mute").destructive()),
//!     )
//!     .await?;
//! }
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod recipes;

mod hooks;
mod response;

use std::collections::HashMap;
use std::sync::{Mutex, Once};
use std::time::Duration;

pub use permissions::PermissionStatus;
use permissions::{request, Permission};
pub use response::{on_response, ResponseSubscription};

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

/// How insistently a notification interrupts.
///
/// Each platform maps it onto its own scale (README *Shared fields*):
/// Android notification channels (one per level), iOS/macOS interruption
/// levels, a Windows toast's popup and priority, the freedesktop urgency
/// hint, and the web's `requireInteraction`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Delivered quietly to the notification list, without a banner where
    /// the platform can do that.
    Low,
    /// The platform's normal notification.
    #[default]
    Default,
    /// Shown as a banner / heads-up even where the platform's normal level
    /// would not.
    High,
    /// Time-sensitive: breaks through where the platform allows it and
    /// stays on screen until acted on.
    Urgent,
}

/// Whether the notification plays the platform's alert sound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Sound {
    /// The platform's default notification sound.
    #[default]
    Default,
    /// No sound.
    Silent,
}

/// A text field attached to an [`Action`]: pressing the action asks the
/// user for a line of text (a reply), which arrives as
/// [`NotificationResponse::text`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextInput {
    /// The send button's label.
    pub button_title: String,
    /// Placeholder text shown in the empty field.
    pub placeholder: String,
}

/// A button on a notification. When the user presses it, every
/// [`on_response`] handler receives [`ResponseAction::Action`] with this
/// action's `id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Action {
    /// The id reported back in [`ResponseAction::Action`].
    pub id: String,
    /// The button label.
    pub title: String,
    /// A text field the action asks for before it is reported.
    pub input: Option<TextInput>,
    /// Bring the app to the foreground when pressed (default `true`).
    /// `false` handles the action without opening the app where the
    /// platform supports that (README *Responses*).
    pub opens_app: bool,
    /// Styled as destructive (red) where the platform has that style.
    pub destructive: bool,
    /// Requires the device to be unlocked before the action runs (iOS).
    pub requires_unlock: bool,
}

impl Action {
    /// A button with `id` (reported back) and `title` (shown).
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Action {
            id: id.into(),
            title: title.into(),
            input: None,
            opens_app: true,
            destructive: false,
            requires_unlock: false,
        }
    }

    /// Ask for a line of text when pressed; the text arrives as
    /// [`NotificationResponse::text`].
    pub fn text_input(mut self, button_title: impl Into<String>, placeholder: impl Into<String>) -> Self {
        self.input = Some(TextInput { button_title: button_title.into(), placeholder: placeholder.into() });
        self
    }

    /// Whether pressing the action brings the app to the foreground.
    pub fn opens_app(mut self, opens_app: bool) -> Self {
        self.opens_app = opens_app;
        self
    }

    /// Style the button as destructive.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// Require an unlocked device before the action runs.
    pub fn requires_unlock(mut self) -> Self {
        self.requires_unlock = true;
        self
    }
}

/// A notification to deliver — the builder you pass to [`notify`] /
/// [`schedule`].
///
/// `title` and `body` are required (use [`Notification::new`]); everything
/// else is optional. The `data` map is opaque key/value app payload carried
/// with the notification and handed back in every
/// [`NotificationResponse`] (e.g. the route to open on tap). Keys starting
/// with `idealyst.` are reserved for the SDK.
///
/// Equality compares every field; platform hooks compare by identity (the
/// same closure).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Notification {
    /// The bold primary line.
    pub title: String,
    /// The body text under the title.
    pub body: String,
    /// An optional secondary line shown between title and body where the
    /// platform supports it (iOS/macOS `subtitle`, Android sub-text; folded
    /// into the body on platforms without a distinct field).
    pub subtitle: Option<String>,
    /// A stable id for update/cancel. `None` → a fresh id is generated at
    /// post time.
    pub id: Option<NotificationId>,
    /// Opaque app payload carried with the notification.
    pub data: HashMap<String, String>,
    /// Buttons, in display order. Platforms show a limited number (iOS 4,
    /// Android 3, Windows 5); the rest are dropped.
    pub actions: Vec<Action>,
    /// How insistently it interrupts.
    pub priority: Priority,
    /// Whether it plays a sound.
    pub sound: Sound,
    /// Notifications with the same group are stacked together (iOS/macOS
    /// thread, Android group).
    pub group: Option<String>,
    /// An image shown with the notification: a local file path on native
    /// targets, a URL on web.
    pub image: Option<String>,
    /// Update in place without alerting again when a notification with this
    /// id is still shown ([`Notification::alert_once`]).
    pub alert_once: bool,
    pub(crate) hooks: hooks::PlatformHooks,
}

impl Notification {
    /// A notification with just a `title` and `body`. Chain the builder
    /// setters for the rest.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Notification { title: title.into(), body: body.into(), ..Default::default() }
    }

    /// Set the secondary line.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Set the stable id (so a later post with the same id replaces this
    /// one, and [`cancel`] can target it). A replacement alerts again — a
    /// banner and sound, as a new notification would — unless it is
    /// [`alert_once`](Notification::alert_once).
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

    /// Add a button.
    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    /// Set how insistently it interrupts.
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Set whether it plays a sound.
    pub fn sound(mut self, sound: Sound) -> Self {
        self.sound = sound;
        self
    }

    /// Stack it with the other notifications in `group`.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Show an image: a local file path on native targets, a URL on web.
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self
    }

    /// When a notification with this id is still shown (on screen or in the
    /// notification center), replace it quietly: new content, no banner, no
    /// sound. With none shown it alerts as usual. For something that
    /// updates often — progress, a running count, a live score.
    ///
    /// Decided when [`notify`] posts; a [`schedule`]d notification always
    /// alerts. Per-platform mechanism in the README's *Shared fields*.
    pub fn alert_once(mut self) -> Self {
        self.alert_once = true;
        self
    }

    /// iOS / macOS / tvOS: change the `UNMutableNotificationContent` after
    /// the shared fields are set and before it is posted — e.g.
    /// `setRelevanceScore:`, `setTargetContentIdentifier:`,
    /// `setSummaryArgument:`, `setAttachments:`. Called on the thread that
    /// posts.
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
    pub fn apple(mut self, hook: impl Fn(&objc2::runtime::AnyObject) + 'static) -> Self {
        self.hooks.apple = Some(std::rc::Rc::new(hook));
        self
    }

    /// Android: change the `android.app.Notification.Builder` after the
    /// shared fields are set and before `build()` — e.g. `setProgress`,
    /// `setStyle`, `setOngoing`, `setColor`, `setChannelId` for a channel
    /// the app created. Called on the UI thread.
    #[cfg(target_os = "android")]
    pub fn android(
        mut self,
        hook: impl for<'a> Fn(&mut jni::JNIEnv<'a>, &jni::objects::JObject<'a>) -> jni::errors::Result<()> + 'static,
    ) -> Self {
        self.hooks.android = Some(std::rc::Rc::new(hook));
        self
    }

    /// Windows: change the toast's XML document after the shared fields are
    /// written and before the toast is created — e.g. a `<progress>`
    /// element, a hero image placement, `<audio src>`, `scenario`.
    #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
    pub fn windows(
        mut self,
        hook: impl Fn(&windows::Data::Xml::Dom::XmlDocument) -> windows::core::Result<()> + 'static,
    ) -> Self {
        self.hooks.windows = Some(std::rc::Rc::new(hook));
        self
    }

    /// Web: change the `NotificationOptions` object passed to
    /// `showNotification` / `new Notification` — e.g. `vibrate`, `icon`,
    /// `badge`, `dir`, `lang`. The `tag` is the SDK's: it is picked when
    /// posting, after this hook, to replace by id (see
    /// [`alert_once`](Notification::alert_once)).
    #[cfg(target_arch = "wasm32")]
    pub fn web(
        mut self,
        hook: impl Fn(&mut runtime_core::__serde_json::Map<String, runtime_core::__serde_json::Value>) + 'static,
    ) -> Self {
        self.hooks.web = Some(std::rc::Rc::new(hook));
        self
    }

    /// Linux: change the freedesktop `hints` map passed to `Notify` — e.g.
    /// `category`, `desktop-entry`, `transient`, `resident`, or a daemon's
    /// own `x-…` hints.
    #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
    pub fn linux(
        mut self,
        hook: impl Fn(&mut HashMap<String, zbus::zvariant::Value<'static>>) + 'static,
    ) -> Self {
        self.hooks.linux = Some(std::rc::Rc::new(hook));
        self
    }
}

/// What the user did with a notification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResponseAction {
    /// Tapped / clicked the notification itself.
    Open,
    /// Dismissed it (swiped away, closed). Reported where the platform
    /// tells the app (README *Responses*).
    Dismiss,
    /// Pressed the [`Action`] with this id.
    Action(String),
}

/// One response to a notification, handed to every [`on_response`]
/// handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationResponse {
    /// The notification responded to.
    pub id: NotificationId,
    /// What the user did.
    pub action: ResponseAction,
    /// The text typed into an action's [`TextInput`], if it had one.
    pub text: Option<String>,
    /// The notification's payload ([`Notification::data`]).
    pub data: HashMap<String, String>,
}

/// The words this SDK shows that the app may replace: the names of the
/// notification categories the user sees in system settings (Android's
/// per-app notification channels: one per [`Priority`], and one for
/// [`Sound::Silent`] notifications). English until the app installs its
/// own with [`set_labels`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    /// The [`Priority::Low`] category.
    pub low: String,
    /// The [`Priority::Default`] category.
    pub default: String,
    /// The [`Priority::High`] category.
    pub high: String,
    /// The [`Priority::Urgent`] category.
    pub urgent: String,
    /// The category of [`Sound::Silent`] notifications.
    pub silent: String,
}

impl Default for Labels {
    fn default() -> Self {
        Labels {
            low: "Quiet notifications".into(),
            default: "Notifications".into(),
            high: "Important notifications".into(),
            urgent: "Urgent notifications".into(),
            silent: "Silent notifications".into(),
        }
    }
}

impl Labels {
    /// The label for `priority`'s category.
    pub fn for_priority(&self, priority: Priority) -> &str {
        match priority {
            Priority::Low => &self.low,
            Priority::Default => &self.default,
            Priority::High => &self.high,
            Priority::Urgent => &self.urgent,
        }
    }
}

static LABELS: Mutex<Option<Labels>> = Mutex::new(None);

/// Replace the words this SDK shows (see [`Labels`]). Takes effect for the
/// next post; Android renames existing channels then.
pub fn set_labels(labels: Labels) {
    *LABELS.lock().unwrap_or_else(|e| e.into_inner()) = Some(labels);
}

/// The installed [`Labels`] (English until [`set_labels`]).
pub fn labels() -> Labels {
    LABELS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
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

/// Start listening for responses. Call once at boot, on the UI thread,
/// before the first screen mounts; later calls do nothing. [`on_response`],
/// [`notify`] and [`schedule`] call it themselves.
///
/// It installs the platform listener — the UNUserNotificationCenter
/// delegate on iOS/macOS (which must exist before launch finishes, or the
/// tap that launched the app is never delivered), the Android bridge that
/// collects taps recorded before the app booted, the Linux D-Bus signal
/// watcher — and the UI-thread pump that runs the handlers.
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        response::start_pump();
        imp::init();
    });
}

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
    init();
    runtime_vocabulary::robot_only! {
        let posted = n.clone();
    }
    let outcome = imp::notify(n).await;
    runtime_vocabulary::robot_only! {
        robot_controls::record_post(&posted, None, &outcome);
    }
    outcome
}

/// Deliver `n` after `after` has elapsed (a single, one-shot delay).
///
/// This is the deliberately-simple scheduling primitive: a delay, mapped to
/// the platform's interval trigger. Calendar repeats, time-of-day rules,
/// and richer recurrence are not part of it.
///
/// Web and Linux have no OS scheduler for notifications; `schedule`
/// returns [`NotifyError::NotSupported`] there.
pub async fn schedule(n: Notification, after: Duration) -> Result<NotificationId, NotifyError> {
    init();
    runtime_vocabulary::robot_only! {
        let posted = n.clone();
    }
    let outcome = imp::schedule(n, after).await;
    runtime_vocabulary::robot_only! {
        robot_controls::record_post(&posted, Some(after.as_millis()), &outcome);
    }
    outcome
}

/// Cancel the pending/delivered notification with `id`. A no-op if no such
/// notification exists.
pub async fn cancel(id: &NotificationId) {
    runtime_vocabulary::robot_only! {
        robot_controls::record_cancel(Some(id));
    }
    imp::cancel(id).await
}

/// Cancel every pending and delivered notification this app posted.
pub async fn cancel_all() {
    runtime_vocabulary::robot_only! {
        robot_controls::record_cancel(None);
    }
    imp::cancel_all().await
}

/// Register this SDK's robot test controls, so an E2E test can assert on
/// what the app posted without reading the device's notification shade,
/// and act as the user responding to one: `notifications.log` returns every
/// [`notify`] / [`schedule`] (with its outcome), [`cancel`] /
/// [`cancel_all`] and handled response since launch,
/// `notifications.clear_log` empties it, and `notifications.respond`
/// delivers a [`NotificationResponse`] to the [`on_response`] handlers as
/// if the user had tapped. Call once at boot, on the UI thread; a no-op
/// outside robot (dev) builds.
pub fn register_robot_controls() {
    runtime_vocabulary::robot_only! {
        robot_controls::register();
    }
}

// Robot test controls (`notifications.log` / `notifications.clear_log` /
// `notifications.respond`): only in robot builds.
runtime_vocabulary::robot_only! {
    mod robot_controls;
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
// Helpers shared by the backends.
// ---------------------------------------------------------------------------

/// A process-unique id for the "no explicit id" case: a monotonic counter,
/// collision-free within a process, which is all `notify`/`schedule` need
/// (the platform keys notifications by this string).
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

/// The body with the subtitle folded on as its first line — for platforms
/// whose notification has no subtitle field (web, Linux).
#[allow(dead_code)]
pub(crate) fn body_with_subtitle(n: &Notification) -> String {
    match &n.subtitle {
        Some(sub) if !sub.is_empty() => format!("{sub}\n{}", n.body),
        _ => n.body.clone(),
    }
}

/// The most buttons any backend shows; a notification's further actions
/// are dropped by the backend that cannot show them.
#[allow(dead_code)]
pub(crate) fn visible_actions(n: &Notification, max: usize) -> &[Action] {
    &n.actions[..n.actions.len().min(max)]
}

/// Hand a platform response to the [`on_response`] handlers (from any
/// thread). The payload's reserved `idealyst.` keys are stripped.
#[allow(dead_code)]
pub(crate) fn deliver_response(
    id: NotificationId,
    action: ResponseAction,
    text: Option<String>,
    mut data: HashMap<String, String>,
) {
    data.retain(|k, _| !k.starts_with(RESERVED_PREFIX));
    response::deliver(NotificationResponse { id, action, text, data });
}

/// Payload keys the SDK uses for its own bookkeeping on platforms that
/// carry the response context in the payload (Windows launch arguments).
#[allow(dead_code)]
pub(crate) const RESERVED_PREFIX: &str = "idealyst.";

// ---------------------------------------------------------------------------
// Platform implementation. Exactly one `imp` compiles per target; Linux uses
// the D-Bus backend, Windows WinRT toasts, and every remaining native target
// with no local-notification model (other desktops, the host running
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

// Linux desktop: real local notifications via the session-bus
// `org.freedesktop.Notifications` service (zbus).
#[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
#[path = "linux.rs"]
mod imp;

// Windows: WinRT toasts (`Windows.UI.Notifications`), with OS-scheduled
// delayed delivery.
#[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
#[path = "windows.rs"]
mod imp;

// Fallback for every remaining target with no native local-notification model
// (other desktops, the test host). Authorization there reports `Unsupported`
// (usable), so we honor that: `notify` / `schedule` succeed as no-ops
// returning the resolved id, nothing ever responds, and `push_token` reports
// `NotSupported`. This keeps host unit tests of the shared logic runnable
// without a real notification center.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android"),
    not(any(target_os = "linux", target_os = "windows"))
))]
mod imp {
    use super::{resolve_id, Notification, NotificationId, NotifyError, PushToken};
    use std::time::Duration;

    pub(super) fn init() {}

    pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
        // No host notification center; report the id it would carry so
        // callers (and tests) see consistent behavior.
        Ok(resolve_id(&n))
    }

    pub(super) async fn schedule(n: Notification, _after: Duration) -> Result<NotificationId, NotifyError> {
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
            .with("route", "/inbox")
            .action(Action::new("reply", "Reply").text_input("Send", "Message"))
            .action(Action::new("mute", "Mute").destructive().opens_app(false))
            .priority(Priority::High)
            .sound(Sound::Silent)
            .group("chat-7")
            .image("/tmp/a.png");
        assert_eq!(n.title, "Title");
        assert_eq!(n.body, "Body");
        assert_eq!(n.subtitle.as_deref(), Some("Sub"));
        assert_eq!(n.id, Some(NotificationId("note-1".into())));
        assert_eq!(n.data.get("route").map(String::as_str), Some("/inbox"));
        assert_eq!(n.actions.len(), 2);
        assert_eq!(
            n.actions[0].input,
            Some(TextInput { button_title: "Send".into(), placeholder: "Message".into() })
        );
        assert!(n.actions[0].opens_app, "actions open the app unless told not to");
        assert!(n.actions[1].destructive && !n.actions[1].opens_app);
        assert_eq!(n.priority, Priority::High);
        assert_eq!(n.sound, Sound::Silent);
        assert_eq!(n.group.as_deref(), Some("chat-7"));
        assert_eq!(n.image.as_deref(), Some("/tmp/a.png"));
    }

    #[test]
    fn defaults_are_the_platform_normal() {
        let n = Notification::new("a", "b");
        assert_eq!(n.priority, Priority::Default);
        assert_eq!(n.sound, Sound::Default);
        assert!(n.actions.is_empty());
        assert_eq!(n, Notification::new("a", "b"));
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
        assert_eq!(NotificationId::from(String::from("y")), NotificationId("y".into()));
        assert_eq!(NotificationId("z".into()).to_string(), "z");
    }

    #[test]
    fn error_display() {
        assert_eq!(NotifyError::NotAuthorized.to_string(), "notifications not authorized");
        assert!(NotifyError::Backend("boom".into()).to_string().contains("boom"));
    }

    #[test]
    fn subtitle_folds_onto_the_body() {
        assert_eq!(body_with_subtitle(&Notification::new("T", "B").subtitle("S")), "S\nB");
        assert_eq!(body_with_subtitle(&Notification::new("T", "B").subtitle("")), "B");
        assert_eq!(body_with_subtitle(&Notification::new("T", "B")), "B");
    }

    #[test]
    fn visible_actions_caps_at_the_platform_limit() {
        let n = (0..5).fold(Notification::new("a", "b"), |n, i| n.action(Action::new(format!("a{i}"), "x")));
        assert_eq!(visible_actions(&n, 3).len(), 3);
        assert_eq!(visible_actions(&n, 10).len(), 5);
    }

    #[test]
    fn labels_default_to_english_and_are_replaceable() {
        assert_eq!(Labels::default().for_priority(Priority::Default), "Notifications");
        let custom = Labels { low: "Leise".into(), ..Labels::default() };
        assert_eq!(custom.for_priority(Priority::Low), "Leise");
    }

    /// The bug class this guards: a response that arrives before any app
    /// code subscribes (the tap that cold-started the app) was dropped.
    /// It must wait for the first handler, reach it once, and reach every
    /// handler registered at that time.
    #[test]
    fn regression_launch_response_waits_for_the_first_handler() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let _queue = response::exclusive_queue();
        init();
        deliver_response(
            NotificationId::from("launch"),
            ResponseAction::Action("reply".into()),
            Some("on my way".into()),
            HashMap::from([
                ("route".to_string(), "/chat/7".to_string()),
                ("idealyst.action".to_string(), "reply".to_string()),
            ]),
        );
        assert_eq!(response::pending_len(), 1, "held until a handler exists");

        let seen: Rc<RefCell<Vec<NotificationResponse>>> = Rc::default();
        let sink = seen.clone();
        let sub = on_response(move |r| sink.borrow_mut().push(r.clone()));
        assert_eq!(response::pending_len(), 0);
        assert_eq!(
            *seen.borrow(),
            vec![NotificationResponse {
                id: NotificationId::from("launch"),
                action: ResponseAction::Action("reply".into()),
                text: Some("on my way".into()),
                data: HashMap::from([("route".to_string(), "/chat/7".to_string())]),
            }],
            "delivered once, reserved payload keys stripped"
        );

        // A later response reaches the live handler; after the
        // subscription drops, responses queue again.
        deliver_response(NotificationId::from("n2"), ResponseAction::Open, None, HashMap::new());
        assert_eq!(seen.borrow().len(), 2);
        drop(sub);
        deliver_response(NotificationId::from("n3"), ResponseAction::Dismiss, None, HashMap::new());
        assert_eq!(seen.borrow().len(), 2);
        assert_eq!(response::pending_len(), 1);
        let _sub = on_response(|_| {});
        assert_eq!(response::pending_len(), 0);
    }

    /// On a host with the **fallback** `imp` (a CI runner — *not* macOS,
    /// whose `apple` backend reports `Backend` in a non-bundled test binary,
    /// *not* Linux, whose D-Bus
    /// backend needs a live session bus, *not* Windows, which posts real
    /// toasts) posts are no-op successes and there's no push token.
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "android",
        target_os = "linux",
        target_os = "windows"
    )))]
    #[tokio::test]
    async fn host_notify_returns_resolved_id() {
        let id = notify(Notification::new("Hi", "there").id("greeting")).await.unwrap();
        assert_eq!(id, NotificationId("greeting".into()));

        let gen = notify(Notification::new("Hi", "again")).await.unwrap();
        assert!(gen.as_str().starts_with("idealyst-"));

        // cancel / cancel_all are no-ops, must not panic.
        cancel(&id).await;
        cancel_all().await;

        assert_eq!(push_token().await, Err(NotifyError::NotSupported));
    }

    /// `authorize()` resolves on a fallback host (no native permission model
    /// → `Unsupported`, which `is_usable()` accepts).
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "android",
        // Windows reports the app's real notification setting.
        target_os = "windows"
    )))]
    #[tokio::test]
    async fn host_authorize_is_usable() {
        assert!(authorize().await.is_usable());
    }
}
