//! CSS-style borders for the macOS backend.
//!
//! Same model as the iOS backend (`backend-ios-core/src/style.rs`), and
//! the routing decision is the SHARED one (`backend_apple_core::border::
//! uniform_border`) so the two converge byte for byte (Rule #7):
//!
//!   * A **uniform** border (all four sides the same width + effective
//!     color) is one `CALayer.borderWidth`/`borderColor` stroke, which
//!     follows the layer's `cornerRadius` with no corner seams.
//!   * An **asymmetric** border (e.g. a `border-bottom`-only underline —
//!     the `TabButton`/`SegmentedControl` active marker) can't be a
//!     uniform layer stroke, so each non-zero side is a thin `NSView`
//!     bar pinned to that edge.
//!
//! ## Why per-side bars are `NSView` + Auto Layout, not CALayer sublayers
//!
//! The macOS gradient uses a CALayer sublayer frame-synced in the layout
//! pass (`gradient::sync_gradient_sublayer`). Borders deliberately do NOT:
//! a sublayer's per-side rectangle would have to be recomputed against the
//! view's `isFlipped` geometry on every resize, whereas `NSView` layout
//! anchors (`topAnchor`/`bottomAnchor`/`leadingAnchor`/`trailingAnchor`)
//! are SEMANTIC — `topAnchor` is the visual top regardless of flip — and
//! AppKit re-resolves them automatically when Taffy changes the parent's
//! frame. So there's no border resync in the layout pass (matching iOS,
//! which also relies on Auto Layout for this), and no flipped-coordinate
//! math to get wrong. The bars are leaf decorations (never an ancestor of
//! a `CAMetalLayer`), so layer-backing them doesn't trip the canvas-detach
//! hazard (`project_macos_appkit_uikit_diffs` #21).
//!
//! A bar is a plain `NSView`, so it's never the `FlippedView` the click
//! path treats as interactive; a click landing on the thin strip forwards
//! up the responder chain to the parent's `mouseDown:` unchanged.

use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::{CGRect, CGFloat, MainThreadMarker, NSObject, NSString};
use runtime_core::{Color, StyleRules};

use super::{color_to_nscolor, CGColorRef};

const BORDER_ID_TOP: &str = "idealyst_border_top";
const BORDER_ID_RIGHT: &str = "idealyst_border_right";
const BORDER_ID_BOTTOM: &str = "idealyst_border_bottom";
const BORDER_ID_LEFT: &str = "idealyst_border_left";

/// KVC marker stashed on the parent's layer when at least one per-side bar
/// is installed, so `remove_existing_bars` can short-circuit the subview
/// walk for the overwhelming common case of a view that never had a
/// border. Mirrors the iOS `BORDER_TAG_MARKER` fast path (NSView has no
/// `tag`, so we use a layer KVC key — the same mechanism the deferred
/// corner-radius stash already uses on this backend).
const BORDER_MARKER_KEY: &str = "idealyst_has_border_bars";

fn border_id_for(idx: usize) -> &'static str {
    match idx {
        0 => BORDER_ID_TOP,
        1 => BORDER_ID_RIGHT,
        2 => BORDER_ID_BOTTOM,
        _ => BORDER_ID_LEFT,
    }
}

fn is_border_id(s: &str) -> bool {
    matches!(s, BORDER_ID_TOP | BORDER_ID_RIGHT | BORDER_ID_BOTTOM | BORDER_ID_LEFT)
}

/// Apply the four CSS border sides to `view` (whose backing `layer` the
/// caller has already created). Routes uniform→CALayer stroke,
/// asymmetric→per-side `NSView` bars, none→clear — exactly the iOS shape.
pub(crate) fn apply_border(view: &NSView, layer: &NSObject, style: &StyleRules) {
    let widths = [
        style.border_top_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        style.border_right_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        style.border_bottom_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        style.border_left_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
    ];

    // Tear down any previous per-side bars so reapplies (state overlays,
    // theme swap) replace rather than stack them.
    remove_existing_bars(view, layer);

    let any_width = widths.iter().any(|w| *w > 0.0);
    if !any_width {
        // No border requested — clear any CALayer stroke a prior uniform
        // apply (or a direct-layer SDK call) may have left.
        let _: () = unsafe { msg_send![layer, setBorderWidth: 0.0_f64] };
        return;
    }

    let colors: [Option<Color>; 4] = [
        style.border_top_color.as_ref().map(|t| t.resolve()),
        style.border_right_color.as_ref().map(|t| t.resolve()),
        style.border_bottom_color.as_ref().map(|t| t.resolve()),
        style.border_left_color.as_ref().map(|t| t.resolve()),
    ];

    if let Some((width, color)) = backend_apple_core::border::uniform_border(widths, &colors) {
        // Uniform → CALayer stroke (follows cornerRadius cleanly).
        let ns_color = color_to_nscolor(&color);
        let cg: CGColorRef = unsafe { msg_send![&*ns_color, CGColor] };
        if !cg.0.is_null() {
            let _: () = unsafe { msg_send![layer, setBorderColor: cg] };
        }
        let _: () = unsafe { msg_send![layer, setBorderWidth: width as f64] };
    } else {
        // Asymmetric → per-side bars. Clear any uniform stroke a prior
        // apply left, then paint each non-zero side.
        let _: () = unsafe { msg_send![layer, setBorderWidth: 0.0_f64] };
        let fallback_color = colors.iter().find_map(|c| c.clone());
        for (idx, &w) in widths.iter().enumerate() {
            if w <= 0.0 {
                continue;
            }
            let Some(color) = colors[idx].clone().or_else(|| fallback_color.clone()) else {
                continue;
            };
            install_border_side(view, layer, idx, w as CGFloat, &color);
        }
    }
}

/// Remove any installed per-side bars. Fast path: if the parent's layer
/// carries no `idealyst_has_border_bars` marker we've never installed one,
/// so skip the `subviews` alloc + identifier walk entirely.
fn remove_existing_bars(view: &NSView, layer: &NSObject) {
    let marker_key = NSString::from_str(BORDER_MARKER_KEY);
    let marker_ptr: *mut NSObject = unsafe { msg_send![layer, valueForKey: &*marker_key] };
    if marker_ptr.is_null() {
        return;
    }
    let subviews: Retained<objc2_foundation::NSArray<NSView>> =
        unsafe { msg_send_id![view, subviews] };
    for sub in subviews.iter() {
        let id_obj: *mut NSString = unsafe { msg_send![&*sub, accessibilityIdentifier] };
        if id_obj.is_null() {
            continue;
        }
        let id_str = unsafe { &*id_obj }.to_string();
        if is_border_id(&id_str) {
            let _: () = unsafe { msg_send![&*sub, removeFromSuperview] };
        }
    }
    // Clear the marker — a re-install sets it again; otherwise subsequent
    // applies short-circuit above.
    let null: *const NSObject = std::ptr::null();
    let _: () = unsafe { msg_send![layer, setValue: null, forKey: &*marker_key] };
}

/// Install one thin `NSView` bar pinned to side `idx`
/// (`[top, right, bottom, left]`) of `view` via Auto Layout, so AppKit
/// keeps it sized to the parent across every layout pass.
fn install_border_side(view: &NSView, parent_layer: &NSObject, idx: usize, width: CGFloat, color: &Color) {
    // apply_style runs on the main thread per the framework contract.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let zero = CGRect::default();
    let bar: Retained<NSView> =
        unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: zero] };

    // NSView has no `backgroundColor`; paint via its CALayer. The bar is a
    // leaf, so layer-backing it can't detach a descendant Metal layer.
    let _: () = unsafe { msg_send![&bar, setWantsLayer: true] };
    let bar_layer: Retained<NSObject> = unsafe { msg_send_id![&bar, layer] };
    let ns_color = color_to_nscolor(color);
    let cg: CGColorRef = unsafe { msg_send![&*ns_color, CGColor] };
    if !cg.0.is_null() {
        let _: () = unsafe { msg_send![&bar_layer, setBackgroundColor: cg] };
    }

    let id_str = NSString::from_str(border_id_for(idx));
    let _: () = unsafe { msg_send![&bar, setAccessibilityIdentifier: &*id_str] };

    // Auto Layout, not frame + autoresizing: apply_style runs before Taffy
    // assigns the parent's frame on initial mount, so a frame-based bar
    // would install at 0×0 and never grow. Anchors recompute every layout
    // pass and are flip-agnostic (semantic top/bottom/leading/trailing).
    let _: () = unsafe { msg_send![&bar, setTranslatesAutoresizingMaskIntoConstraints: false] };
    unsafe { view.addSubview(&bar) };

    // Mark the parent layer so the next remove can't take the fast path.
    let marker_key = NSString::from_str(BORDER_MARKER_KEY);
    let one: Retained<NSObject> =
        unsafe { msg_send_id![objc2::class!(NSNumber), numberWithBool: true] };
    let _: () = unsafe { msg_send![parent_layer, setValue: &*one, forKey: &*marker_key] };

    unsafe {
        let p_top: Retained<NSObject> = msg_send_id![view, topAnchor];
        let p_bot: Retained<NSObject> = msg_send_id![view, bottomAnchor];
        let p_lead: Retained<NSObject> = msg_send_id![view, leadingAnchor];
        let p_trail: Retained<NSObject> = msg_send_id![view, trailingAnchor];
        let b_top: Retained<NSObject> = msg_send_id![&bar, topAnchor];
        let b_bot: Retained<NSObject> = msg_send_id![&bar, bottomAnchor];
        let b_lead: Retained<NSObject> = msg_send_id![&bar, leadingAnchor];
        let b_trail: Retained<NSObject> = msg_send_id![&bar, trailingAnchor];
        let b_width: Retained<NSObject> = msg_send_id![&bar, widthAnchor];
        let b_height: Retained<NSObject> = msg_send_id![&bar, heightAnchor];

        let activate = |c: &Retained<NSObject>| {
            let _: () = msg_send![c, setActive: true];
        };
        // Pin the bar to its edge: full span along the edge, `width` thick.
        match idx {
            0 => {
                // top
                let c1: Retained<NSObject> = msg_send_id![&b_top, constraintEqualToAnchor: &*p_top];
                let c2: Retained<NSObject> = msg_send_id![&b_lead, constraintEqualToAnchor: &*p_lead];
                let c3: Retained<NSObject> = msg_send_id![&b_trail, constraintEqualToAnchor: &*p_trail];
                let c4: Retained<NSObject> = msg_send_id![&b_height, constraintEqualToConstant: width];
                activate(&c1); activate(&c2); activate(&c3); activate(&c4);
            }
            1 => {
                // right
                let c1: Retained<NSObject> = msg_send_id![&b_top, constraintEqualToAnchor: &*p_top];
                let c2: Retained<NSObject> = msg_send_id![&b_bot, constraintEqualToAnchor: &*p_bot];
                let c3: Retained<NSObject> = msg_send_id![&b_trail, constraintEqualToAnchor: &*p_trail];
                let c4: Retained<NSObject> = msg_send_id![&b_width, constraintEqualToConstant: width];
                activate(&c1); activate(&c2); activate(&c3); activate(&c4);
            }
            2 => {
                // bottom
                let c1: Retained<NSObject> = msg_send_id![&b_bot, constraintEqualToAnchor: &*p_bot];
                let c2: Retained<NSObject> = msg_send_id![&b_lead, constraintEqualToAnchor: &*p_lead];
                let c3: Retained<NSObject> = msg_send_id![&b_trail, constraintEqualToAnchor: &*p_trail];
                let c4: Retained<NSObject> = msg_send_id![&b_height, constraintEqualToConstant: width];
                activate(&c1); activate(&c2); activate(&c3); activate(&c4);
            }
            _ => {
                // left
                let c1: Retained<NSObject> = msg_send_id![&b_top, constraintEqualToAnchor: &*p_top];
                let c2: Retained<NSObject> = msg_send_id![&b_bot, constraintEqualToAnchor: &*p_bot];
                let c3: Retained<NSObject> = msg_send_id![&b_lead, constraintEqualToAnchor: &*p_lead];
                let c4: Retained<NSObject> = msg_send_id![&b_width, constraintEqualToConstant: width];
                activate(&c1); activate(&c2); activate(&c3); activate(&c4);
            }
        }
    }
}
