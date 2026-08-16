//! The CALayer machinery behind [`crate::shadow::ShadowPlacement`], shared
//! verbatim by iOS and macOS.
//!
//! CALayer is the same class on both platforms — only *getting* to it differs
//! (`UIView.layer` is mandatory, `NSView.layer` is nil until `wantsLayer`) — so
//! every routine here takes the layer itself. The two backends contribute the
//! view lookup and the `Color → CGColor` adapter, and share this file for the
//! rest. That's Rule #7 held at the tightest seam available: there is no second
//! implementation to drift.
//!
//! ## The sibling layer
//!
//! A bounds-masked layer clips its own drop shadow away (see
//! [`crate::shadow`]), so when a style asks for `shadow` *and*
//! `overflow: hidden` the shadow moves to a second CALayer inserted into the
//! **parent's** layer, directly beneath the view's own. It is never a
//! descendant of the masked layer, so nothing clips it.
//!
//! ## The sibling must have a fill — `shadowPath` alone is not enough
//!
//! CoreAnimation derives a shadow from the layer's composited content and
//! scales it by that content's alpha; `shadowPath` chooses the shadow's *shape*
//! but does not conjure something to cast it. A contentless layer with a
//! `shadowPath` and `shadowOpacity = 1` renders **nothing**. Measured by
//! rasterizing the same scene three ways: no fill → alpha 0 outside the box,
//! a fill at alpha 0.01 → 1, an opaque fill → 98. The first version of this
//! module left the sibling empty and was silently invisible.
//!
//! So [`sync_sibling`] mirrors the card's own background onto the sibling. The
//! sibling has the card's frame and corner radius and sits directly behind it,
//! so an opaque background hides it exactly. A translucent one cannot hide, and
//! is refused rather than approximated — see [`mirror_caster_fill`].
//!
//! The view hierarchy is untouched: no extra `UIView`/`NSView`, so child
//! indexing, the Taffy mirror, hit-testing and robot introspection all behave
//! exactly as before. Only the layer tree gains a node, and only for the views
//! that actually asked for both.
//!
//! ## Lifecycle, and why it lands where it does
//!
//! * **Install** is deferred to the layout pass. At style-apply time the view
//!   usually has no superlayer yet (styles are applied on freshly created views
//!   before insertion), so there is nowhere to put the sibling. The layer is
//!   *created and configured* eagerly and *parented* by [`sync_sibling`], which
//!   the layout pass already calls per view.
//! * **Ownership** is a strong reference held on the view's own layer under
//!   [`SIBLING_KEY`]. Keeping the handle through a detach is deliberate: a
//!   reparent is a remove followed by an insert, and the next layout pass
//!   re-attaches the same layer to the new parent.
//! * **Teardown** has to be explicit, because the sibling lives in the
//!   *parent's* layer and so does not go away when the view does. Backends call
//!   [`detach_sibling`] from their `remove_child` / `clear_children` paths.
//!   Removing an *ancestor* needs no hook: the sibling sits inside that
//!   ancestor's subtree and is torn down with it.
//! * **Z-order** is re-checked on every sync. AppKit and UIKit both rewrite
//!   `sublayers` as subviews come and go, and a shadow that drifts above its
//!   view paints over the card's own content.
//!
//! ## Known limitation: transforms and opacity animate on the view only
//!
//! The sibling is a peer, not a descendant, so a transform or opacity written
//! straight to the view's layer by the animation drivers does not carry to it.
//! [`sync_sibling`] mirrors both, but it only runs on layout, so a shadow under
//! a *clipped* card that is mid-tween holds its last laid-out geometry. Rotation
//! is approximated by the layer's transformed bounding box, since the sibling
//! tracks `frame`. Plain (unclipped) shadows are unaffected — they stay on the
//! view's own layer, where CoreAnimation transforms them for free — which is
//! the other reason [`crate::shadow::shadow_placement`] keeps the sibling
//! branch as narrow as it is.

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, NSString};

use crate::cg::{CGColorRef, CGPathRef};

/// Marks a layer as carrying a BOX shadow, so [`sync_own_shadow_path`] knows it
/// may trace a rectangular path over it, and so the clearing paths know the
/// shadow is theirs to remove.
///
/// A label's `text_shadow` deliberately takes the GLYPH silhouette — stamping a
/// rect path there would paint a solid slab behind the text — and never sets
/// this flag.
pub const BOX_SHADOW_FLAG_KEY: &str = "idealyst_has_box_shadow";

/// KVC key on the view's own layer holding a strong reference to its sibling
/// shadow layer. CALayer supports arbitrary KVC keys and retains the value.
const SIBLING_KEY: &str = "idealyst_shadow_sibling";

/// `CALayer.name` on the sibling. Not used for lookup (the KVC handle is O(1));
/// it exists so the layer is identifiable in a Core Animation layer dump when
/// someone is staring at an unexplained layer in Instruments.
const SIBLING_NAME: &str = "idealyst_box_shadow";

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Rounded-rect path in one call, on both platforms. The toolkit
    /// equivalents diverge (`UIBezierPath.bezierPathWithRoundedRect:` vs
    /// `NSBezierPath.cgPath`, the latter macOS 14+ against a floor of 11.0), so
    /// going straight to CoreGraphics is what lets this file be shared.
    fn CGPathCreateWithRoundedRect(
        rect: CGRect,
        corner_width: CGFloat,
        corner_height: CGFloat,
        transform: *const std::ffi::c_void,
    ) -> CGPathRef;
    fn CGPathRelease(path: CGPathRef);
    fn CGColorGetAlpha(color: CGColorRef) -> CGFloat;
}

/// Suppresses CoreAnimation's implicit animations for the duration of a scope.
///
/// A CALayer that does not back a view animates every property change over
/// ~0.25 s by default. The sibling is exactly such a layer, so without this its
/// frame and path would *ease* toward each new layout instead of tracking it —
/// the shadow visibly lagging behind its card during a scroll or a resize.
/// RAII so an early return can't leave a transaction open.
struct NoImplicitAnimations;

impl NoImplicitAnimations {
    fn begin() -> Self {
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

/// True if this layer carries a box shadow that we installed.
pub fn has_box_shadow(layer: &NSObject) -> bool {
    let key = NSString::from_str(BOX_SHADOW_FLAG_KEY);
    let flag: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
    !flag.is_null()
}

fn set_box_shadow_flag(layer: &NSObject, on: bool) {
    let key = NSString::from_str(BOX_SHADOW_FLAG_KEY);
    unsafe {
        if on {
            let flag: Retained<NSObject> =
                msg_send_id![class!(NSNumber), numberWithBool: true];
            let _: () = msg_send![layer, setValue: &*flag, forKey: &*key];
        } else {
            let null: *mut NSObject = std::ptr::null_mut();
            let _: () = msg_send![layer, setValue: null, forKey: &*key];
        }
    }
}

/// Write a box shadow's appearance onto `target` — which is either the view's
/// own layer or its sibling.
///
/// `shadowColor` carries the author's alpha and `shadowOpacity` is a plain
/// enable multiplier at 1.0; CoreAnimation multiplies the two, so the effective
/// strength is exactly the authored color alpha. CSS `blur` is the full blur
/// diameter while CALayer's `shadowRadius` is the standard-deviation-ish half,
/// so it halves — both backends inherit that conversion from here rather than
/// each picking a divisor.
pub fn write_shadow_props(target: &NSObject, color: CGColorRef, offset: CGSize, blur: f32) {
    unsafe {
        if !color.0.is_null() {
            let _: () = msg_send![target, setShadowColor: color];
        }
        let _: () = msg_send![target, setShadowOffset: offset];
        let _: () = msg_send![target, setShadowRadius: (blur as CGFloat / 2.0)];
        let _: () = msg_send![target, setShadowOpacity: 1.0f32];
    }
    set_box_shadow_flag(target, true);
}

/// Stop the view's OWN layer from painting a box shadow, including releasing
/// the cached path.
///
/// No-op unless we marked the layer: a label carrying a `text_shadow` must be
/// left alone. Clearing matters for reactive restyles — a shadow that is set
/// and then never unset keeps painting after the author removed it.
pub fn clear_own_shadow(layer: &NSObject) {
    if !has_box_shadow(layer) {
        return;
    }
    unsafe {
        let _: () = msg_send![layer, setShadowOpacity: 0.0f32];
        let _: () = msg_send![layer, setShadowPath: CGPathRef(std::ptr::null())];
    }
    set_box_shadow_flag(layer, false);
}

/// Give a box shadow an explicit `shadowPath` tracing `bounds` at `radius`.
///
/// **Why this matters for frame rate.** With `shadowOpacity > 0` and no path,
/// CoreAnimation derives the shadow's shape from the layer's alpha channel: it
/// renders the subtree into an offscreen buffer, blurs it to build the
/// silhouette, then composites — and redoes that on every recomposite. A
/// screenful of shadowed cards then costs a screenful of offscreen passes per
/// scrolled frame (yellow under Simulator ▸ Debug ▸ Color Offscreen-Rendered
/// Yellow). Tracing the rounded rect ourselves skips the derivation entirely.
fn set_rounded_shadow_path(target: &NSObject, bounds: CGRect, radius: CGFloat) {
    // Create-rule function: we own the +1. `setShadowPath:` retains it, so we
    // release ours afterwards.
    let path = unsafe {
        CGPathCreateWithRoundedRect(bounds, radius, radius, std::ptr::null())
    };
    if path.0.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![target, setShadowPath: path];
        CGPathRelease(path);
    }
}

/// Re-trace the OWN-layer shadow path against the layer's current bounds and
/// corner radius. Call from the post-layout hook AFTER the corner radius has
/// been clamped, so the path follows the final curve.
///
/// The path is in layer coordinates and does NOT follow a resize, so without
/// this a shadow keeps the shape it had at its first layout.
pub fn sync_own_shadow_path(layer: &NSObject) {
    if !has_box_shadow(layer) {
        return;
    }
    let opacity: f32 = unsafe { msg_send![layer, shadowOpacity] };
    if opacity <= 0.0 {
        return;
    }
    let bounds: CGRect = unsafe { msg_send![layer, bounds] };
    // A 0×0 view would get an empty path; the layout pass calls again once
    // Taffy has assigned a frame.
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return;
    }
    let radius: CGFloat = unsafe { msg_send![layer, cornerRadius] };
    set_rounded_shadow_path(layer, bounds, radius);
}

/// The sibling shadow layer for `layer`, if one has been created.
pub fn sibling(layer: &NSObject) -> Option<Retained<NSObject>> {
    let key = NSString::from_str(SIBLING_KEY);
    let ptr: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*key] };
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { Retained::retain(ptr) }.expect("non-null sibling layer"))
    }
}

/// The sibling shadow layer for `layer`, creating and storing it on first call.
/// The returned layer is unparented until [`sync_sibling`] runs.
pub fn ensure_sibling(layer: &NSObject) -> Retained<NSObject> {
    if let Some(existing) = sibling(layer) {
        return existing;
    }
    let sib: Retained<NSObject> = unsafe { msg_send_id![class!(CALayer), layer] };
    unsafe {
        let name = NSString::from_str(SIBLING_NAME);
        let _: () = msg_send![&*sib, setName: &*name];
        let key = NSString::from_str(SIBLING_KEY);
        let _: () = msg_send![layer, setValue: &*sib, forKey: &*key];
    }
    sib
}

/// Unparent the sibling but keep the handle, so a reparent re-attaches the same
/// layer on the next layout pass.
///
/// Backends MUST call this when a view leaves its parent: the sibling lives in
/// the *parent's* layer, so removing the view alone would leave a shadow
/// floating where the card used to be.
pub fn detach_sibling(layer: &NSObject) {
    if let Some(sib) = sibling(layer) {
        let _: () = unsafe { msg_send![&*sib, removeFromSuperlayer] };
    }
}

/// Unparent the sibling AND drop the handle — the style no longer wants one.
pub fn drop_sibling(layer: &NSObject) {
    if let Some(sib) = sibling(layer) {
        unsafe {
            let _: () = msg_send![&*sib, removeFromSuperlayer];
            let key = NSString::from_str(SIBLING_KEY);
            let null: *mut NSObject = std::ptr::null_mut();
            let _: () = msg_send![layer, setValue: null, forKey: &*key];
        }
    }
}

/// Parent, position and re-path the sibling against the view's current layout.
/// No-op for a view with no sibling, so the layout pass can call it blindly.
pub fn sync_sibling(layer: &NSObject) {
    let Some(sib) = sibling(layer) else { return };

    let superlayer: *mut NSObject = unsafe { msg_send![layer, superlayer] };
    if superlayer.is_null() {
        // Not in a hierarchy — either not inserted yet, or just detached. Make
        // sure the shadow isn't left parented to a stale ancestor; the next
        // layout pass re-attaches once the view has a home.
        detach_sibling(layer);
        return;
    }

    let _no_anim = NoImplicitAnimations::begin();

    // Z-order: directly beneath the view's own layer. Re-checked every sync
    // because both toolkits rewrite `sublayers` as subviews come and go, and a
    // shadow that drifts above its view paints over the card's own content.
    let current: *mut NSObject = unsafe { msg_send![&*sib, superlayer] };
    if current != superlayer || !is_directly_below(superlayer, &sib, layer) {
        unsafe {
            if !current.is_null() {
                let _: () = msg_send![&*sib, removeFromSuperlayer];
            }
            let _: () = msg_send![superlayer, insertSublayer: &*sib, below: layer];
        }
    }

    // The view's backing-layer `frame` is already expressed in the superlayer's
    // coordinate space, whatever the view's flipped-ness — reusing it verbatim
    // is what keeps this file free of AppKit's `isFlipped` geometry question.
    let frame: CGRect = unsafe { msg_send![layer, frame] };
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        // Pre-layout: nothing to trace yet, and a stale path would paint a
        // shadow at the old size. Hide until a real frame arrives.
        let _: () = unsafe { msg_send![&*sib, setHidden: true] };
        return;
    }
    let card_hidden: bool = unsafe { msg_send![layer, isHidden] };
    unsafe {
        let _: () = msg_send![&*sib, setFrame: frame];
        // Mirror opacity so a faded card doesn't leave a full-strength shadow
        // behind. See the module's known-limitation note: this tracks layout,
        // not mid-tween animation frames.
        let opacity: f32 = msg_send![layer, opacity];
        let _: () = msg_send![&*sib, setOpacity: opacity];
    }

    let radius: CGFloat = unsafe { msg_send![layer, cornerRadius] };
    let bounds = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: frame.size,
    };
    set_rounded_shadow_path(&sib, bounds, radius);
    let castable = mirror_caster_fill(layer, &sib, radius);
    // Hidden if the card is hidden, or if there's no fill that could cast.
    let _: () = unsafe { msg_send![&*sib, setHidden: card_hidden || !castable] };
}

/// Copy the card's background fill onto its sibling, because **CoreAnimation
/// scales a shadow by the alpha of the layer casting it.**
///
/// This is not an optimization, it is the thing that makes the sibling work at
/// all. `shadowPath` chooses the shadow's *shape*; it does not conjure a
/// caster. A contentless layer with a `shadowPath` and `shadowOpacity = 1`
/// renders nothing — measured, not assumed: rasterizing one gives alpha 0
/// outside the box, a fill at alpha 0.01 gives 1, and an opaque fill gives 98.
/// The first implementation here left the sibling empty and was invisible for
/// exactly this reason.
///
/// Reading the fill off the card's own layer (rather than plumbing the authored
/// `background` through both backends) keeps every background writer — the
/// style path, the color-transition driver, the animation setters — working
/// untouched: whatever they last wrote is what the next layout pass mirrors.
///
/// The sibling sits directly behind a card of the same size and corner radius,
/// so an OPAQUE background hides it completely and the result is
/// pixel-identical to the card drawn alone — measured, see
/// `translucent_card_tradeoff` in `tests/apple/shadow_layer_body.rs`.
///
/// **A translucent background is refused, not approximated.** Mirroring one
/// paints it twice (sibling + card): a 50%-alpha card measured 128 → 224 on the
/// interior, a plainly visible change, and the shadow came out at half strength
/// because CoreAnimation scaled it by that same alpha. Trading a wrong-looking
/// box for a shadow is not a good deal, so we leave the shadow off — exactly
/// the behaviour that view had before any of this — and the sibling stays
/// hidden and inert.
///
/// Fixing the translucent case properly means painting the background ONCE, on
/// the sibling, and clearing it from the card — which requires every background
/// writer in both backends (the style path, the color-transition driver, the
/// animation setters) to route through this seam rather than writing the card's
/// layer directly. That is a deliberate, separate change.
/// Returns whether the sibling ended up with a fill that can actually cast.
fn mirror_caster_fill(layer: &NSObject, sib: &NSObject, radius: CGFloat) -> bool {
    let bg: CGColorRef = unsafe { msg_send![layer, backgroundColor] };
    // A layer with no background, or a see-through one, cannot host a fill that
    // hides behind the card.
    if bg.0.is_null() || unsafe { CGColorGetAlpha(bg) } < OPAQUE_ENOUGH {
        return false;
    }
    unsafe {
        let _: () = msg_send![sib, setBackgroundColor: bg];
        // Match the curve so the two fills' antialiased edges coincide; a square
        // sibling would poke corners out from behind a rounded card.
        let _: () = msg_send![sib, setCornerRadius: radius];
    }
    true
}

/// Alpha at or above which a background is treated as fully covering. Just
/// under 1.0 so a color that round-tripped through 8-bit (255/255 = 1.0, but
/// 254/255 = 0.996 after an author writes `rgba(…, 0.997)`) isn't rejected on a
/// floating-point hair.
const OPAQUE_ENOUGH: CGFloat = 0.99;

/// Is `sib` the sublayer immediately preceding `layer` in `superlayer`?
///
/// Read-only walk on purpose: `[CALayer sublayers]` hands back a *live*
/// `CALayerArray` proxy, and mutating it mid-index-walk runs past the new end
/// and aborts (the crash `gradient::remove_existing` documents).
fn is_directly_below(superlayer: *mut NSObject, sib: &NSObject, layer: &NSObject) -> bool {
    let subs: *mut NSObject = unsafe { msg_send![superlayer, sublayers] };
    if subs.is_null() {
        return false;
    }
    let count: usize = unsafe { msg_send![subs, count] };
    let sib_ptr = sib as *const NSObject;
    let layer_ptr = layer as *const NSObject;
    for i in 0..count {
        let entry: *mut NSObject = unsafe { msg_send![subs, objectAtIndex: i] };
        if entry as *const NSObject == sib_ptr {
            if i + 1 >= count {
                return false;
            }
            let next: *mut NSObject = unsafe { msg_send![subs, objectAtIndex: i + 1] };
            return next as *const NSObject == layer_ptr;
        }
    }
    false
}
