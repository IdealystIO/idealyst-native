//! Mounts `idea-ui-docs` on the wgpu (GPU) backend as a native desktop
//! window — the AppKit backend's job, done entirely on the GPU.
//!
//! The pieces this exercises:
//!   - [`NativeSkin`] reports a real host-OS [`Platform`] (here `MacOs`),
//!     giving the app a genuine desktop identity (window as chrome, no
//!     bezel) rather than a phone-simulator skin.
//!   - [`host_winit::newcore::run_with`] mounts the tree through
//!     `render_wgpu::newcore::start` (World + Registry + `realize` +
//!     flush driver) and hands the app's `register_scene_extensions`
//!     seam the fresh registry after `register_builtins`. That seam is
//!     what puts the `codeblock` and `table` payload handlers on the GPU
//!     registry — an unregistered payload panics at realize.
//!   - Navigators need no registration: the swap navigator is a
//!     vocabulary built-in installed by `register_builtins`, so ONE
//!     handler serves the GPU backend like every other host. (This used
//!     to be the crate's whole point — `swap_navigator::register_generic`
//!     on a `RegisterNavigator` impl for `WgpuBackend`; that per-backend
//!     navigator registry no longer exists.)
//!
//! The chrome (idea-ui-nav `AppShell` + header) is plain author layout
//! built from real `StyleRules`, so the GPU backend (no CSS) renders the
//! pinned sidebar + body correctly.

use std::rc::Rc;

use host_winit::DeviceProfile;
use render_wgpu::NativeSkin;
use runtime_shared::{ColorScheme, Platform};

use idea_ui_docs::{app, register_scene_extensions};

fn main() {
    // Desktop-sized window. idea-ui-docs pins its AppShell sidebar at
    // ≥900px (`install_breakpoints(Breakpoints { lg_min: 900.0, .. })`),
    // so 1280 wide lands firmly in the pinned-sidebar layout.
    let profile = DeviceProfile {
        logical_size: (1280, 832),
        position: None,
        title: "idea-ui Docs — GPU (wgpu)".to_string(),
        color_scheme: ColorScheme::Auto,
    };

    // A real native desktop identity — NOT a phone simulator. The window
    // is the chrome; the skin draws no bezel.
    let skin = Rc::new(NativeSkin::new(Platform::MacOs));

    if let Err(e) = host_winit::newcore::run_with(profile, skin, register_scene_extensions, app) {
        eprintln!("[idea-ui-docs-gpu] fatal: {e}");
        std::process::exit(1);
    }
}
