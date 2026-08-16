//! Live-CALayer integration test for the synthesized sibling shadow layer.
//!
//! `harness = false` on purpose: libtest runs every `#[test]` on a spawned
//! worker thread, and Core Animation wants the main thread. With no harness
//! this file's `main` IS the process's main thread, which is the closest
//! reachable environment to how the backends actually call this code.
//!
//! Everything here is real: real `CALayer`s, real `insertSublayer:below:`, real
//! `CGPath`s handed to `setShadowPath:`. That matters beyond the logic — passing
//! a CoreGraphics handle with the wrong Objective-C type encoding does not warn,
//! it ABORTS the process at the call site (see `backend_apple_core::cg`), so
//! merely *reaching* the end of this file proves the encodings line up.
//!
//! Run: `cargo test -p backend-apple-core --test shadow_layer_calayer`

use backend_apple_core::cg::{CGColorRef, CGPathRef};
use backend_apple_core::shadow_layer;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize};

// `class!(CALayer)` is a runtime lookup, so QuartzCore has to be loaded. The
// backends get it via UIKit/AppKit; this test links it directly.
#[link(name = "QuartzCore", kind = "framework")]
extern "C" {}

/// Opaque `CGContextRef`, encoded as `^{CGContext=}` — the argument type of
/// `-[CALayer renderInContext:]`. Same trap as `cg::CGColorRef`: a bare
/// `*mut c_void` aborts the process instead of warning.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGContextRef(*mut std::ffi::c_void);

unsafe impl objc2::encode::Encode for CGContextRef {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGContext", &[]));
}

#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGColorSpaceRef(*mut std::ffi::c_void);

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);
    fn CGBitmapContextCreate(
        data: *mut std::ffi::c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGBitmapContextGetData(ctx: CGContextRef) -> *mut u8;
    fn CGContextRelease(ctx: CGContextRef);
    fn CGColorCreate(space: CGColorSpaceRef, components: *const CGFloat) -> CGColorRef;
    fn CGColorRelease(color: CGColorRef);
}

/// `kCGImageAlphaPremultipliedLast` — an RGBA8 buffer we can index directly.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

/// Rasterize `root` (and its sublayers) into an RGBA8 buffer of `w`×`h`.
///
/// `-[CALayer renderInContext:]` runs the same Core Animation drawing the
/// compositor does, including `masksToBounds` clipping and shadows — which is
/// what makes this a real check of "does the shadow actually paint" rather than
/// a restatement of the properties we just set. The context starts fully
/// transparent, so any non-zero alpha outside a layer's own box IS shadow.
fn rasterize(root: &NSObject, w: usize, h: usize) -> Vec<u8> {
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let ctx = CGBitmapContextCreate(
            std::ptr::null_mut(),
            w,
            h,
            8,
            w * 4,
            cs,
            ALPHA_PREMULTIPLIED_LAST,
        );
        assert!(!ctx.0.is_null(), "failed to create bitmap context");
        let _: () = msg_send![root, renderInContext: ctx];
        let data = CGBitmapContextGetData(ctx);
        let out = std::slice::from_raw_parts(data, w * h * 4).to_vec();
        CGContextRelease(ctx);
        CGColorSpaceRelease(cs);
        out
    }
}

/// Alpha at a pixel. Coordinates are bitmap coordinates; every sample this test
/// takes is on the card's vertical centre line, where the CoreGraphics /
/// Core Animation y-flip cancels out.
fn alpha_at(buf: &[u8], w: usize, x: usize, y: usize) -> u8 {
    buf[(y * w + x) * 4 + 3]
}

fn rgba_color(r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) -> CGColorRef {
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let comps = [r, g, b, a];
        let c = CGColorCreate(cs, comps.as_ptr());
        CGColorSpaceRelease(cs);
        c
    }
}

// ---------------------------------------------------------------- helpers

fn new_layer() -> Retained<NSObject> {
    unsafe { msg_send_id![class!(CALayer), layer] }
}

fn rect(x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

fn set_frame(layer: &NSObject, r: CGRect) {
    let _: () = unsafe { msg_send![layer, setFrame: r] };
}

fn frame_of(layer: &NSObject) -> CGRect {
    unsafe { msg_send![layer, frame] }
}

fn superlayer_of(layer: &NSObject) -> *mut NSObject {
    unsafe { msg_send![layer, superlayer] }
}

fn add_sublayer(parent: &NSObject, child: &NSObject) {
    let _: () = unsafe { msg_send![parent, addSublayer: child] };
}

fn shadow_opacity(layer: &NSObject) -> f32 {
    unsafe { msg_send![layer, shadowOpacity] }
}

fn shadow_path(layer: &NSObject) -> CGPathRef {
    unsafe { msg_send![layer, shadowPath] }
}

/// Sublayer order as raw pointers — the property the whole z-order dance is
/// about.
fn sublayer_ptrs(parent: &NSObject) -> Vec<*const NSObject> {
    let subs: *mut NSObject = unsafe { msg_send![parent, sublayers] };
    if subs.is_null() {
        return Vec::new();
    }
    let count: usize = unsafe { msg_send![subs, count] };
    (0..count)
        .map(|i| {
            let p: *mut NSObject = unsafe { msg_send![subs, objectAtIndex: i] };
            p as *const NSObject
        })
        .collect()
}

fn index_of(parent: &NSObject, layer: &NSObject) -> Option<usize> {
    let target = layer as *const NSObject;
    sublayer_ptrs(parent).iter().position(|p| *p == target)
}

// ---------------------------------------------------------------- checks

struct Report {
    failures: Vec<String>,
    checks: usize,
}

impl Report {
    fn check(&mut self, what: &str, ok: bool) {
        self.checks += 1;
        if ok {
            println!("  ok   {what}");
        } else {
            println!("  FAIL {what}");
            self.failures.push(what.to_string());
        }
    }
}

pub fn run() {
    let mut r = Report {
        failures: Vec::new(),
        checks: 0,
    };

    // A parent with two children, the second of which is our shadowed,
    // clipped card. `first` exists so "directly below the card" is a stronger
    // claim than "at index 0".
    let parent = new_layer();
    set_frame(&parent, rect(0.0, 0.0, 800.0, 600.0));
    let first = new_layer();
    set_frame(&first, rect(0.0, 0.0, 100.0, 20.0));
    let card = new_layer();
    set_frame(&card, rect(50.0, 60.0, 200.0, 100.0));
    let _: () = unsafe { msg_send![&*card, setCornerRadius: 16.0 as CGFloat] };
    // The masked layer — the thing that makes the sibling necessary at all.
    let _: () = unsafe { msg_send![&*card, setMasksToBounds: true] };
    add_sublayer(&parent, &first);
    add_sublayer(&parent, &card);

    println!("sibling install");
    let sib = shadow_layer::ensure_sibling(&card);
    r.check(
        "ensure_sibling is idempotent — a second call returns the same layer",
        {
            let again = shadow_layer::ensure_sibling(&card);
            &*again as *const NSObject == &*sib as *const NSObject
        },
    );
    r.check(
        "a freshly created sibling is unparented (nothing to parent it to yet)",
        superlayer_of(&sib).is_null(),
    );

    // Null color exercises `write_shadow_props`'s null guard; the shadow still
    // paints in CoreAnimation's default black.
    shadow_layer::write_shadow_props(
        &sib,
        CGColorRef(std::ptr::null()),
        CGSize {
            width: 0.0,
            height: 10.0,
        },
        24.0,
    );
    r.check("shadow is enabled on the sibling", shadow_opacity(&sib) == 1.0);
    r.check("CSS blur halves into shadowRadius", {
        let radius: CGFloat = unsafe { msg_send![&*sib, shadowRadius] };
        radius == 12.0 as CGFloat
    });
    r.check("offset is written through", {
        let off: CGSize = unsafe { msg_send![&*sib, shadowOffset] };
        off.width == 0.0 && off.height == 10.0
    });

    println!("sync — parenting, z-order, geometry, path");
    shadow_layer::sync_sibling(&card);
    r.check(
        "sibling is parented to the CARD'S PARENT, not the card — a child of the \
         masked layer would be clipped, which is the whole bug",
        superlayer_of(&sib) == &*parent as *const NSObject as *mut NSObject,
    );
    r.check(
        "sibling sits DIRECTLY BELOW the card (order [first, sibling, card])",
        sublayer_ptrs(&parent)
            == vec![
                &*first as *const NSObject,
                &*sib as *const NSObject,
                &*card as *const NSObject,
            ],
    );
    r.check("sibling matches the card's frame in parent coordinates", {
        let f = frame_of(&sib);
        let c = frame_of(&card);
        f.origin.x == c.origin.x
            && f.origin.y == c.origin.y
            && f.size.width == c.size.width
            && f.size.height == c.size.height
    });
    r.check(
        "an explicit shadowPath was traced — without one CoreAnimation derives \
         the silhouette through an offscreen pass on every recomposite",
        !shadow_path(&sib).0.is_null(),
    );
    r.check(
        "the sibling itself is NOT masked (masking it would clip the shadow \
         right back off)",
        {
            let masked: bool = unsafe { msg_send![&*sib, masksToBounds] };
            !masked
        },
    );

    println!("resize");
    set_frame(&card, rect(50.0, 60.0, 300.0, 140.0));
    shadow_layer::sync_sibling(&card);
    r.check("sibling follows a resize", {
        let f = frame_of(&sib);
        f.size.width == 300.0 && f.size.height == 140.0
    });

    println!("z-order repair");
    // Stand in for the toolkit rewriting `sublayers` as subviews come and go:
    // shove the shadow to the top, where it would paint OVER the card.
    let _: () = unsafe { msg_send![&*sib, removeFromSuperlayer] };
    add_sublayer(&parent, &sib);
    r.check(
        "precondition: the shadow is now above the card",
        index_of(&parent, &sib) > index_of(&parent, &card),
    );
    shadow_layer::sync_sibling(&card);
    r.check(
        "sync pulls a drifted shadow back below its card",
        sublayer_ptrs(&parent)
            == vec![
                &*first as *const NSObject,
                &*sib as *const NSObject,
                &*card as *const NSObject,
            ],
    );

    println!("detach / reattach");
    shadow_layer::detach_sibling(&card);
    r.check(
        "detach unparents the shadow — otherwise removing the card leaves it \
         painting where the card used to be",
        superlayer_of(&sib).is_null(),
    );
    r.check(
        "detach KEEPS the handle, so a reparent re-attaches the same layer",
        shadow_layer::sibling(&card).is_some(),
    );
    shadow_layer::sync_sibling(&card);
    r.check(
        "the next layout pass re-attaches it",
        superlayer_of(&sib) == &*parent as *const NSObject as *mut NSObject,
    );

    println!("orphaned card");
    let orphan = new_layer();
    set_frame(&orphan, rect(0.0, 0.0, 80.0, 80.0));
    let orphan_sib = shadow_layer::ensure_sibling(&orphan);
    shadow_layer::sync_sibling(&orphan);
    r.check(
        "a card with no superlayer leaves its shadow unparented instead of \
         panicking or parenting it to a stale ancestor",
        superlayer_of(&orphan_sib).is_null(),
    );

    println!("zero-size card");
    let tiny = new_layer();
    set_frame(&tiny, rect(0.0, 0.0, 0.0, 0.0));
    add_sublayer(&parent, &tiny);
    let tiny_sib = shadow_layer::ensure_sibling(&tiny);
    shadow_layer::sync_sibling(&tiny);
    r.check(
        "a pre-layout 0x0 card hides its shadow rather than tracing an empty \
         path or painting at a stale size",
        {
            let hidden: bool = unsafe { msg_send![&*tiny_sib, isHidden] };
            hidden
        },
    );

    println!("own-layer shadow (the unclipped, single-layer case)");
    let plain = new_layer();
    set_frame(&plain, rect(0.0, 0.0, 120.0, 60.0));
    let _: () = unsafe { msg_send![&*plain, setCornerRadius: 8.0 as CGFloat] };
    add_sublayer(&parent, &plain);
    shadow_layer::write_shadow_props(
        &plain,
        CGColorRef(std::ptr::null()),
        CGSize {
            width: 0.0,
            height: 4.0,
        },
        12.0,
    );
    shadow_layer::sync_own_shadow_path(&plain);
    r.check(
        "an unclipped view keeps its shadow on its own layer",
        shadow_opacity(&plain) == 1.0 && !shadow_path(&plain).0.is_null(),
    );
    r.check(
        "sync_sibling is a no-op for a view that never grew one",
        shadow_layer::sibling(&plain).is_none(),
    );

    println!("clearing (reactive restyle)");
    shadow_layer::clear_own_shadow(&plain);
    r.check(
        "clear_own_shadow stops the paint AND releases the cached path — a \
         shadow that is set and never unset keeps painting after the author \
         removed it",
        shadow_opacity(&plain) == 0.0 && shadow_path(&plain).0.is_null(),
    );
    r.check(
        "has_box_shadow is false once cleared",
        !shadow_layer::has_box_shadow(&plain),
    );

    // A glyph shadow (`text_shadow` on a label) is written straight to the
    // layer by the text path and never carries our marker; the box-shadow
    // routines must not touch it, or a label would get a solid rect painted
    // behind its text.
    println!("glyph shadows are left alone");
    let label = new_layer();
    set_frame(&label, rect(0.0, 0.0, 100.0, 20.0));
    unsafe {
        let _: () = msg_send![&*label, setShadowOpacity: 0.75f32];
    }
    shadow_layer::clear_own_shadow(&label);
    r.check(
        "an unmarked (glyph) shadow survives clear_own_shadow",
        shadow_opacity(&label) == 0.75,
    );
    shadow_layer::sync_own_shadow_path(&label);
    r.check(
        "an unmarked (glyph) shadow is never given a rectangular path",
        shadow_path(&label).0.is_null(),
    );

    println!("drop");
    shadow_layer::drop_sibling(&card);
    r.check(
        "drop_sibling unparents AND releases the handle",
        shadow_layer::sibling(&card).is_none() && superlayer_of(&sib).is_null(),
    );


    rasterized_shadow_checks(&mut r);
    translucent_card_tradeoff(&mut r);

    println!("\n{} checks, {} failed", r.checks, r.failures.len());
    if !r.failures.is_empty() {
        for f in &r.failures {
            eprintln!("FAILED: {f}");
        }
        std::process::exit(1);
    }
    println!("PASS");
}

/// The actual bug, in pixels.
///
/// `shadow` + `overflow: hidden` on one view rendered NO shadow on iOS and
/// macOS while web rendered one. Everything above asserts that the sibling
/// layer is wired up correctly; this asserts that the wiring produces a
/// visible shadow — and, in the same shape, that putting the shadow on the
/// masked layer (what both backends used to do) produces none. That second
/// half is what makes this a regression test rather than a description: it
/// fails against the old code path, on purpose, every run.
///
/// Both scenes are identical apart from which layer carries the shadow:
/// a 400×300 root, a 200×100 white card at (100,100), `masksToBounds` on,
/// 16 pt corner radius, and a wide zero-offset shadow so the halo surrounds
/// the card and the sample point doesn't depend on the y-flip.
fn rasterized_shadow_checks(r: &mut Report) {
    const W: usize = 400;
    const H: usize = 300;
    // 6 px outside the card's left edge, on its vertical centre line.
    const OUT_X: usize = 94;
    const MID_Y: usize = 150;
    // Well inside the card.
    const IN_X: usize = 200;

    fn scene(on_sibling: bool) -> (Retained<NSObject>, Retained<NSObject>) {
        let root = new_layer();
        set_frame(&root, rect(0.0, 0.0, W as CGFloat, H as CGFloat));
        let card = new_layer();
        set_frame(&card, rect(100.0, 100.0, 200.0, 100.0));
        unsafe {
            let _: () = msg_send![&*card, setCornerRadius: 16.0 as CGFloat];
            // The clip the author asked for with `overflow: hidden`.
            let _: () = msg_send![&*card, setMasksToBounds: true];
            let white = rgba_color(1.0, 1.0, 1.0, 1.0);
            let _: () = msg_send![&*card, setBackgroundColor: white];
            CGColorRelease(white);
        }
        add_sublayer(&root, &card);

        let black = rgba_color(0.0, 0.0, 0.0, 1.0);
        // Zero offset + wide blur: the halo is symmetric, so the sample point
        // is the same whichever way Core Animation's y axis runs.
        let offset = CGSize { width: 0.0, height: 0.0 };
        if on_sibling {
            let sib = shadow_layer::ensure_sibling(&card);
            shadow_layer::write_shadow_props(&sib, black, offset, 40.0);
            shadow_layer::sync_sibling(&card);
        } else {
            shadow_layer::write_shadow_props(&card, black, offset, 40.0);
            shadow_layer::sync_own_shadow_path(&card);
        }
        unsafe { CGColorRelease(black) };
        (root, card)
    }

    println!("rasterized: the bug");
    let (buggy_root, _buggy_card) = scene(false);
    let buggy = rasterize(&buggy_root, W, H);
    let buggy_out = alpha_at(&buggy, W, OUT_X, MID_Y);
    r.check(
        &format!(
            "REGRESSION GUARD — a shadow on the masked layer paints NOTHING \
             outside the card (alpha {buggy_out}); this is the bug, and this \
             check failing means the shadow moved back onto the clipped layer"
        ),
        buggy_out == 0,
    );
    r.check(
        "…the card itself still renders (so the empty halo isn't an empty scene)",
        alpha_at(&buggy, W, IN_X, MID_Y) > 250,
    );

    println!("rasterized: the fix");
    let (fixed_root, fixed_card) = scene(true);
    let fixed = rasterize(&fixed_root, W, H);
    let fixed_out = alpha_at(&fixed, W, OUT_X, MID_Y);
    r.check(
        &format!(
            "the sibling layer paints a real shadow outside the clipped card \
             (alpha {fixed_out}) — the web behaviour, on Core Animation"
        ),
        fixed_out > 0,
    );
    r.check(
        "the card's own fill is unchanged by the sibling (it is BEHIND the card, \
         not over it)",
        alpha_at(&fixed, W, IN_X, MID_Y) > 250,
    );

    // The clip has to survive: the whole point is that the author gets BOTH.
    println!("rasterized: clipping still works");
    unsafe {
        let overflowing = new_layer();
        // Twice the card's width, so it would spill 200 px to the right if the
        // clip were lost — straight through the sample point at x = 350.
        set_frame(&overflowing, rect(0.0, 0.0, 400.0, 100.0));
        let red = rgba_color(1.0, 0.0, 0.0, 1.0);
        let _: () = msg_send![&*overflowing, setBackgroundColor: red];
        CGColorRelease(red);
        add_sublayer(&fixed_card, &overflowing);
    }
    let clipped = rasterize(&fixed_root, W, H);
    // 50 px past the card's right edge, where the child would land unclipped.
    let spill = (clipped[(MID_Y * W + 350) * 4], clipped[(MID_Y * W + 350) * 4 + 3]);
    r.check(
        &format!(
            "an oversized child is still clipped to the card (red={}, alpha={} \
             at 50 px past the edge) — the shadow moved out, the clip did not",
            spill.0, spill.1
        ),
        spill.0 < 200,
    );
    r.check(
        "…and the child does paint inside the card, so the clip isn't hiding \
         everything",
        clipped[(MID_Y * W + IN_X) * 4] > 200,
    );
}

/// A translucent card is REFUSED, not approximated — measured, not asserted.
///
/// The sibling casts by mirroring the card's own fill, and CoreAnimation scales
/// a shadow by its caster's alpha. Mirroring a translucent fill paints it twice:
/// an earlier build of this code measured a 50%-alpha card going 128 -> 224 on
/// the interior, a plainly visible change, for a half-strength shadow. Trading a
/// wrong-looking box for a shadow is a bad deal, so `mirror_caster_fill` leaves
/// the sibling hidden and the view renders exactly as it did before.
///
/// These checks are what stops someone "fixing" the translucent case by simply
/// dropping the alpha gate.
fn translucent_card_tradeoff(r: &mut Report) {
    const W: usize = 400;
    const H: usize = 300;

    fn scene(alpha: CGFloat, with_sibling: bool) -> Retained<NSObject> {
        let root = new_layer();
        set_frame(&root, rect(0.0, 0.0, W as CGFloat, H as CGFloat));
        let card = new_layer();
        set_frame(&card, rect(100.0, 100.0, 200.0, 100.0));
        unsafe {
            let _: () = msg_send![&*card, setMasksToBounds: true];
            // Pure red at the requested alpha, so "how many times was it
            // painted" is readable straight off the red channel.
            let c = rgba_color(1.0, 0.0, 0.0, alpha);
            let _: () = msg_send![&*card, setBackgroundColor: c];
            CGColorRelease(c);
        }
        add_sublayer(&root, &card);
        if with_sibling {
            let black = rgba_color(0.0, 0.0, 0.0, 1.0);
            let sib = shadow_layer::ensure_sibling(&card);
            shadow_layer::write_shadow_props(
                &sib,
                black,
                CGSize { width: 0.0, height: 0.0 },
                40.0,
            );
            shadow_layer::sync_sibling(&card);
            unsafe { CGColorRelease(black) };
        }
        root
    }

    println!("translucent card is refused, not approximated");
    let plain = rasterize(&scene(0.5, false), W, H);
    let shadowed = rasterize(&scene(0.5, true), W, H);
    let plain_in = alpha_at(&plain, W, 200, 150);
    let shadowed_in = alpha_at(&shadowed, W, 200, 150);
    let shadowed_out = alpha_at(&shadowed, W, 94, 150);
    println!(
        "   50% card: interior alpha {plain_in} alone vs {shadowed_in} with a \
         sibling attached; outside = {shadowed_out}"
    );
    r.check(
        &format!(
            "a translucent card's fill is NOT doubled ({plain_in} -> \
             {shadowed_in}) — dropping the alpha gate in mirror_caster_fill \
             measured 128 -> 224 here, a visibly denser box"
        ),
        shadowed_in == plain_in,
    );
    r.check(
        "…and it renders no shadow, i.e. exactly as it did before this change \
         — a known gap, never a regression",
        shadowed_out == 0,
    );

    // The case that actually ships: an opaque card must be pixel-identical.
    let opaque_plain = rasterize(&scene(1.0, false), W, H);
    let opaque_shadowed = rasterize(&scene(1.0, true), W, H);
    let px = |b: &[u8], x: usize| b[(150 * W + x) * 4..(150 * W + x) * 4 + 4].to_vec();
    r.check(
        "an OPAQUE card — every idea-ui Card, and the case this whole change \
         exists for — is pixel-identical inside, with the sibling behind it",
        px(&opaque_plain, 200) == px(&opaque_shadowed, 200),
    );
    r.check(
        "…and gains the shadow outside it",
        alpha_at(&opaque_plain, W, 94, 150) == 0
            && alpha_at(&opaque_shadowed, W, 94, 150) > 0,
    );
}
