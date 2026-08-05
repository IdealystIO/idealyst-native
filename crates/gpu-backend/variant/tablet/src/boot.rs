//! Boot entries for the tablet variant — the tablet window profile
//! over `host_winit::run_with` (World + Registry + `realize` + the
//! dispatch-site flush driver).
//!
//! [`crate::run_runtime_server`] is a separate, core-agnostic path: it
//! replays an `idealyst dev` host's wire stream rather than mounting a
//! local tree, so it takes no `build` closure and shares nothing with
//! these entries beyond the window profile.

use std::rc::Rc;

use host_winit::{DeviceProfile, RunError};
use render_wgpu::newcore::{SceneElement, SceneRegistry};
use render_wgpu::{Painter, WgpuBackend};
use runtime_shared::ColorScheme;

use crate::{HEIGHT, TITLE, WIDTH};

/// Open the tablet-profile window and mount `build()`'s scene tree.
pub fn run<F>(skin: Rc<dyn Painter>, build: F) -> Result<(), RunError>
where
    F: FnOnce() -> SceneElement + 'static,
{
    run_at(skin, None, build)
}

/// Like [`run`], plus a screen-logical position for side-by-side
/// harness layouts.
pub fn run_at<F>(
    skin: Rc<dyn Painter>,
    position: Option<(i32, i32)>,
    build: F,
) -> Result<(), RunError>
where
    F: FnOnce() -> SceneElement + 'static,
{
    run_with(skin, position, |_| {}, build)
}

/// Like [`run_at`], but invokes `register` with the scene registry
/// after `register_builtins` — apps/SDKs add payload handlers before
/// the tree realizes.
pub fn run_with<R, F>(
    skin: Rc<dyn Painter>,
    position: Option<(i32, i32)>,
    register: R,
    build: F,
) -> Result<(), RunError>
where
    R: FnOnce(&mut SceneRegistry<WgpuBackend>) + 'static,
    F: FnOnce() -> SceneElement + 'static,
{
    host_winit::run_with(
        DeviceProfile {
            logical_size: (WIDTH, HEIGHT),
            position,
            title: TITLE.to_string(),
            color_scheme: ColorScheme::Auto,
        },
        skin,
        register,
        build,
    )
}
