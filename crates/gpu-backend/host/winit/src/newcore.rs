//! New-core windowed boot (idea-lite migration, P5). Behind the
//! `new-core` cargo feature.
//!
//! [`run`] is the new-core sibling of [`crate::run`]: identical winit
//! event-loop / window / wgpu-surface boot (both route through
//! `app.rs::run_impl`, so there is exactly ONE surface/event path for
//! both cores — no mechanical drift possible), but the mount goes
//! through `render_wgpu::newcore::start` — World + Registry + `realize`
//! + `Backend::finish` + post-mount flush — instead of
//! `runtime_core::mount`. The interaction [`render_wgpu::Host`] is
//! constructed the same way (it owns the backend + hit-testing); the
//! new-core caps impls wrap every author callback at install time, so
//! the Host's pointer/key/keyboard dispatch drives the flush with zero
//! Host edits.
//!
//! Mount buffering: this host has NO buffering window (the winit
//! scheduler defers microtasks as 0 ms timers onto the event loop —
//! the old GPU boot behaved identically), so `start`'s pre-`finish`
//! drain is a no-op here; deferred build microtasks land on the first
//! event-loop turns after `resumed`, exactly like the old mount.
//! Compare `host_appkit::newcore::run`, whose AppKit scheduler does
//! buffer.

use std::rc::Rc;

use render_wgpu::newcore::{SceneElement, SceneRegistry};
use render_wgpu::{Painter, WgpuBackend};

use crate::app::run_impl;
use crate::{DeviceProfile, RunError};

/// Boot the winit event loop, open the host window + wgpu surface,
/// mount `build()`'s tree on the NEW core, and run until the user
/// closes the window. Returns only on event-loop failure (window close
/// exits the process, same as [`crate::run`]).
///
/// Equivalent to [`run_with`] with a no-op registry seam.
pub fn run<F>(
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build: F,
) -> Result<(), RunError>
where
    F: FnOnce() -> SceneElement + 'static,
{
    run_with(profile, skin, |_| {}, build)
}

/// Like [`run`], but invokes `register` with the scene registry after
/// `register_builtins`, so apps/SDKs can add their own payload handlers
/// before the tree realizes (the new-core counterpart of the old
/// `run_with`'s backend-registrar seam).
pub fn run_with<R, F>(
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    register: R,
    build: F,
) -> Result<(), RunError>
where
    R: FnOnce(&mut SceneRegistry<WgpuBackend>) + 'static,
    F: FnOnce() -> SceneElement + 'static,
{
    run_impl(
        profile,
        skin,
        Box::new(move |host: &mut render_wgpu::Host| {
            // `start` installs the flush driver (dispatch hook +
            // FLUSH_WORLD) and returns the retained app. Same
            // retention as the old boot's `forget(owner)` — the app
            // lives until the process exits with the event loop
            // (window close calls `process::exit`).
            let app = render_wgpu::newcore::start(host.backend().clone(), register, build);
            std::mem::forget(app);
        }),
    )
}
