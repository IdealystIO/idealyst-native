//! Suppressing CoreAnimation's implicit animations.
//!
//! A `CALayer` that backs a `UIView` / `NSView` does not animate implicitly:
//! the view is its layer's delegate and returns `NSNull` from
//! `actionForLayer:forKey:` outside an explicit animation block. A layer the
//! backend creates and inserts ITSELF has no such delegate, so CoreAnimation
//! falls back to its default action and eases **every** property change —
//! `position`, `bounds`, `transform`, `path` — over ~0.25 s.
//!
//! That default is wrong for every sublayer this backend owns, because all of
//! them are slaved to a layout pass: they must track the box they decorate,
//! not drift toward it. Left unguarded it shows up as a shadow lagging its
//! card during a scroll, a gradient easing open on first paint, and icons
//! sliding into place on first render.
//!
//! Wrap any layout-driven mutation of a non-view-backed layer in
//! [`NoImplicitAnimations::begin`].

use objc2::runtime::NSObject;
use objc2::{class, msg_send};

/// Suppresses CoreAnimation's implicit animations for the duration of a scope.
///
/// RAII so an early return cannot leave a transaction open — an unbalanced
/// `begin` without its `commit` corrupts the transaction stack for the rest of
/// the run-loop turn, which surfaces far away from the cause.
#[must_use = "the guard must be held for the scope of the mutation; dropping it \
              immediately commits the transaction and re-enables implicit animations"]
pub struct NoImplicitAnimations;

impl NoImplicitAnimations {
    /// Open a transaction with actions disabled.
    pub fn begin() -> Self {
        unsafe {
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
        }
        Self
    }
}

impl Drop for NoImplicitAnimations {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![class!(CATransaction), commit];
        }
    }
}

/// True when `layer` is one the backend created and inserted itself — i.e.
/// one that needs [`NoImplicitAnimations`]. A layer with no delegate is not
/// view-backed.
///
/// Diagnostic only: the sync paths know statically which layers are theirs, so
/// nothing on the hot path pays for this.
pub fn is_unbacked_layer(layer: &NSObject) -> bool {
    let delegate: *mut NSObject = unsafe { msg_send![layer, delegate] };
    delegate.is_null()
}
