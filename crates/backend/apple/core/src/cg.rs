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

/// `CATransform3D` — Core Animation's 4x4 matrix, as `-[CALayer transform]`
/// takes and returns it.
///
/// Sixteen `f64`s in row order, matching the C ABI. Lives here rather than in
/// a backend for the reason this whole module exists: an encoding pinned
/// inside a `cfg`-gated UIKit/AppKit module never runs during a host
/// `cargo test`, and a wrong struct encoding does not warn — it aborts the
/// process at the call site.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct CATransform3D {
    pub m11: f64, pub m12: f64, pub m13: f64, pub m14: f64,
    pub m21: f64, pub m22: f64, pub m23: f64, pub m24: f64,
    pub m31: f64, pub m32: f64, pub m33: f64, pub m34: f64,
    pub m41: f64, pub m42: f64, pub m43: f64, pub m44: f64,
}

unsafe impl Encode for CATransform3D {
    const ENCODING: Encoding = Encoding::Struct(
        "CATransform3D",
        &[
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
            f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING,
        ],
    );
}

impl CATransform3D {
    /// The identity matrix.
    pub const IDENTITY: Self = Self {
        m11: 1.0, m12: 0.0, m13: 0.0, m14: 0.0,
        m21: 0.0, m22: 1.0, m23: 0.0, m24: 0.0,
        m31: 0.0, m32: 0.0, m33: 1.0, m34: 0.0,
        m41: 0.0, m42: 0.0, m43: 0.0, m44: 1.0,
    };

    /// Uniform scale about the layer's anchor point.
    pub const fn scale(s: f64) -> Self {
        Self { m11: s, m22: s, ..Self::IDENTITY }
    }

    /// The uniform scale factor currently encoded, i.e. `m11`. Only
    /// meaningful for a matrix this module built.
    pub fn scale_factor(&self) -> f64 {
        self.m11
    }
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

    /// `-[CALayer setTransform:]` takes the struct BY VALUE, so a wrong field
    /// count or element type mis-marshals the matrix rather than aborting —
    /// which is worse, because it renders as a plausible-looking wrong
    /// transform instead of failing loudly.
    #[test]
    fn catransform3d_encodes_as_sixteen_doubles() {
        let Encoding::Struct(name, fields) = CATransform3D::ENCODING else {
            panic!("CATransform3D must encode as a struct");
        };
        assert_eq!(name, "CATransform3D");
        assert_eq!(fields.len(), 16, "Core Animation's matrix is 4x4");
        assert!(fields.iter().all(|f| *f == f64::ENCODING), "CGFloat is f64 on 64-bit Apple");
    }

    /// The matrix is 16 contiguous doubles; anything else means the `#[repr(C)]`
    /// layout drifted from what Core Animation reads back.
    #[test]
    fn catransform3d_is_sixteen_contiguous_doubles() {
        assert_eq!(std::mem::size_of::<CATransform3D>(), 16 * std::mem::size_of::<f64>());
    }

    /// `scale` must touch only the two diagonal terms — a scale that also
    /// perturbed m33/m44 would skew or vanish the layer.
    #[test]
    fn scale_sets_only_the_x_and_y_diagonal() {
        let m = CATransform3D::scale(2.0);
        assert_eq!(m.m11, 2.0);
        assert_eq!(m.m22, 2.0);
        assert_eq!(m.m33, 1.0, "z is untouched");
        assert_eq!(m.m44, 1.0, "w is untouched");
        assert_eq!(m.scale_factor(), 2.0);
        assert_eq!(CATransform3D::scale(1.0), CATransform3D::IDENTITY);
    }
}
