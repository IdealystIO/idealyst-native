//! New-core boot entries (idea-lite migration). Behind the `new-core`
//! cargo feature.
//!
//! Same tablet window profile as [`crate::run`] / [`crate::run_at`],
//! but the mount goes through `host_winit::newcore::run_with` — World
//! + Registry + `realize` + the dispatch-site flush driver — instead
//! of `runtime_core::mount`. The winit surface/event path is shared
//! with the old boot (`host-winit`'s `run_impl`), so the only delta is
//! which core realizes the tree.
//!
//! Named seam: [`crate::run_runtime_server`] has NO new-core twin.
//! Native runtime-server shells render streamed wire commands from an
//! old-core dev host (the dev-chain entry in the idea-lite migration
//! log) — porting them is a dev-server workstream, not a variant one.

use std::rc::Rc;

use host_winit::{DeviceProfile, RunError};
use render_wgpu::newcore::{SceneElement, SceneRegistry};
use render_wgpu::{Painter, WgpuBackend};
use runtime_core::ColorScheme;

use crate::{HEIGHT, TITLE, WIDTH};

/// New-core sibling of [`crate::run`]: open the tablet-profile window
/// and mount `build()`'s scene tree on the new core.
pub fn run<F>(skin: Rc<dyn Painter>, build: F) -> Result<(), RunError>
where
    F: FnOnce() -> SceneElement + 'static,
{
    run_at(skin, None, build)
}

/// New-core sibling of [`crate::run_at`] — same window profile, plus a
/// screen-logical position for side-by-side harness layouts.
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
/// after `register_builtins` (the new-core registrar seam — apps/SDKs
/// add payload handlers before the tree realizes).
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
    host_winit::newcore::run_with(
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
