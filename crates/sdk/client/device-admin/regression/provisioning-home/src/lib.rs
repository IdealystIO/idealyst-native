//! The provisioning-home probe: logs every `device_admin` event to logcat
//! as `[provisioning-home] <event>` so `run.sh` can see that the
//! provisioning event reached the app on its first start.

use runtime_core::{on_scope_drop, text, ui, Element};

pub fn app() -> Element {
    let subscription = device_admin::events(|event| {
        log::info!("[provisioning-home] {event:?}");
    });
    on_scope_drop(move || drop(subscription));
    ui! {
        view {
            text("Provisioning home probe".to_string())
        }
    }
}

/// Android entry: the generated wrapper's `attach` mounts this.
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam; nothing to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
