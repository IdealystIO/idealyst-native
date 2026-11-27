//! Windows local notifications and their responses — WinRT toasts
//! (`Windows.UI.Notifications`).
//!
//! ## Identity
//!
//! A toast is shown for an AppUserModelID Windows knows. The Win32 host
//! identifies the app at startup (`host_win32::set_app_identity`: the
//! manifest's bundle id + app name); a process no host identified gets an id
//! derived from its executable. Both through
//! `backend_windows::app_identity`.
//!
//! ## Mechanism
//!
//! - `notify` shows a `ToastGeneric` toast: title, then the subtitle (a line
//!   of its own — the template has no distinct field), then the body, a hero
//!   image, `<audio silent>` for [`Sound::Silent`], and an `<actions>` block
//!   (a text `<input>` for an action with a [`TextInput`](crate::TextInput),
//!   then the buttons, at most five). The XML goes through the app's
//!   [`Notification::windows`] hook before the toast is created. The
//!   [`NotificationId`] is the toast's `Tag` (group `idealyst`), so posting
//!   again under the same id replaces the toast in place, popping up
//!   again — unless it is [`Notification::alert_once`] and the toast is
//!   still in the notification history, which suppresses the popup.
//! - [`Priority::Low`] suppresses the popup (the toast goes straight to the
//!   notification center); `High` / `Urgent` set the toast's `High`
//!   priority.
//! - `schedule` hands a `ScheduledToastNotification` to the OS scheduler —
//!   it fires at the due time whether or not the app is still running. A
//!   pending one with the same id is replaced.
//! - `cancel` removes the toast from the notification history (delivered) and
//!   from the schedule (pending); `cancel_all` clears both for the app.
//! - Authorization is `permissions::status(Notifications)` — on Windows the
//!   app's notification setting (Windows never prompts; the user turns an
//!   app's notifications off in Settings). Posting while it is off returns
//!   [`NotifyError::NotAuthorized`] — `Show` itself would drop it silently.
//! - `push_token`: WNS channels exist only for packaged (MSIX) apps, so
//!   [`NotifyError::NotSupported`].
//!
//! ## Responses
//!
//! Every activation target — the toast body and each button — carries
//! URL-encoded arguments: the payload, plus the reserved `idealyst.id` and
//! `idealyst.action` keys naming the notification and what was pressed.
//! The toast's `Activated` event (a thread-pool thread) parses them back
//! into a [`crate::NotificationResponse`], with the reply read from the
//! event's `UserInput` under the action's id; `Dismissed` with reason
//! `UserCanceled` reports [`ResponseAction::Dismiss`] (an expiry or an app
//! cancel is not a dismissal). The shown toasts are kept in [`LIVE`] so
//! their handlers stay registered.
//!
//! An unpackaged Win32 app receives these events only while it runs and
//! only for toasts it showed in this process: activating a toast after the
//! app exited — or a scheduled one, which has no events — needs a
//! registered COM activator, which a plain executable does not have, so
//! nothing is reported for it.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::core::{IInspectable, Interface, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::{DateTime, IPropertyValue, TypedEventHandler};
use windows::UI::Notifications::{
    ScheduledToastNotification, ToastActivatedEventArgs, ToastDismissalReason, ToastDismissedEventArgs,
    ToastNotification, ToastNotificationManager, ToastNotificationPriority, ToastNotifier,
};

use crate::{
    deliver_response, resolve_id, visible_actions, Notification, NotificationId, NotifyError, Priority, PushToken,
    ResponseAction, Sound, RESERVED_PREFIX,
};

/// The toast group every notification from this SDK is posted in.
const GROUP: &str = "idealyst";
/// Windows caps a toast `Tag` at 64 characters.
const MAX_TAG: usize = 64;
/// A toast shows at most five buttons.
const MAX_ACTIONS: usize = 5;
/// The reserved argument naming the notification responded to.
const ARG_ID: &str = "idealyst.id";
/// The reserved argument naming what was pressed: `open` for the body, else
/// the action id.
const ARG_ACTION: &str = "idealyst.action";
const OPEN: &str = "open";

/// The toasts shown in this process, by tag — holding them keeps their
/// `Activated` / `Dismissed` handlers registered.
static LIVE: LazyLock<Mutex<HashMap<String, ToastNotification>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn backend(e: windows::core::Error) -> NotifyError {
    NotifyError::Backend(format!("WinRT: {e}"))
}

fn app_id() -> HSTRING {
    HSTRING::from(backend_windows::app_identity::current())
}

fn notifier() -> Result<ToastNotifier, NotifyError> {
    ToastNotificationManager::CreateToastNotifierWithId(&app_id()).map_err(backend)
}

/// The notifier, if the user has not turned the app's notifications off.
async fn authorized_notifier() -> Result<ToastNotifier, NotifyError> {
    if !permissions::status(permissions::Permission::Notifications).await.is_usable() {
        return Err(NotifyError::NotAuthorized);
    }
    notifier()
}

pub(super) fn init() {}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let notifier = authorized_notifier().await?;
    let toast = ToastNotification::CreateToastNotification(&document(&n, &id)?).map_err(backend)?;
    let tag = tag(&id);
    toast.SetTag(&HSTRING::from(&tag)).map_err(backend)?;
    toast.SetGroup(&HSTRING::from(GROUP)).map_err(backend)?;
    match n.priority {
        Priority::Low => toast.SetSuppressPopup(true).map_err(backend)?,
        Priority::Default => {}
        Priority::High | Priority::Urgent => toast.SetPriority(ToastNotificationPriority::High).map_err(backend)?,
    }
    if n.alert_once && in_history(&tag) {
        // Replaces the toast in the notification center without a popup.
        toast.SetSuppressPopup(true).map_err(backend)?;
    }
    listen(&toast, id.clone(), n.data.clone()).map_err(backend)?;
    notifier.Show(&toast).map_err(backend)?;
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(tag, toast);
    Ok(id)
}

/// Whether a toast tagged `tag` is still shown — on screen or in the
/// notification center. The history, not [`LIVE`]: a toast whose popup
/// timed out leaves `LIVE` (its `Dismissed` fires) but stays in the
/// notification center.
fn in_history(tag: &str) -> bool {
    let Ok(history) = ToastNotificationManager::History() else { return false };
    let Ok(list) = history.GetHistoryWithId(&app_id()) else { return false };
    list.into_iter().any(|t| {
        t.Tag().map(|have| have.to_string_lossy() == tag).unwrap_or(false)
            && t.Group().map(|g| g.to_string_lossy() == GROUP).unwrap_or(false)
    })
}

/// Register the toast's `Activated` / `Dismissed` handlers: each reports a
/// response for `id` with `data` as its payload fallback.
fn listen(toast: &ToastNotification, id: NotificationId, data: HashMap<String, String>) -> windows::core::Result<()> {
    let activated_id = id.clone();
    let activated_data = data.clone();
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        let Ok(args) = args.cast::<ToastActivatedEventArgs>() else { return Ok(()) };
        let arguments = args.Arguments().map(|a| a.to_string_lossy()).unwrap_or_default();
        let (action, mut payload) = parse_arguments(&arguments);
        if payload.is_empty() {
            payload = activated_data.clone();
        }
        let text = match &action {
            ResponseAction::Action(action_id) => user_input(&args, action_id),
            _ => None,
        };
        forget(&activated_id);
        deliver_response(activated_id.clone(), action, text, payload);
        Ok(())
    }))?;
    toast.Dismissed(&TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(move |_, args| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        forget(&id);
        if args.Reason()? == ToastDismissalReason::UserCanceled {
            deliver_response(id.clone(), ResponseAction::Dismiss, None, data.clone());
        }
        Ok(())
    }))?;
    Ok(())
}

/// The reply typed into the `<input>` with `input_id`, if any.
fn user_input(args: &ToastActivatedEventArgs, input_id: &str) -> Option<String> {
    let inputs = args.UserInput().ok()?;
    let value = inputs.Lookup(&HSTRING::from(input_id)).ok()?;
    let text = value.cast::<IPropertyValue>().ok()?.GetString().ok()?.to_string_lossy();
    (!text.is_empty()).then_some(text)
}

fn forget(id: &NotificationId) {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&tag(id));
}

pub(super) async fn schedule(n: Notification, after: Duration) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let notifier = authorized_notifier().await?;
    unschedule(&notifier, Some(&tag(&id)));
    let toast = ScheduledToastNotification::CreateScheduledToastNotification(&document(&n, &id)?, due(after))
        .map_err(backend)?;
    toast.SetTag(&HSTRING::from(tag(&id))).map_err(backend)?;
    toast.SetGroup(&HSTRING::from(GROUP)).map_err(backend)?;
    if n.priority == Priority::Low {
        toast.SetSuppressPopup(true).map_err(backend)?;
    }
    notifier.AddToSchedule(&toast).map_err(backend)?;
    Ok(id)
}

pub(super) async fn cancel(id: &NotificationId) {
    let tag = tag(id);
    forget(id);
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(&HSTRING::from(&tag), &HSTRING::from(GROUP), &app_id());
    }
    if let Ok(notifier) = notifier() {
        unschedule(&notifier, Some(&tag));
    }
}

pub(super) async fn cancel_all() {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.ClearWithId(&app_id());
    }
    if let Ok(notifier) = notifier() {
        unschedule(&notifier, None);
    }
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    Err(NotifyError::NotSupported)
}

/// Remove pending scheduled toasts: the one tagged `tag`, or all of them.
fn unschedule(notifier: &ToastNotifier, tag: Option<&str>) {
    let Ok(pending) = notifier.GetScheduledToastNotifications() else { return };
    for toast in pending {
        let matches = match tag {
            Some(t) => toast.Tag().map(|have| have.to_string_lossy() == t).unwrap_or(false),
            None => true,
        };
        if matches {
            let _ = notifier.RemoveFromSchedule(&toast);
        }
    }
}

/// The toast tag for `id`: the id itself, or — past Windows' 64-character
/// cap — a stable hash of it (so update and cancel still find it).
fn tag(id: &NotificationId) -> String {
    let s = id.as_str();
    if s.chars().count() <= MAX_TAG {
        return s.to_string();
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("idealyst-{:016x}", h.finish())
}

/// `after` from now as a WinRT `DateTime` (100 ns ticks since 1601-01-01).
fn due(after: Duration) -> DateTime {
    /// 1601-01-01 → 1970-01-01, in 100 ns ticks.
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let at = SystemTime::now() + after;
    let since_unix = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    DateTime { UniversalTime: UNIX_EPOCH_TICKS + (since_unix.as_nanos() / 100) as i64 }
}

fn document(n: &Notification, id: &NotificationId) -> Result<XmlDocument, NotifyError> {
    let doc = XmlDocument::new().map_err(backend)?;
    doc.LoadXml(&HSTRING::from(toast_xml(n, id))).map_err(backend)?;
    if let Some(hook) = &n.hooks.windows {
        hook(&doc).map_err(backend)?;
    }
    Ok(doc)
}

/// The toast's XML payload.
fn toast_xml(n: &Notification, id: &NotificationId) -> String {
    let mut lines = vec![n.title.as_str()];
    if let Some(sub) = n.subtitle.as_deref().filter(|s| !s.is_empty()) {
        lines.push(sub);
    }
    lines.push(&n.body);
    let mut binding: String = lines.iter().map(|l| format!("<text>{}</text>", escape(l))).collect();
    if let Some(image) = &n.image {
        binding.push_str(&format!(r#"<image placement="hero" src="{}"/>"#, escape(&image_src(image))));
    }
    let audio = if n.sound == Sound::Silent { r#"<audio silent="true"/>"# } else { "" };
    let actions = visible_actions(n, MAX_ACTIONS);
    let actions_xml = if actions.is_empty() {
        String::new()
    } else {
        let inputs: String = actions
            .iter()
            .filter_map(|a| {
                let input = a.input.as_ref()?;
                Some(format!(
                    r#"<input id="{}" type="text" placeHolderContent="{}"/>"#,
                    escape(&a.id),
                    escape(&input.placeholder)
                ))
            })
            .collect();
        let buttons: String = actions
            .iter()
            .map(|a| {
                let (label, hint) = match &a.input {
                    Some(input) => (input.button_title.as_str(), format!(r#" hint-inputId="{}""#, escape(&a.id))),
                    None => (a.title.as_str(), String::new()),
                };
                format!(
                    r#"<action content="{}" arguments="{}"{hint}/>"#,
                    escape(label),
                    escape(&arguments(n, id, &a.id))
                )
            })
            .collect();
        format!("<actions>{inputs}{buttons}</actions>")
    };
    format!(
        r#"<toast launch="{}"><visual><binding template="ToastGeneric">{binding}</binding></visual>{audio}{actions_xml}</toast>"#,
        escape(&arguments(n, id, OPEN))
    )
}

/// A local path as a `file:///` URI; URIs pass through.
fn image_src(image: &str) -> String {
    if image.contains("://") {
        image.to_string()
    } else {
        format!("file:///{}", image.replace('\\', "/").trim_start_matches('/'))
    }
}

/// The activation arguments for pressing `action` on `n`: the payload, then
/// the reserved id and action keys, URL-encoded.
fn arguments(n: &Notification, id: &NotificationId, action: &str) -> String {
    let mut data: Vec<_> = n.data.iter().filter(|(k, _)| !k.starts_with(RESERVED_PREFIX)).collect();
    data.sort();
    let mut pairs: Vec<String> = data.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect();
    pairs.push(format!("{}={}", url_encode(ARG_ID), url_encode(id.as_str())));
    pairs.push(format!("{}={}", url_encode(ARG_ACTION), url_encode(action)));
    pairs.join("&")
}

/// The pressed action and the payload carried in activation `arguments`.
fn parse_arguments(arguments: &str) -> (ResponseAction, HashMap<String, String>) {
    let mut data = HashMap::new();
    let mut action = ResponseAction::Open;
    for pair in arguments.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (url_decode(k), url_decode(v));
        if k == ARG_ACTION {
            if v != OPEN {
                action = ResponseAction::Action(v);
            }
        } else if !k.starts_with(RESERVED_PREFIX) {
            data.insert(k, v);
        }
    }
    (action, data)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_xml_escapes_text_and_carries_data_in_launch() {
        let n = Notification::new("A & B", "<body>").subtitle("sub").with("route", "/notes/1?x=y");
        assert_eq!(
            toast_xml(&n, &NotificationId::from("n1")),
            r#"<toast launch="route=%2Fnotes%2F1%3Fx%3Dy&amp;idealyst.id=n1&amp;idealyst.action=open"><visual><binding template="ToastGeneric"><text>A &amp; B</text><text>sub</text><text>&lt;body&gt;</text></binding></visual></toast>"#
        );
    }

    #[test]
    fn toast_xml_writes_inputs_buttons_image_and_silence() {
        let n = Notification::new("T", "B")
            .action(crate::Action::new("reply", "Reply").text_input("Send", "Say <hi>"))
            .action(crate::Action::new("mute", "Mute"))
            .sound(Sound::Silent)
            .image(r"C:\pics\a.png");
        let xml = toast_xml(&n, &NotificationId::from("chat"));
        assert!(xml.contains(r#"<image placement="hero" src="file:///C:/pics/a.png"/>"#), "{xml}");
        assert!(xml.contains(r#"<audio silent="true"/>"#), "{xml}");
        assert!(
            xml.contains(concat!(
                r#"<actions><input id="reply" type="text" placeHolderContent="Say &lt;hi&gt;"/>"#,
                r#"<action content="Send" arguments="idealyst.id=chat&amp;idealyst.action=reply" hint-inputId="reply"/>"#,
                r#"<action content="Mute" arguments="idealyst.id=chat&amp;idealyst.action=mute"/></actions>"#,
            )),
            "{xml}"
        );
    }

    #[test]
    fn activation_arguments_round_trip() {
        let n = Notification::new("T", "B").with("route", "/a b&c=d").with("idealyst.secret", "x");
        let id = NotificationId::from("n&1");
        let (action, data) = parse_arguments(&arguments(&n, &id, "reply"));
        assert_eq!(action, ResponseAction::Action("reply".into()));
        assert_eq!(data, HashMap::from([("route".to_string(), "/a b&c=d".to_string())]));
        assert_eq!(parse_arguments(&arguments(&n, &id, OPEN)).0, ResponseAction::Open);
        assert_eq!(url_decode("%E2%9C%93%2"), "✓%2", "a truncated escape stays literal");
    }

    #[test]
    fn long_ids_get_a_stable_tag_within_the_cap() {
        let short = NotificationId::from("note-1");
        assert_eq!(tag(&short), "note-1");
        let long = NotificationId::from("x".repeat(100));
        assert!(tag(&long).len() <= MAX_TAG);
        assert_eq!(tag(&long), tag(&long.clone()));
    }

    #[test]
    fn due_is_after_now_in_windows_ticks() {
        let now = due(Duration::ZERO).UniversalTime;
        let later = due(Duration::from_secs(60)).UniversalTime;
        assert!((later - now - 600_000_000).abs() < 10_000_000, "{now} {later}");
    }

    /// The bug: Windows fell to the no-op stub — `notify` "succeeded" and
    /// showed nothing, `schedule` scheduled nothing. A scheduled toast is really in the
    /// OS schedule under its id until cancelled. (A shown toast cannot be
    /// read back reliably from a test process — the history only lists
    /// toasts still in Action Center — so the schedule is what is checked.)
    #[test]
    fn regression_windows_schedule_and_cancel_reach_the_os_schedule() {
        backend_windows::app_identity::set("idealyst.test.notifications", "Notifications test").unwrap();
        let pending = |id: &str| {
            notifier()
                .unwrap()
                .GetScheduledToastNotifications()
                .unwrap()
                .into_iter()
                .filter(|t| t.Tag().unwrap().to_string_lossy() == id)
                .count()
        };
        let id = pollster::block_on(schedule(
            Notification::new("Later", "scheduled from a test").id("sched-test"),
            Duration::from_secs(3600),
        ));
        match id {
            // Notifications turned off for apps on the test machine: the
            // authorization path is what ran. Nothing else to check.
            Err(NotifyError::NotAuthorized) => return,
            other => assert_eq!(other, Ok(NotificationId::from("sched-test"))),
        }
        assert_eq!(pending("sched-test"), 1);
        // Re-scheduling under the same id replaces, not duplicates.
        pollster::block_on(schedule(Notification::new("Later", "again").id("sched-test"), Duration::from_secs(3600)))
            .unwrap();
        assert_eq!(pending("sched-test"), 1);
        pollster::block_on(cancel(&NotificationId::from("sched-test")));
        assert_eq!(pending("sched-test"), 0);
    }
}

