//! iOS-backend handler stub for Stack navigator.
//!
//! **Migration status — not yet wired.** The legacy `Primitive::Navigator`
//! path is still the operational stack on iOS (see
//! `backend-ios-mobile/src/imp/navigator.rs`). This module exists so
//! the SDK builds across the workspace, but its `register` call is a
//! no-op until the per-backend handler is ported through the new
//! `NavigatorHandler` contract.
//!
//! Port checklist:
//! 1. Add `navigator_handlers: NavigatorRegistry<IosBackend>` field on
//!    `IosBackend` + inherent `register_navigator::<P, F>(&mut self,
//!    factory)`.
//! 2. Override `Backend::create_navigator_extension` to look up + init
//!    + install dispatcher (see web.rs port checklist for the standard
//!    sequence).
//! 3. Port the `UINavigationController` creation, push/pop/replace/reset
//!    handling, edge-swipe back observer (`UINavigationControllerDelegate`),
//!    and slot-style applier from `backend-ios-mobile/src/imp/navigator.rs`
//!    + the navigator portion of `tab_drawer.rs` into a fresh
//!    `IosStackHandler` in this file.
//! 4. Replace the no-op `register` with the usual registry install.

use backend_ios_mobile::IosBackend;

/// No-op until the iOS handler is ported. See module doc.
pub fn register(_backend: &mut IosBackend) {
    // See web.rs for full notes — same posture.
}
