//! Windows local notifications — WinRT toasts (`Windows.UI.Notifications`).
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
//!   of its own — the template has no distinct field), then the body. The
//!   [`NotificationId`] is the toast's `Tag` (group `idealyst`), so posting
//!   again under the same id replaces the toast in place.
//! - `schedule` hands a `ScheduledToastNotification` to the OS scheduler —
//!   it fires at the due time whether or not the app is still running (unlike
//!   Linux / Android, where there is no OS one-shot trigger and `schedule` is
//!   `NotSupported`). A pending one with the same id is replaced.
//! - `cancel` removes the toast from the notification history (delivered) and
//!   from the schedule (pending); `cancel_all` clears both for the app.
//! - Authorization is `permissions::status(Notifications)` — on Windows the
//!   app's notification setting (Windows never prompts; the user turns an
//!   app's notifications off in Settings). Posting while it is off returns
//!   [`NotifyError::NotAuthorized`] — `Show` itself would drop it silently.
//! - The `data` map rides in the toast's `launch` argument (URL-encoded),
//!   where an activation handler reads it.
//! - `push_token`: WNS channels exist only for packaged (MSIX) apps, so
//!   [`NotifyError::NotSupported`].

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::DateTime;
use windows::UI::Notifications::{
    ScheduledToastNotification, ToastNotification, ToastNotificationManager, ToastNotifier,
};

use crate::{resolve_id, Notification, NotificationId, NotifyError, PushToken};

/// The toast group every notification from this SDK is posted in.
const GROUP: &str = "idealyst";
/// Windows caps a toast `Tag` at 64 characters.
const MAX_TAG: usize = 64;

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

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let notifier = authorized_notifier().await?;
    let toast = ToastNotification::CreateToastNotification(&document(&n)?).map_err(backend)?;
    toast.SetTag(&HSTRING::from(tag(&id))).map_err(backend)?;
    toast.SetGroup(&HSTRING::from(GROUP)).map_err(backend)?;
    notifier.Show(&toast).map_err(backend)?;
    Ok(id)
}

pub(super) async fn schedule(n: Notification, after: Duration) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let notifier = authorized_notifier().await?;
    unschedule(&notifier, Some(&tag(&id)));
    let toast = ScheduledToastNotification::CreateScheduledToastNotification(&document(&n)?, due(after))
        .map_err(backend)?;
    toast.SetTag(&HSTRING::from(tag(&id))).map_err(backend)?;
    toast.SetGroup(&HSTRING::from(GROUP)).map_err(backend)?;
    notifier.AddToSchedule(&toast).map_err(backend)?;
    Ok(id)
}

pub(super) async fn cancel(id: &NotificationId) {
    let tag = tag(id);
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(&HSTRING::from(&tag), &HSTRING::from(GROUP), &app_id());
    }
    if let Ok(notifier) = notifier() {
        unschedule(&notifier, Some(&tag));
    }
}

pub(super) async fn cancel_all() {
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

fn document(n: &Notification) -> Result<XmlDocument, NotifyError> {
    let doc = XmlDocument::new().map_err(backend)?;
    doc.LoadXml(&HSTRING::from(toast_xml(n))).map_err(backend)?;
    Ok(doc)
}

/// The toast's XML payload.
fn toast_xml(n: &Notification) -> String {
    let mut lines = vec![n.title.as_str()];
    if let Some(sub) = n.subtitle.as_deref().filter(|s| !s.is_empty()) {
        lines.push(sub);
    }
    lines.push(&n.body);
    let texts: String = lines.iter().map(|l| format!("<text>{}</text>", escape(l))).collect();
    let mut data: Vec<_> = n.data.iter().collect();
    data.sort();
    let launch: Vec<String> = data.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect();
    format!(
        r#"<toast launch="{}"><visual><binding template="ToastGeneric">{texts}</binding></visual></toast>"#,
        escape(&launch.join("&"))
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_xml_escapes_text_and_carries_data_in_launch() {
        let n = Notification::new("A & B", "<body>").subtitle("sub").with("route", "/notes/1?x=y");
        assert_eq!(
            toast_xml(&n),
            r#"<toast launch="route=%2Fnotes%2F1%3Fx%3Dy"><visual><binding template="ToastGeneric"><text>A &amp; B</text><text>sub</text><text>&lt;body&gt;</text></binding></visual></toast>"#
        );
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

