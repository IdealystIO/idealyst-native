//! Portable host tests for the `notifications` public surface.
//!
//! The pure-logic tests (builder, id types) run on **every** host. The
//! behavioral tests (`notify` / `schedule` / `cancel` / `push_token` /
//! `authorize`) run only where the **fallback** `imp` compiles — desktop
//! Windows / Linux / a CI runner — because the macOS `apple` backend
//! messages the real `UNUserNotificationCenter`, which throws in a
//! non-bundled test binary. The native backends are compile-checked only
//! (see the README).

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
// platform notification centers).
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
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
