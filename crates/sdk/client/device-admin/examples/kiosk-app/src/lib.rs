//! `kiosk-app` — the app the `kiosk-agent` locks onto the screen.
//!
//! On mount it calls `device_admin::start_lock_task()`: once the agent has
//! allow-listed it, that locks silently (`Ok(true)`); before, Android asks
//! the user to pin the app (`Ok(false)`). Its manifest declares
//! `lock_task_mode = "if_whitelisted"` and `home_launcher = true`
//! (Cargo.toml), so it also enters lock task when launched and can be made
//! the persistent home screen. "Admin exit" stands in for the admin-password
//! flow a real kiosk puts in front of `stop_lock_task()`.

use runtime_core::driver::spawn_async;
use runtime_core::{signal, text, ui, Element, Signal};

/// The installed versionCode, shown so an update is visible on screen.
fn version_label() -> String {
    match device_admin::installed_version("ai.truday.idealyst.kioskapp") {
        Ok(Some(v)) => format!("Kiosk {} (versionCode {})", v.name.unwrap_or_default(), v.code),
        Ok(None) => "Kiosk (not installed?)".to_string(),
        Err(e) => format!("Kiosk ({e})"),
    }
}

pub fn app() -> Element {
    let status: Signal<String> = signal("Locking…".to_string());
    spawn_async(async move {
        status.set(match device_admin::start_lock_task().await {
            Ok(true) => "Locked in by the device owner".to_string(),
            Ok(false) => "Not allow-listed: Android asked to pin the app".to_string(),
            Err(e) => format!("Lock task failed: {e}"),
        });
    });

    let exit = move || {
        spawn_async(async move {
            status.set(match device_admin::stop_lock_task().await {
                Ok(_) => "Unlocked by admin".to_string(),
                Err(e) => format!("Unlock failed: {e}"),
            });
        });
    };

    ui! {
        view {
            text(version_label())
            text(move || status.get())
            button(label = "Admin exit".to_string(), on_click = exit)
        }
    }
}

/// Android entry: the generated wrapper's `attach` mounts this.
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. `device-admin` is a capability crate, not
/// a rendered primitive, so there is nothing to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
