//! Mounts `idea-ui-docs` on the wgpu (GPU) backend as a native desktop
//! window — the AppKit backend's job, done entirely on the GPU.
//!
//! The pieces this exercises (all newly wired for the universal-native
//! GPU path):
//!   - [`NativeSkin`] reports a real host-OS [`Platform`] (here `MacOs`),
//!     so idea-ui-docs takes its desktop custom-header + pinned-sidebar
//!     branch — exactly what `backend-macos` renders — rather than the
//!     mobile native-header branch a sim skin would trigger.
//!   - [`host_winit::run_with`] registers the drawer navigator's
//!     backend-neutral **desktop** handler on the `WgpuBackend` before
//!     mount, via the new `RegisterNavigator` impl. `register_native`
//!     selects the desktop (persistent-sidebar) handler — which lays out
//!     with real `StyleRules`, so the GPU backend (no CSS) renders the
//!     pinned sidebar + body correctly. Without registration the
//!     navigator leaf would hit the "not registered" panic.
//!
//! `table` (the docs PropsTable) needs no registration here: its native
//! path lowers to primitives, not `Element::External`.

use std::rc::Rc;

use host_winit::{run_with, DeviceProfile};
use render_wgpu::{NativeSkin, WgpuBackend};
use runtime_core::{ColorScheme, Platform};

use idea_ui_docs::app;

fn main() {
    // Desktop-sized window. idea-ui-docs pins its sidebar at ≥900px
    // (`install_navigator_pin_width(900.0)`), so 1280 wide lands firmly in
    // the pinned-sidebar layout.
    let profile = DeviceProfile {
        logical_size: (1280, 832),
        position: None,
        title: "idea-ui Docs — GPU (wgpu)".to_string(),
        color_scheme: ColorScheme::Auto,
    };

    // A real native desktop identity — NOT a phone simulator. The window
    // is the chrome; the skin draws no bezel.
    let skin = Rc::new(NativeSkin::new(Platform::MacOs));

    // Register the DrawerNavigator's backend-neutral desktop handler on
    // the wgpu backend before the app tree mounts. Form factor is the
    // compile-time `idealyst_form` cfg (unset → desktop here).
    let register = |backend: &mut WgpuBackend| {
        drawer_navigator::register_native(backend);
    };

    if let Err(e) = run_with(profile, skin, register, app) {
        eprintln!("[idea-ui-docs-gpu] fatal: {e}");
        std::process::exit(1);
    }
}
