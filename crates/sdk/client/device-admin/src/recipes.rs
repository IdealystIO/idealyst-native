//! Compile-checked usage **recipes** for the device-admin SDK.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing. Recipes are self-contained
//! (imports live inside each fn) so the captured source reads as a complete,
//! copy-pasteable example.

use runtime_core::recipe;

recipe!(
    start_lock_task,
    /// A kiosk screen that locks itself on screen when it mounts and shows
    /// whether that worked. On Android the device-owner agent must first
    /// allow-list this package with `set_lock_task_packages`; then
    /// `start_lock_task()` resolves `true` and locks silently. Otherwise it
    /// resolves `false` (Android asks the user to pin the app; iOS does
    /// nothing unless the MDM allows Single App Mode). The unlock button is
    /// where an app puts its own admin check before calling
    /// `stop_lock_task()`.
    pub fn kiosk_lock_on_mount() -> ::runtime_core::Element {
        use crate::{start_lock_task, stop_lock_task};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, text, ui, Signal};

        let status: Signal<String> = signal("Locking…".to_string());
        spawn_async(async move {
            status.set(match start_lock_task().await {
                Ok(true) => "Locked".to_string(),
                Ok(false) => "Not allow-listed: lock task needs the device owner".to_string(),
                Err(e) => format!("Lock task unavailable: {e}"),
            });
        });

        let unlock = move || {
            // Gate this behind the app's own admin check.
            spawn_async(async move {
                if let Ok(true) = stop_lock_task().await {
                    status.set("Unlocked".to_string());
                }
            });
        };

        ui! {
            view() {
                text(move || status.get())
                button(label = "Admin: unlock".to_string(), on_click = unlock)
            }
        }
    }
);

recipe!(
    events,
    /// The device-owner agent's event wiring: the provisioning extras (the
    /// server URL and enrolment token from the QR code) arrive once as
    /// `ProvisioningComplete`, install outcomes as `InstallResult`, and a
    /// reboot as `BootCompleted`. Events raised while nothing listened are
    /// replayed on the first registration. The subscription lives as long as
    /// the component: `on_scope_drop` drops it at unmount — mount this at the
    /// app root to listen for the app's lifetime.
    pub fn agent_event_log() -> ::runtime_core::Element {
        use crate::{events, Event};
        use ::runtime_core::{on_scope_drop, signal, text, ui, Signal};

        let last: Signal<String> = signal("No events yet".to_string());
        let subscription = events(move |event| {
            last.set(match event {
                Event::ProvisioningComplete { extras } => {
                    format!("Provisioned; server = {:?}", extras.get("server"))
                }
                Event::InstallResult(result) => format!("Install: {result:?}"),
                Event::BootCompleted => "Rebooted".to_string(),
                other => format!("{other:?}"),
            });
        });
        on_scope_drop(move || drop(subscription));

        ui! {
            view() {
                text(move || last.get())
            }
        }
    }
);
