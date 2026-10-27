//! Events from the OS, and the listener registry they are delivered to.
//!
//! Listeners live in a UI-thread thread-local: the platform half delivers
//! every event on the main thread, and a listener typically writes a signal,
//! which belongs to that thread. While at least one listener is registered
//! the platform half delivers events at once; with none, it queues them on
//! the device and replays the queue when the first listener registers.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Map, Value};

use crate::{reply, InstallResult};

/// Something the OS reported to the device-admin app.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// QR provisioning finished: the app is now device owner. `extras` is the
    /// QR code's admin extras bundle (`PROVISIONING_ADMIN_EXTRAS_BUNDLE`) —
    /// typically the server URL and an enrolment token. Delivered once per
    /// provisioning.
    ProvisioningComplete {
        /// The admin extras, as a JSON object.
        extras: Map<String, Value>,
    },
    /// The app became a device admin (`DeviceAdminReceiver.onEnabled`).
    AdminEnabled,
    /// The app stopped being a device admin.
    AdminDisabled,
    /// A package entered lock task mode. Device owner only.
    LockTaskEntering {
        /// The package now locked.
        package: String,
    },
    /// Lock task mode ended. Device owner only.
    LockTaskExiting,
    /// An [`install_apk`](crate::install_apk) or
    /// [`uninstall`](crate::uninstall) finished.
    InstallResult(InstallResult),
    /// A package was updated (`ACTION_PACKAGE_REPLACED`). Seen only while a
    /// listener is registered: Android does not deliver it to a stopped app.
    PackageReplaced {
        /// The updated package.
        package: String,
    },
    /// The device finished booting.
    BootCompleted,
}

/// Parse one event from the platform half. `None` for an event type this
/// version does not know (a newer platform half), which is skipped.
pub(crate) fn parse(json: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(json).ok()?;
    let string = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    Some(match v.get("type")?.as_str()? {
        "provisioning_complete" => Event::ProvisioningComplete {
            extras: v
                .get("extras")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        },
        "admin_enabled" => Event::AdminEnabled,
        "admin_disabled" => Event::AdminDisabled,
        "lock_task_entering" => Event::LockTaskEntering {
            package: string("package")?,
        },
        "lock_task_exiting" => Event::LockTaskExiting,
        "install_result" => Event::InstallResult(reply::install_result(v.clone()).ok()?),
        "package_replaced" => Event::PackageReplaced {
            package: string("package")?,
        },
        "boot_completed" => Event::BootCompleted,
        _ => return None,
    })
}

type Listener = Rc<dyn Fn(&Event)>;

runtime_tls::thread_local! {
    static LISTENERS: RefCell<(u64, Vec<(u64, Listener)>)> = const { RefCell::new((0, Vec::new())) };
}

/// Register `listener` for every [`Event`]. Delivery stops when the returned
/// subscription drops. Call on the UI thread: events are delivered there.
///
/// The first registration also replays events queued while nothing
/// listened (provisioning, boot, installs), in the order they happened.
/// On platforms with no events (everything but Android) the listener is
/// kept and never called.
#[must_use = "events stop when the subscription is dropped"]
pub fn events(listener: impl Fn(&Event) + 'static) -> EventSubscription {
    let (id, first) = LISTENERS.with(|l| {
        let mut l = l.borrow_mut();
        l.0 += 1;
        let id = l.0;
        let first = l.1.is_empty();
        l.1.push((id, Rc::new(listener)));
        (id, first)
    });
    if first {
        set_platform_listening(true);
    }
    EventSubscription { id }
}

/// Keeps an [`events`] listener registered; dropping it unregisters.
#[derive(Debug)]
pub struct EventSubscription {
    id: u64,
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        let now_empty = LISTENERS.with(|l| {
            let mut l = l.borrow_mut();
            let before = l.1.len();
            l.1.retain(|(id, _)| *id != self.id);
            before != l.1.len() && l.1.is_empty()
        });
        if now_empty {
            set_platform_listening(false);
        }
    }
}

/// Deliver a platform event to every listener. The list is cloned first, so
/// a listener may register or drop subscriptions while it runs.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn dispatch(json: &str) {
    let Some(event) = parse(json) else { return };
    let listeners: Vec<Listener> =
        LISTENERS.with(|l| l.borrow().1.iter().map(|(_, f)| f.clone()).collect());
    for listener in listeners {
        listener(&event);
    }
}

fn set_platform_listening(on: bool) {
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        // A failure here (no JVM context yet) leaves events queued on the
        // device; the next registration retries.
        let method = if on { "attach_events" } else { "detach_events" };
        if let Err(e) = crate::android::call(method, &serde_json::json!({})) {
            log::warn!("device_admin: {method} failed: {e}");
        }
    }
    #[cfg(not(all(not(target_arch = "wasm32"), target_os = "android")))]
    let _ = on;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn every_event_type_parses() {
        assert_eq!(
            parse(r#"{"type":"provisioning_complete","extras":{"server":"https://x","n":3}}"#),
            Some(Event::ProvisioningComplete {
                extras: serde_json::json!({ "server": "https://x", "n": 3 })
                    .as_object()
                    .unwrap()
                    .clone(),
            })
        );
        assert_eq!(parse(r#"{"type":"admin_enabled"}"#), Some(Event::AdminEnabled));
        assert_eq!(parse(r#"{"type":"admin_disabled"}"#), Some(Event::AdminDisabled));
        assert_eq!(
            parse(r#"{"type":"lock_task_entering","package":"com.k"}"#),
            Some(Event::LockTaskEntering { package: "com.k".into() })
        );
        assert_eq!(parse(r#"{"type":"lock_task_exiting"}"#), Some(Event::LockTaskExiting));
        assert!(matches!(
            parse(r#"{"type":"install_result","operation":"install","package":"com.k","status":0}"#),
            Some(Event::InstallResult(r)) if r.is_success()
        ));
        assert_eq!(
            parse(r#"{"type":"package_replaced","package":"com.k"}"#),
            Some(Event::PackageReplaced { package: "com.k".into() })
        );
        assert_eq!(parse(r#"{"type":"boot_completed"}"#), Some(Event::BootCompleted));
    }

    #[test]
    fn unknown_or_malformed_events_are_skipped() {
        assert_eq!(parse(r#"{"type":"from_the_future"}"#), None);
        assert_eq!(parse(r#"{"type":"lock_task_entering"}"#), None);
        assert_eq!(parse("{"), None);
    }

    #[test]
    fn listeners_receive_events_until_their_subscription_drops() {
        let seen = Rc::new(Cell::new(0));
        let a = {
            let seen = seen.clone();
            events(move |e| {
                if *e == Event::BootCompleted {
                    seen.set(seen.get() + 1);
                }
            })
        };
        let b = {
            let seen = seen.clone();
            events(move |_| seen.set(seen.get() + 10))
        };
        dispatch(r#"{"type":"boot_completed"}"#);
        assert_eq!(seen.get(), 11);
        drop(b);
        dispatch(r#"{"type":"boot_completed"}"#);
        assert_eq!(seen.get(), 12);
        drop(a);
        dispatch(r#"{"type":"boot_completed"}"#);
        assert_eq!(seen.get(), 12);
    }

    #[test]
    fn a_listener_may_unsubscribe_another_while_dispatching() {
        let held: Rc<RefCell<Option<EventSubscription>>> = Rc::new(RefCell::new(None));
        let ran = Rc::new(Cell::new(false));
        let first = {
            let held = held.clone();
            events(move |_| {
                held.borrow_mut().take();
            })
        };
        *held.borrow_mut() = Some({
            let ran = ran.clone();
            events(move |_| ran.set(true))
        });
        dispatch(r#"{"type":"admin_enabled"}"#);
        // The list was snapshotted, so the second listener still ran once.
        assert!(ran.get());
        assert!(held.borrow().is_none());
        drop(first);
    }
}
