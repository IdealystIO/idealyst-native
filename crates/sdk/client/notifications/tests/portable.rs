//! Portable host tests for the `notifications` public surface.
//!
//! The pure-logic tests (builder, id types) run on **every** host. The
//! no-op behavioral tests (`notify` / `schedule` / `cancel` / `push_token` /
//! `authorize`) run only where the **fallback** `imp` compiles — desktop
//! Windows / a CI runner — because the macOS `apple` backend messages the
//! real `UNUserNotificationCenter`, which throws in a non-bundled test
//! binary. Linux has its own `linux` mod below: its D-Bus backend is real,
//! so the always-run tests there assert only the bus-free contract
//! (`schedule`/`push_token` unsupported, `authorize` usable) and the live
//! post is an `#[ignore]`d test. The other native backends are
//! compile-checked only (see the README).

use notifications::{Notification, NotificationId};

#[test]
fn builder_is_ergonomic_and_chainable() {
    let n = Notification::new("Title", "Body")
        .subtitle("Sub")
        .id("note-42")
        .with("k", "v");
    assert_eq!(n.title, "Title");
    assert_eq!(n.body, "Body");
    assert_eq!(n.subtitle.as_deref(), Some("Sub"));
    assert_eq!(n.id, Some(NotificationId::from("note-42")));
    assert_eq!(n.data.get("k").map(String::as_str), Some("v"));
}

#[test]
fn notification_id_string_round_trips() {
    assert_eq!(NotificationId::from("x").as_str(), "x");
    assert_eq!(NotificationId::from("y").to_string(), "y");
}

// Behavioral tests only where the fallback `imp` is in play (not the real
// platform notification centers, and not Linux's real D-Bus backend).
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android",
    target_os = "linux"
)))]
mod fallback {
    use notifications::{
        authorize, cancel, cancel_all, notify, push_token, schedule, Notification, NotificationId,
        NotifyError,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn notify_returns_explicit_id() {
    let id = notify(Notification::new("Hi", "there").id("greeting"))
        .await
        .unwrap();
    assert_eq!(id, NotificationId::from("greeting"));
}

#[tokio::test]
async fn notify_generates_id_when_absent() {
    let a = notify(Notification::new("a", "b")).await.unwrap();
    let b = notify(Notification::new("a", "b")).await.unwrap();
    // Distinct generated ids for two id-less posts.
    assert_ne!(a, b);
    assert!(a.as_str().starts_with("idealyst-"));
}

#[tokio::test]
async fn schedule_returns_an_id_on_host() {
    // The host fallback treats schedule as a no-op success carrying the
    // resolved id; native backends map the delay to a platform trigger.
    let id = schedule(Notification::new("Soon", "delayed").id("later"), Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(id, NotificationId::from("later"));
}

#[tokio::test]
async fn cancel_paths_do_not_panic() {
    let id = notify(Notification::new("x", "y").id("cancelme"))
        .await
        .unwrap();
    cancel(&id).await;
    cancel_all().await;
}

#[tokio::test]
async fn push_token_is_a_seam_on_host() {
    // No remote-push wiring on the host → the documented seam reports
    // NotSupported rather than fabricating a token.
    assert_eq!(push_token().await, Err(NotifyError::NotSupported));
}

#[tokio::test]
async fn authorize_is_usable_on_host() {
    // No native permission model on the host → Unsupported, which is usable
    // (never blocks the caller). The real prompt fires on device via the
    // `permissions` crate.
    assert!(authorize().await.is_usable());
}
}

// Linux D-Bus backend. The always-run tests here touch no bus: `schedule` and
// `push_token` short-circuit to `NotSupported`, and `authorize` goes through
// the `permissions` fallback (also bus-free). The live `Notify` round-trip is
// `#[ignore]`d because it needs a session bus with an
// `org.freedesktop.Notifications` owner (a desktop session), absent on a
// headless CI runner.
#[cfg(target_os = "linux")]
mod linux {
    use notifications::{
        authorize, cancel, notify, push_token, schedule, Notification, NotificationId, NotifyError,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn schedule_is_unsupported_no_native_trigger() {
        // The freedesktop spec has no post-later trigger, so `schedule`
        // reports NotSupported rather than faking a process-lifetime timer.
        let r = schedule(
            Notification::new("Soon", "delayed").id("later"),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(r, Err(NotifyError::NotSupported));
    }

    #[tokio::test]
    async fn push_token_is_a_seam() {
        // No remote-push token model on a Linux desktop.
        assert_eq!(push_token().await, Err(NotifyError::NotSupported));
    }

    #[tokio::test]
    async fn authorize_is_usable() {
        assert!(authorize().await.is_usable());
    }

    /// Live post + cancel against a running notification daemon. Ignored: it
    /// needs a session bus with an `org.freedesktop.Notifications` owner. Run
    /// on a desktop with `cargo test -p notifications --test portable --
    /// --ignored`.
    #[ignore = "needs a live session bus + notification daemon"]
    #[tokio::test]
    async fn live_notify_and_cancel() {
        let id = notify(Notification::new("Idealyst", "portable live test").id("idealyst-portable"))
            .await
            .expect("notify should reach the daemon");
        assert_eq!(id, NotificationId::from("idealyst-portable"));
        // Re-post under the same id must replace, not stack.
        let again = notify(Notification::new("Idealyst", "replaced").id("idealyst-portable"))
            .await
            .expect("re-notify should reach the daemon");
        assert_eq!(again, id);
        cancel(&id).await;
    }
}
