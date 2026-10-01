//! The spike's built bundle and the host components it imports.

use runtime_scene::Element;
use runtime_vocabulary::builders::{text, view};
use runtime_world::Signal;
use stream_host::{HostExports, HostProps};

pub mod fetch;
pub mod full;
pub mod guest_build;

/// `spike-fullguest` (model B: the real framework inside the bundle),
/// release-built for wasm32 by this crate's build script.
pub const FULL_GUEST_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spike_fullguest.wasm"));

/// `spike-guest`, release-built for wasm32 by this crate's build script —
/// the app's built-in copy, used when no bundle server answers.
pub const GUEST_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spike_guest.wasm"));

/// What this "app binary" exports to bundles:
/// - host components the bundle mounts by name: `Badge`, and
///   `CameraPreview` (a stand-in for the native preview view — frames would
///   stay native, the bundle only places it);
/// - the host functions bundles may call. This list is the allowlist.
pub fn host_exports() -> HostExports {
    HostExports::new()
        .export("Badge", |p| -> Element {
            let label = p.string();
            view().child(text().content(format!("[{label}]"))).build()
        })
        .export("CameraPreview", |p| -> Element {
            let facing = p.string();
            view().child(text().content(format!("▣ native {facing} camera preview"))).build()
        })
        .host_fn(spike_camera::battery_level::export())
        .host_fn(spike_camera::take_photo::export())
}

/// The props the demo app mounts each streamed component with: the app's
/// side of the prop contract. A fetched bundle must accept exactly these
/// before it may replace the running one.
pub fn demo_props(external: Signal<i64>, shared: Signal<i64>) -> Vec<(&'static str, HostProps)> {
    vec![
        (
            "Counter",
            HostProps::new().value("title", "Hello from wasm".to_string()).read_signal("external", external.read_only()),
        ),
        ("Stepper", HostProps::new().signal("value", shared)),
        ("Camera", HostProps::new().value("facing", "back".to_string())),
    ]
}
