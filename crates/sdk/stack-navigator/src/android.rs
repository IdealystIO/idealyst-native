//! Android-backend handler stub for Stack navigator.
//!
//! **Migration status — not yet wired.** The legacy `Primitive::Navigator`
//! path is still the operational stack on Android (see
//! `backend-android-mobile/src/imp/primitives/navigator.rs`). This
//! module exists so the SDK builds across the workspace, but its
//! `register` call is a no-op until the per-backend handler is ported
//! through the new `NavigatorHandler` contract.
//!
//! Port checklist:
//! 1. Add `navigator_handlers: NavigatorRegistry<AndroidBackend>` field
//!    on `AndroidBackend` + inherent `register_navigator::<P, F>(&mut self,
//!    factory)`.
//! 2. Override `Backend::create_navigator_extension` to look up + init
//!    + install dispatcher.
//! 3. Port the `RustNavigator` Kotlin-class JNI bridge + FragmentManager
//!    backstack manipulation + slot-style applier from
//!    `backend-android-mobile/src/imp/primitives/navigator.rs` into a
//!    fresh `AndroidStackHandler` in this file.
//! 4. Replace the no-op `register` with the usual registry install.

use backend_android_mobile::AndroidBackend;

/// No-op until the Android handler is ported. See module doc.
pub fn register(_backend: &mut AndroidBackend) {
    // See web.rs for full notes — same posture.
}
