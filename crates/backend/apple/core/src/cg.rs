//! Typed CoreGraphics pointer newtypes shared by the Apple backends.
//!
//! ## Why these exist
//!
//! objc2 verifies a method's Objective-C type encoding against the types you
//! pass, in debug builds. CoreGraphics handles are opaque C pointers, and a
//! bare `*const c_void` encodes as `^v` — but UIKit/AppKit declare, say,
//! `-[CALayer setShadowPath:]` as taking `^{CGPath=}` and
//! `-[CALayer setShadowColor:]` as `^{CGColor=}`. The mismatch does not warn:
//! it **aborts the process** at the call site.
//!
//! That failure mode is nasty precisely because it looks like anything but a
//! type error — the app dies mid-layout with no panic message, right after
//! whatever log line preceded the first styled view. It has bitten this
//! codebase more than once (the whiteboard-demo's icon path on macOS; the
//! iOS `shadowPath` work that introduced [`CGPathRef`]).
//!
//! These live in `apple-core`, NOT in a backend, for one specific reason: the
//! backends' UIKit/AppKit modules are `cfg`-gated to their target OS, so a
//! test pinning the encoding inside one of them **never runs on the host** and
//! guards nothing during a normal `cargo test`. Nothing here touches an OS
//! API — it's pure objc2 encoding metadata — so it compiles and tests
//! everywhere.

use objc2::encode::{Encode, Encoding};

/// Opaque `CGColorRef`, encoded as `^{CGColor=}`.
///
/// Used for `setShadowColor:`, `setBorderColor:`, `setBackgroundColor:` on a
/// CALayer, and as the argument to `CGColorGetAlpha`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct CGColorRef(pub *const std::ffi::c_void);

unsafe impl Encode for CGColorRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

/// Opaque `CGPathRef`, encoded as `^{CGPath=}`.
///
/// Used for `-[CALayer setShadowPath:]`. Pass `CGPathRef(std::ptr::null())` to
/// clear a previously-installed path.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct CGPathRef(pub *const std::ffi::c_void);

unsafe impl Encode for CGPathRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGPath", &[]));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `sync_shadow_path` first passed the CGPath as a bare
    /// `*const c_void`. objc2's debug-mode encoding check saw `^v` where
    /// `-[CALayer setShadowPath:]` declares `^{CGPath=}` and SIGABRTed the app
    /// the instant the layout pass reached a shadowed view — the process died
    /// straight after `run_layout_pass` with no panic text to go on.
    #[test]
    fn regression_cgpath_encodes_as_typed_cgpath_not_void_ptr() {
        assert_eq!(
            CGPathRef::ENCODING,
            Encoding::Pointer(&Encoding::Struct("CGPath", &[])),
            "CGPathRef must encode as ^{{CGPath=}} for -[CALayer setShadowPath:] \
             — a bare `*const c_void` (^v) aborts the process at layout",
        );
    }

    /// Same trap, sibling type: `setShadowColor:` / `setBorderColor:` declare
    /// `^{CGColor=}`.
    #[test]
    fn cgcolor_encodes_as_typed_cgcolor() {
        assert_eq!(
            CGColorRef::ENCODING,
            Encoding::Pointer(&Encoding::Struct("CGColor", &[])),
            "CGColorRef must encode as ^{{CGColor=}}",
        );
    }

    /// The encodings must stay DISTINCT. Collapsing both to one type would
    /// compile and then abort at whichever call site got the wrong one.
    #[test]
    fn cgcolor_and_cgpath_encodings_differ() {
        assert_ne!(CGColorRef::ENCODING, CGPathRef::ENCODING);
    }
}
