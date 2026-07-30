//! iOS runtime-server shell entry points.
//!
//! Defines the `#[no_mangle] ios_main` / `ios_teardown` C symbols the
//! Swift host calls, delegating to `backend_ios::ios_main_with_register`
//! / `ios_teardown_impl`. The shell is the sole definer of those symbols
//! (the backend's own copies are gated off behind `entry-symbols`), which
//! is the whole reason the crate exists as a separate staticlib.
//!
//! On non-iOS hosts this is an empty crate (the entry symbols are
//! `cfg(target_os = "ios")`-gated) so the workspace still type-checks
//! during cross-compile of other targets.
//!
//! # Why this shell no longer registers SDK handlers
//!
//! It used to bundle the first-party SDKs (swap/stack navigators,
//! codeblock, table) and call each one's `register(&mut IosBackend)` so
//! their native chrome rendered over the wire on device. That was the
//! old-core `Element::External` model: an unrecognised element crossed
//! the wire as a `CreateExternal` payload and the CLIENT rebuilt it from
//! its per-backend `ExternalRegistry`.
//!
//! Runtime v2 has no such registry. An SDK primitive is a scene payload
//! with a handler installed on a `runtime_scene::Registry<H>`, and in
//! runtime-server mode the scene is realized on the **host**, against
//! `Registry<WireRecordingBackend>` — the dev-server sidecar's
//! `register_scene_extensions_recorder` seam (see
//! `dev_server::sidecar::run_newcore`). The handler therefore runs
//! server-side and what crosses the wire is already ordinary primitive
//! commands. There is nothing left for the client to register, so the
//! registration is deleted rather than ported, and the SDK dependencies
//! are gone with it.
//!
//! **Behavior delta worth knowing** (runtime-server / `idealyst dev`
//! only, not device builds): an SDK whose `register` type-dispatches to a
//! backend-CONCRETE handler — `codeblock` picks a native
//! `UITextView`-backed mount on `Registry<IosBackend>` — cannot take that
//! branch here, because the registry it sees is the recorder's. Such a
//! primitive renders its portable variant during hot-reload and its
//! native variant in a real build. A locally-mounted device build
//! (`idealyst run --ios`) is unaffected: there the app registers on the
//! real `Registry<IosBackend>` through the generated wrapper's
//! `register_scene_extensions` seam.

#![cfg(target_os = "ios")]

use std::ffi::{c_char, c_void};

/// C-exported entry the Swift host calls from `viewDidLoad`. Same ABI
/// as `backend_ios::ios_main` (root view + dev-endpoint
/// C-string) — `run-ios` repoints the linked staticlib here without
/// touching the Swift glue or bridging header.
///
/// # Safety
/// Same contract as `backend_ios::ios_main_with_register`.
#[no_mangle]
pub unsafe extern "C" fn ios_main(root_view: *mut c_void, endpoint_utf8: *const c_char) {
    // Empty registration: see the module docs — SDK scene handlers are
    // installed on the HOST's recorder registry in runtime-server mode.
    unsafe { backend_ios::ios_main_with_register(root_view, endpoint_utf8, |_backend| {}) }
}

/// C-exported teardown. Delegates to the backend's implementation.
/// This crate is the sole definer of the `ios_teardown` C symbol (the
/// backend's `#[no_mangle]` version is gated off via `entry-symbols`),
/// so there's no duplicate-symbol clash at the swiftc link step.
///
/// # Safety
/// Same contract as `backend_ios::ios_teardown_impl`.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    unsafe { backend_ios::ios_teardown_impl() }
}
