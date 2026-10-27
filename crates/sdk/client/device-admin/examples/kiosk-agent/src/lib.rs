//! `kiosk-agent` — the device-owner half of a kiosk tablet, exercising the
//! `device-admin` SDK end to end.
//!
//! Make it device owner on a freshly wiped emulator (no accounts):
//!
//! ```text
//! adb shell dpm set-device-owner ai.truday.idealyst.kioskagent/io.idealyst.deviceadmin.AdminReceiver
//! ```
//!
//! Then:
//! - **Lock down** allow-lists `kiosk-app` (and this app) for lock task with
//!   no system UI, disables the keyguard and status bar, makes the kiosk the
//!   persistent home screen, blocks safe boot, and launches the kiosk.
//! - **Install update** installs `kiosk-update.apk` from this app's files
//!   dir, checking it against the certificate SHA-256 in
//!   `kiosk-update.sha256` next to it, then relaunches the kiosk. A real
//!   agent downloads both from its server; on an emulator, stand that in as
//!   root (the app cannot read files `adb push` leaves on shared storage):
//!
//!   ```text
//!   adb root
//!   adb push kiosk-update.apk kiosk-update.sha256 /data/data/ai.truday.idealyst.kioskagent/files/
//!   adb shell chown -R $(adb shell stat -c %u /data/data/ai.truday.idealyst.kioskagent) \
//!       /data/data/ai.truday.idealyst.kioskagent/files
//!   ```
//! - **Release** undoes the lock-down and relinquishes device owner.
//!
//! Every SDK event is appended to the log on screen.

use runtime_core::driver::spawn_async;
use runtime_core::{on_scope_drop, signal, text, ui, Element, Signal};

const KIOSK: &str = "ai.truday.idealyst.kioskapp";
const AGENT: &str = "ai.truday.idealyst.kioskagent";
const UPDATE_DIR: &str = "/data/data/ai.truday.idealyst.kioskagent/files";

/// Run `steps` in order, stopping at the first failure, and describe the
/// outcome.
fn run_steps(steps: &[(&str, &dyn Fn() -> device_admin::Result<()>)]) -> String {
    for (name, step) in steps {
        if let Err(e) = step() {
            return format!("{name} failed: {e}");
        }
    }
    "done".to_string()
}

fn lock_down() -> String {
    use device_admin::*;
    run_steps(&[
        ("set_lock_task_packages", &|| set_lock_task_packages(&[KIOSK, AGENT])),
        ("set_lock_task_features", &|| set_lock_task_features(LockTaskFeatures::NONE)),
        ("set_keyguard_disabled", &|| set_keyguard_disabled(true)),
        ("set_status_bar_disabled", &|| set_status_bar_disabled(true)),
        ("set_home_activity", &|| set_home_activity(KIOSK, ".MainActivity")),
        ("add_user_restriction", &|| add_user_restriction(restriction::DISALLOW_SAFE_BOOT)),
        ("launch", &|| launch(KIOSK)),
    ])
}

fn release() -> String {
    use device_admin::*;
    run_steps(&[
        ("clear_home_activity", &|| clear_home_activity(KIOSK)),
        ("clear_user_restriction", &|| clear_user_restriction(restriction::DISALLOW_SAFE_BOOT)),
        ("set_status_bar_disabled", &|| set_status_bar_disabled(false)),
        ("set_keyguard_disabled", &|| set_keyguard_disabled(false)),
        ("set_lock_task_packages", &|| set_lock_task_packages(&[])),
        ("relinquish_device_owner", &relinquish_device_owner),
    ])
}

fn owner_label() -> String {
    match device_admin::is_device_owner() {
        Ok(true) => "Device owner: yes".to_string(),
        Ok(false) => "Device owner: no".to_string(),
        Err(e) => format!("Device owner: {e}"),
    }
}

pub fn app() -> Element {
    let status: Signal<String> = signal(owner_label());
    let log: Signal<Vec<String>> = signal(Vec::new());

    let subscription = device_admin::events(move |event| {
        let mut lines = log.get();
        lines.push(format!("{event:?}"));
        let excess = lines.len().saturating_sub(8);
        lines.drain(..excess);
        log.set(lines);
    });
    on_scope_drop(move || drop(subscription));

    let on_lock = move || status.set(format!("Lock down: {}", lock_down()));
    let on_install = move || {
        let cert = match std::fs::read_to_string(format!("{UPDATE_DIR}/kiosk-update.sha256")) {
            Ok(c) => c.trim().to_string(),
            Err(e) => return status.set(format!("Install: no kiosk-update.sha256 ({e})")),
        };
        status.set("Installing…".to_string());
        spawn_async(async move {
            let apk = format!("{UPDATE_DIR}/kiosk-update.apk");
            status.set(match device_admin::install_apk(&apk, &cert).await {
                Ok(result) if result.is_success() => {
                    let relaunch = device_admin::launch(KIOSK).err();
                    format!("Install: {result:?}; relaunch error: {relaunch:?}")
                }
                Ok(result) => format!("Install: {result:?}"),
                Err(e) => format!("Install refused: {e}"),
            });
        });
    };
    let on_release = move || status.set(format!("Release: {}; {}", release(), owner_label()));

    ui! {
        view {
            text(move || status.get())
            button(label = "Lock down".to_string(), on_click = on_lock)
            button(label = "Install update".to_string(), on_click = on_install)
            button(label = "Release".to_string(), on_click = on_release)
            text(move || log.get().join("\n"))
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
