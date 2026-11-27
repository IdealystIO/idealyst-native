//! Linux desktop local notifications and their responses via the
//! session-bus service `org.freedesktop.Notifications` (the Desktop
//! Notifications Specification), driven directly through `zbus`.
//!
//! VERIFICATION: type-checked for `x86_64-unknown-linux-gnu`; the argument
//! builder, the signal mapping and the error mapping are unit-tested below.
//! The live post is exercised by the `#[ignore]`d `live_notify_posts`
//! integration test, which needs a running session bus + a notification
//! daemon and so can't run in a headless CI job.
//!
//! ## Posting
//!
//! `notify` calls the `Notify` method (`app_name, replaces_id, app_icon,
//! summary, body, actions, hints, expire_timeout`) and gets back a
//! server-assigned `u32` id. Shared fields map to the spec:
//! - actions → the `actions` list of `key, label` pairs, led by the
//!   `default` key (a click on the body);
//! - a text-input action → the `inline-reply` action and the
//!   `x-kde-reply-*` hints (KDE Plasma's inline reply; a daemon without
//!   the `inline-reply` capability shows it as a plain button and the
//!   response carries no text);
//! - [`Priority`] → the `urgency` hint (low / normal / critical);
//! - [`Sound::Silent`] → `suppress-sound`; the image → `image-path`;
//! - re-posting an id passes its previous server id as `replaces_id`, so
//!   the daemon updates it in place (whether a replaced notification pops
//!   up again is the daemon's choice); an [`Notification::alert_once`]
//!   replacement adds `suppress-sound`;
//! - the payload → `x-idealyst-<key>` hints (app-private, ignored by
//!   daemons);
//! - then the app's [`Notification::linux`] hook edits the hints map.
//!
//! `cancel` / `cancel_all` call `CloseNotification(u32)`.
//!
//! ## Responses
//!
//! [`init`] starts one watcher thread holding its own session-bus
//! connection, subscribed to the service's signals:
//! `ActionInvoked(id, key)` → [`ResponseAction::Open`] for `default`, an
//! action otherwise; `NotificationReplied(id, text)` → the text-input
//! action with its text; `NotificationClosed(id, 2)` (dismissed by the
//! user) → [`ResponseAction::Dismiss`]. Daemons follow an action with a
//! `NotificationClosed` for the same id, so the first response for an id
//! retires it and later signals for it are ignored. The signals are
//! broadcast to every client; only ids this process posted are reported.
//! Responses arrive only while the app runs.
//!
//! ## String id ↔ server u32
//!
//! The public API keys notifications by a *string* [`NotificationId`], the
//! protocol by a server-assigned `u32`: `replaces_id = 0` posts anew and the
//! daemon returns a fresh id; passing a previous `u32` back replaces that
//! notification. [`REGISTRY`] maps both ways so re-posting under the same
//! id replaces it on screen, `cancel(id)` finds the `u32` to close, and a
//! signal finds the notification it is about. There is no `CloseAll` in
//! the spec, so `cancel_all` closes each tracked id.
//!
//! ## No subtitle / no scheduling / no grouping / no push token
//!
//! - No distinct *subtitle* field: it is folded into the body.
//! - No delayed trigger (`expire_timeout` is how long a *shown*
//!   notification lingers), so `schedule` reports `NotSupported` rather
//!   than tie delivery to a process-lifetime timer.
//! - No grouping concept: `group` is dropped.
//! - No remote-push token on a Linux desktop: `NotSupported`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use zbus::zvariant::Value;
use zbus::Connection;

use crate::{
    body_with_subtitle, deliver_response, resolve_id, Notification, NotificationId, NotifyError, Priority,
    PushToken, ResponseAction, Sound,
};

const BUS_NAME: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";

/// The `app_name` reported to the daemon (shown in some UIs / used for
/// grouping). A fixed identifier — the SDK doesn't know the host app's name;
/// the `desktop-entry` hint (through the hook) is the spec's way to name it.
const APP_NAME: &str = "Idealyst";

/// `expire_timeout = -1` = let the server pick the default lifetime. (0 would
/// mean "never expire"; a positive value is milliseconds.)
const EXPIRE_DEFAULT: i32 = -1;

/// The action key of a click on the notification body.
const DEFAULT_KEY: &str = "default";
/// The action key KDE Plasma reports an inline reply under.
const INLINE_REPLY_KEY: &str = "inline-reply";
/// `NotificationClosed` reason: dismissed by the user.
const CLOSED_BY_USER: u32 = 2;

/// A notification this process has on screen.
#[derive(Clone)]
struct Live {
    server_id: u32,
    data: HashMap<String, String>,
    /// The action id of the text-input action, posted as `inline-reply`.
    reply_action: Option<String>,
}

/// Our string id → the daemon's last server-assigned `u32` for it (and what
/// a response needs). Lazily initialized (the default-hasher
/// `HashMap::new()` isn't a const fn).
static REGISTRY: LazyLock<Mutex<HashMap<NotificationId, Live>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn map_zbus(e: zbus::Error) -> NotifyError {
    NotifyError::Backend(format!("D-Bus: {e}"))
}

/// The `replaces_id` to send for `id`: the previously-returned server id if we
/// have one (→ replace the on-screen notification), else `0` (→ new post).
fn replaces_id(id: &NotificationId) -> u32 {
    REGISTRY.lock().expect("notification registry poisoned").get(id).map(|l| l.server_id).unwrap_or(0)
}

fn remember(id: &NotificationId, live: Live) {
    REGISTRY.lock().expect("notification registry poisoned").insert(id.clone(), live);
}

/// The notification posted under `server_id`, removed from the registry —
/// the first response for a notification retires it.
fn retire(server_id: u32) -> Option<(NotificationId, Live)> {
    let mut reg = REGISTRY.lock().expect("notification registry poisoned");
    let id = reg.iter().find(|(_, l)| l.server_id == server_id).map(|(id, _)| id.clone())?;
    reg.remove(&id).map(|live| (id, live))
}

/// The pure, bus-free part of building a `Notify` call. Separated so it's
/// unit-testable without a live daemon.
struct NotifyArgs {
    summary: String,
    body: String,
    /// `key, label` pairs, flattened as the spec's `actions` array wants.
    actions: Vec<String>,
    /// The hints, keyed by name.
    hints: HashMap<String, Value<'static>>,
    expire_timeout: i32,
    reply_action: Option<String>,
}

/// `replacing`: the post replaces a notification of ours still shown.
fn build_args(n: &Notification, replacing: bool) -> NotifyArgs {
    let mut hints: HashMap<String, Value<'static>> = n
        .data
        .iter()
        .map(|(k, v)| (format!("x-idealyst-{k}"), Value::from(v.clone())))
        .collect();
    let urgency: u8 = match n.priority {
        Priority::Low => 0,
        Priority::Default | Priority::High => 1,
        Priority::Urgent => 2,
    };
    hints.insert("urgency".into(), Value::U8(urgency));
    // An `alert_once` replacement is updated in place by the `replaces_id`
    // alone; it only has to keep quiet.
    if n.sound == Sound::Silent || (n.alert_once && replacing) {
        hints.insert("suppress-sound".into(), Value::Bool(true));
    }
    if let Some(image) = &n.image {
        hints.insert("image-path".into(), Value::from(image.clone()));
    }

    let mut actions = vec![DEFAULT_KEY.to_string(), String::new()];
    let mut reply_action = None;
    for a in &n.actions {
        match (&a.input, &reply_action) {
            // One inline reply per notification (the spec extension has one).
            (Some(input), None) => {
                reply_action = Some(a.id.clone());
                actions.push(INLINE_REPLY_KEY.into());
                actions.push(a.title.clone());
                hints.insert("x-kde-reply-placeholder-text".into(), Value::from(input.placeholder.clone()));
                hints.insert("x-kde-reply-submit-button-text".into(), Value::from(input.button_title.clone()));
            }
            _ => {
                actions.push(a.id.clone());
                actions.push(a.title.clone());
            }
        }
    }
    if let Some(hook) = &n.hooks.linux {
        hook(&mut hints);
    }
    NotifyArgs {
        summary: n.title.clone(),
        body: body_with_subtitle(n),
        actions,
        hints,
        expire_timeout: EXPIRE_DEFAULT,
        reply_action,
    }
}

pub(super) fn init() {
    // One watcher for the process; it ends with the process. A failure to
    // reach the bus leaves the app posting-only, as it would without a
    // daemon at all.
    let _ = std::thread::Builder::new().name("notifications-dbus".into()).spawn(|| {
        if let Err(e) = watch_signals() {
            eprintln!("notifications: D-Bus signal watcher stopped: {e}");
        }
    });
}

fn watch_signals() -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)?
        .path(OBJECT_PATH)?
        .build();
    for msg in zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None)? {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        let Some(member) = header.member() else { continue };
        let signal = match member.as_str() {
            "ActionInvoked" => msg.body().deserialize::<(u32, String)>().ok().map(|(id, key)| Signal::Action(id, key)),
            "NotificationReplied" => {
                msg.body().deserialize::<(u32, String)>().ok().map(|(id, text)| Signal::Replied(id, text))
            }
            "NotificationClosed" => {
                msg.body().deserialize::<(u32, u32)>().ok().map(|(id, reason)| Signal::Closed(id, reason))
            }
            _ => None,
        };
        if let Some(signal) = signal {
            handle_signal(signal);
        }
    }
    Ok(())
}

/// One of the service's signals about a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Signal {
    Action(u32, String),
    Replied(u32, String),
    Closed(u32, u32),
}

/// Report `signal` as a response if it is about a notification this
/// process posted (and has not already responded to).
fn handle_signal(signal: Signal) {
    let server_id = match &signal {
        Signal::Action(id, _) | Signal::Replied(id, _) | Signal::Closed(id, _) => *id,
    };
    if let Signal::Closed(_, reason) = signal {
        if reason != CLOSED_BY_USER {
            // Expired or closed by an app: retire it, report nothing.
            retire(server_id);
            return;
        }
    }
    let Some((id, live)) = retire(server_id) else { return };
    let (action, text) = match signal {
        Signal::Action(_, key) if key == DEFAULT_KEY => (ResponseAction::Open, None),
        Signal::Action(_, key) if key == INLINE_REPLY_KEY => {
            (ResponseAction::Action(live.reply_action.clone().unwrap_or(key)), None)
        }
        Signal::Action(_, key) => (ResponseAction::Action(key), None),
        Signal::Replied(_, text) => {
            (ResponseAction::Action(live.reply_action.clone().unwrap_or_else(|| INLINE_REPLY_KEY.into())), Some(text))
        }
        Signal::Closed(..) => (ResponseAction::Dismiss, None),
    };
    deliver_response(id, action, text, live.data);
}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let replaces = replaces_id(&id);
    let args = build_args(&n, replaces != 0);

    let conn = Connection::session().await.map_err(map_zbus)?;
    let actions: Vec<&str> = args.actions.iter().map(String::as_str).collect();
    let hints: HashMap<&str, &Value> = args.hints.iter().map(|(k, v)| (k.as_str(), v)).collect();

    let reply = conn
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "Notify",
            &(
                APP_NAME,
                replaces,
                "", // app_icon — none; the daemon falls back to a default.
                args.summary.as_str(),
                args.body.as_str(),
                actions,
                hints,
                args.expire_timeout,
            ),
        )
        .await
        .map_err(map_zbus)?;

    let server_id: u32 = reply.body().deserialize().map_err(map_zbus)?;
    remember(&id, Live { server_id, data: n.data, reply_action: args.reply_action });
    Ok(id)
}

pub(super) async fn schedule(_n: Notification, _after: std::time::Duration) -> Result<NotificationId, NotifyError> {
    // No native scheduling primitive in the freedesktop spec (see module
    // docs).
    Err(NotifyError::NotSupported)
}

pub(super) async fn cancel(id: &NotificationId) {
    let live = REGISTRY.lock().expect("notification registry poisoned").remove(id);
    if let Some(live) = live {
        // Best-effort, mirroring the other backends' infallible `cancel`.
        let _ = close(live.server_id).await;
    }
}

pub(super) async fn cancel_all() {
    // Drain the registry, then close each tracked id (no CloseAll in the spec).
    let ids: Vec<u32> = {
        let mut reg = REGISTRY.lock().expect("notification registry poisoned");
        reg.drain().map(|(_, live)| live.server_id).collect()
    };
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(_) => return,
    };
    for server_id in ids {
        let _ = conn
            .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), "CloseNotification", &(server_id,))
            .await;
    }
}

/// Close a single server-assigned notification id via `CloseNotification`.
async fn close(server_id: u32) -> Result<(), NotifyError> {
    let conn = Connection::session().await.map_err(map_zbus)?;
    conn.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), "CloseNotification", &(server_id,))
        .await
        .map_err(map_zbus)?;
    Ok(())
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // No remote-push token model on a Linux desktop (see module docs).
    Err(NotifyError::NotSupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use std::collections::HashMap as StdHashMap;

    fn hint<'a>(args: &'a NotifyArgs, key: &str) -> Option<&'a Value<'static>> {
        args.hints.get(key)
    }

    #[test]
    fn alert_once_keeps_a_replacement_quiet() {
        let n = Notification::new("Upload", "60%").alert_once();
        assert_eq!(hint(&build_args(&n, true), "suppress-sound"), Some(&Value::Bool(true)));
        assert_eq!(hint(&build_args(&n, false), "suppress-sound"), None, "a first post alerts");
        let plain = Notification::new("Upload", "60%");
        assert_eq!(hint(&build_args(&plain, true), "suppress-sound"), None, "a plain replacement alerts");
    }

    #[test]
    fn build_args_summary_body_and_defaults() {
        let args = build_args(&Notification::new("Title", "Body").subtitle("Sub"), false);
        assert_eq!(args.summary, "Title");
        assert_eq!(args.body, "Sub\nBody");
        assert_eq!(args.expire_timeout, EXPIRE_DEFAULT);
        assert_eq!(args.actions, vec!["default".to_string(), String::new()], "a body click is always reported");
        assert_eq!(hint(&args, "urgency"), Some(&Value::U8(1)));
        assert_eq!(hint(&args, "suppress-sound"), None);
    }

    #[test]
    fn build_args_maps_data_priority_sound_and_image_to_hints() {
        let mut data = StdHashMap::new();
        data.insert("route".to_string(), "/inbox".to_string());
        let n = Notification::new("T", "B")
            .data(data)
            .priority(Priority::Urgent)
            .sound(Sound::Silent)
            .image("/tmp/a.png");
        let args = build_args(&n, false);
        assert_eq!(hint(&args, "x-idealyst-route"), Some(&Value::from("/inbox")));
        assert_eq!(hint(&args, "urgency"), Some(&Value::U8(2)));
        assert_eq!(hint(&args, "suppress-sound"), Some(&Value::Bool(true)));
        assert_eq!(hint(&args, "image-path"), Some(&Value::from("/tmp/a.png")));
    }

    #[test]
    fn build_args_lists_actions_and_one_inline_reply() {
        let n = Notification::new("T", "B")
            .action(Action::new("reply", "Reply").text_input("Send", "Message"))
            .action(Action::new("mute", "Mute"))
            .action(Action::new("note", "Note").text_input("Save", "Note"));
        let args = build_args(&n, false);
        assert_eq!(
            args.actions,
            ["default", "", "inline-reply", "Reply", "mute", "Mute", "note", "Note"].map(String::from).to_vec()
        );
        assert_eq!(args.reply_action.as_deref(), Some("reply"));
        assert_eq!(hint(&args, "x-kde-reply-placeholder-text"), Some(&Value::from("Message")));
        assert_eq!(hint(&args, "x-kde-reply-submit-button-text"), Some(&Value::from("Send")));
    }

    #[test]
    fn the_linux_hook_edits_the_hints() {
        let n = Notification::new("T", "B").linux(|hints| {
            hints.insert("category".into(), Value::from("im.received"));
            hints.remove("urgency");
        });
        let args = build_args(&n, false);
        assert_eq!(hint(&args, "category"), Some(&Value::from("im.received")));
        assert_eq!(hint(&args, "urgency"), None);
    }

    #[test]
    fn map_zbus_produces_backend_error() {
        let err = map_zbus(zbus::Error::Failure("boom".to_string()));
        match err {
            NotifyError::Backend(msg) => assert!(msg.contains("boom"), "got {msg}"),
            other => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn replaces_id_defaults_zero_then_tracks() {
        let id = NotificationId("linux-test-replaces".into());
        // Unknown id → 0 (a fresh post).
        assert_eq!(replaces_id(&id), 0);
        remember(&id, Live { server_id: 7, data: StdHashMap::new(), reply_action: None });
        // Known id → the server id (a replace).
        assert_eq!(replaces_id(&id), 7);
        REGISTRY.lock().expect("notification registry poisoned").remove(&id);
    }

    /// The bug class: daemons follow `ActionInvoked` with a
    /// `NotificationClosed(id, 2)` for the same notification, which read
    /// alone would report a dismissal after every button press. The first
    /// response retires the id; signals about ids this process never posted
    /// (other apps share the bus) are ignored.
    #[test]
    fn regression_signals_report_one_response_per_notification() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let _queue = crate::response::exclusive_queue();
        crate::response::start_pump();
        let seen: Rc<RefCell<Vec<crate::NotificationResponse>>> = Rc::default();
        let sink = seen.clone();
        let _sub = crate::on_response(move |r| sink.borrow_mut().push(r.clone()));
        let data = StdHashMap::from([("k".to_string(), "v".to_string())]);
        remember(&NotificationId::from("a"), Live { server_id: 901, data: data.clone(), reply_action: None });
        remember(
            &NotificationId::from("b"),
            Live { server_id: 902, data: StdHashMap::new(), reply_action: Some("reply".into()) },
        );
        remember(&NotificationId::from("c"), Live { server_id: 903, data: StdHashMap::new(), reply_action: None });
        remember(&NotificationId::from("d"), Live { server_id: 904, data: StdHashMap::new(), reply_action: None });

        handle_signal(Signal::Action(901, "default".into()));
        handle_signal(Signal::Closed(901, CLOSED_BY_USER));
        handle_signal(Signal::Replied(902, "on my way".into()));
        handle_signal(Signal::Closed(903, CLOSED_BY_USER));
        handle_signal(Signal::Closed(904, 1)); // expired
        handle_signal(Signal::Action(999, "default".into())); // another app's

        let got: Vec<_> = seen.borrow().iter().map(|r| (r.id.0.clone(), r.action.clone(), r.text.clone())).collect();
        assert_eq!(
            got,
            vec![
                ("a".to_string(), ResponseAction::Open, None),
                ("b".to_string(), ResponseAction::Action("reply".into()), Some("on my way".to_string())),
                ("c".to_string(), ResponseAction::Dismiss, None),
            ]
        );
        assert_eq!(seen.borrow()[0].data, data);
        assert_eq!(replaces_id(&NotificationId::from("d")), 0, "an expired notification is retired too");
    }

    /// Live post against the running notification daemon. Ignored because it
    /// needs a session bus with an `org.freedesktop.Notifications` owner (a
    /// desktop session), which a headless CI runner doesn't have. Run on a
    /// desktop with: `cargo test -p notifications --target
    /// x86_64-unknown-linux-gnu -- --ignored live_notify_posts`.
    #[ignore = "needs a live session bus + notification daemon"]
    #[tokio::test]
    async fn live_notify_posts() {
        let id = notify(Notification::new("Idealyst", "Live D-Bus test").id("idealyst-live"))
            .await
            .expect("notify should reach the daemon");
        assert_eq!(id, NotificationId("idealyst-live".into()));
        assert_ne!(replaces_id(&id), 0, "a re-post now replaces");
        cancel(&id).await;
    }
}
