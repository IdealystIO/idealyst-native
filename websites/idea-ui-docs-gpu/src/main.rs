//! Mounts `idea-ui-docs` on the wgpu (GPU) backend as a native desktop
//! window — the AppKit backend's job, done entirely on the GPU.
//!
//! The pieces this exercises (all newly wired for the universal-native
//! GPU path):
//!   - [`NativeSkin`] reports a real host-OS [`Platform`] (here `MacOs`),
//!     giving the app a genuine desktop identity (window as chrome, no
//!     bezel) rather than a phone-simulator skin.
//!   - [`host_winit::run_with`] registers the swap navigator's ONE
//!     backend-neutral handler on the `WgpuBackend` before mount, via
//!     the `RegisterNavigator` impl (`swap_navigator::register_generic`).
//!     The chrome (idea-ui-nav `AppShell` + header) is plain author
//!     layout built from real `StyleRules`, so the GPU backend (no CSS)
//!     renders the pinned sidebar + body correctly. Without registration
//!     the navigator leaf would hit the "not registered" panic.
//!
//! `table` (the docs PropsTable) needs no registration here: its native
//! path lowers to primitives, not `Element::External`.

use std::rc::Rc;

use host_winit::{run_with, DeviceProfile};
use render_wgpu::{NativeSkin, WgpuBackend};
use runtime_core::{ColorScheme, Platform};

use idea_ui_docs::app;

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

    // Register the SwapNavigator's backend-neutral handler on the wgpu
    // backend before the app tree mounts, through the generic
    // `RegisterNavigator` path. (The per-target `register` fns +
    // inventory self-registration only cover web/macOS/iOS/Android; the
    // GPU backend uses the same generic path as SSR.)
    let register = |backend: &mut WgpuBackend| {
        swap_navigator::register_generic(backend);
    };

    if let Err(e) = run_with(profile, skin, register, app) {
        eprintln!("[idea-ui-docs-gpu] fatal: {e}");
        std::process::exit(1);
    }
}
