//! iOS runtime-server shell entry points.
//!
//! Defines the `#[no_mangle] ios_main` / `ios_teardown` C symbols the
//! Swift host calls. `ios_main` here delegates to
//! `backend_ios::ios_main_with_register`, passing a closure that
//! registers the first-party SDK handlers (Drawer navigator, code
//! block, table) on the RS client backend. That's what lets native
//! SDK chrome render over the wire on device: the backend staticlib
//! itself is SDK-free (the SDKs depend on it), so the SDK set is
//! compiled in here, fixed at build time — the mobile analogue of the
//! per-app web wrapper's `register_extensions`.
//!
//! On non-iOS hosts this is an empty crate (the SDK deps + entry
//! symbols are `cfg(target_os = "ios")`-gated) so the workspace still
//! type-checks during cross-compile of other targets.

#![cfg(target_os = "ios")]

use std::ffi::{c_char, c_void};

use backend_ios::IosBackend;

/// Register the compiled-in first-party SDK handlers on the RS client
/// backend. Mirrors `examples/*/src/lib.rs::register_extensions` for
/// the iOS target. Adding a first-party SDK to the over-the-wire
/// native client = adding its `register` call here.
fn register_first_party_sdks(backend: &mut IosBackend) {
    drawer_navigator::register(backend);
    idea_codeblock::register(backend);
    table::register(backend);
}

/// C-exported entry the Swift host calls from `viewDidLoad`. Same ABI
/// as `backend_ios::ios_main` (root view + dev-endpoint
/// C-string) — `run-ios` repoints the linked staticlib here without
/// touching the Swift glue or bridging header.
///
/// # Safety
/// Same contract as `backend_ios::ios_main_with_register`.
#[no_mangle]
pub unsafe extern "C" fn ios_main(root_view: *mut c_void, endpoint_utf8: *const c_char) {
    unsafe {
        backend_ios::ios_main_with_register(
            root_view,
            endpoint_utf8,
            register_first_party_sdks,
        )
    }
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
