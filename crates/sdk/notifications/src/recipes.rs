//! Compile-checked usage **recipes** for the notifications SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to
//! use the SDK. Because the fn compiles against the live API, a signature
//! change that isn't reflected here is a compile error (whenever the catalog
//! is built), so these examples can't silently rot — and the MCP / docs
//! surface them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing — the recipes, and the imports
//! inside them, don't compile at all. So there's no `#[cfg]` here and no cost
//! in shipped apps. Recipes are self-contained (imports live inside each fn)
//! so the captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    Notification,
    /// A "Notify" button that asks for permission, then posts a local
    /// notification. Press handlers can't be `async`, so the work runs on
    /// [`spawn_async`](runtime_core::driver::spawn_async): call
    /// [`authorize`](crate::authorize) first (the OS prompt, via the shared
    /// `permissions` crate), and only [`notify`](crate::notify) a
    /// [`Notification`](crate::Notification) when it's granted. The outcome is
    /// written into a `signal` the `text` reads reactively, so the label
    /// updates in place.
    pub fn notify_button() -> ::runtime_core::Element {
        use crate::{authorize, notify, Notification};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, text, ui};

        // Reactive status line; the press handler writes into it.
        let status = signal!("Tap to notify".to_string());

        let on_click = move || {
            spawn_async(async move {
                if authorize().await.is_granted() {
                    match notify(Notification::new("Hi", "From the recipe")).await {
                        Ok(id) => status.set(format!("Posted: {id}")),
                        Err(e) => status.set(format!("Failed: {e}")),
                    }
                } else {
                    status.set("Permission denied".to_string());
                }
            });
        };

        ui! {
            view {
                button(label = "Notify", on_click = on_click)
                text(move || status.get())
            }
        }
    }
);

recipe!(
    Notification,
    /// Schedule a local notification to fire after a delay, and give it a
    /// stable `id` so a later post with the same id replaces it and
    /// [`cancel`](crate::cancel) can take it back. `schedule` is the simple
    /// delay primitive (richer recurrence is a later layer); it's
    /// [`NotSupported`](crate::NotifyError::NotSupported) on web, which has no
    /// native delayed-delivery API.
    pub fn schedule_with_id() -> ::runtime_core::Element {
        use crate::{authorize, cancel, schedule, Notification, NotificationId};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, text, ui};
        use ::std::time::Duration;

        let status = signal!("idle".to_string());
        // A stable id lets us replace or cancel this exact reminder later.
        let id = NotificationId::from("reminder");

        let arm = {
            let id = id.clone();
            move || {
                let id = id.clone();
                spawn_async(async move {
                    if authorize().await.is_granted() {
                        let note = Notification::new("Reminder", "Stand up and stretch")
                            .id(id)
                            .with("route", "/breaks");
                        match schedule(note, Duration::from_secs(60)).await {
                            Ok(_) => status.set("scheduled in 60s".to_string()),
                            Err(e) => status.set(format!("not scheduled: {e}")),
                        }
                    }
                });
            }
        };

        let cancel_it = move || {
            let id = id.clone();
            spawn_async(async move {
                cancel(&id).await;
                status.set("cancelled".to_string());
            });
        };

        ui! {
            view {
                button(label = "Remind me", on_click = arm)
                button(label = "Cancel", on_click = cancel_it)
                text(move || status.get())
            }
        }
    }
);
