use runtime_core::{Color, Length, StyleRules, Tokenized};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIColor, UIView};
use std::rc::Rc;
use block2::ConcreteBlock;
use crate::phase_record;

/// Opaque wrapper for CoreGraphics' `CGColorRef` so `msg_send!`'s
/// debug-mode encoding check sees `^{CGColor=}` instead of `^v`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct CGColorRef(pub *const std::ffi::c_void);

unsafe impl Encode for CGColorRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

// `parse_color` lives in `backend_apple_core::color` now — same
// signature, same semantics. Re-exported here so the iOS-core
// public surface stays unchanged for downstream callers.
pub use backend_apple_core::color::parse_color;

pub fn color_to_uicolor(color: &Color) -> Retained<UIColor> {
    let (r, g, b, a) = parse_color(&color.0);
    unsafe { UIColor::colorWithRed_green_blue_alpha(r, g, b, a) }
}

pub fn length_to_px(len: &Length) -> CGFloat {
    match len {
        Length::Px(v) => *v as CGFloat,
        Length::Percent(_) => 0.0,
        Length::Auto => 0.0,
    }
}

pub fn font_weight_to_uikit(weight: runtime_core::FontWeight) -> CGFloat {
    match weight {
        runtime_core::FontWeight::Thin => -0.6,
        runtime_core::FontWeight::ExtraLight => -0.5,
        runtime_core::FontWeight::Light => -0.4,
        runtime_core::FontWeight::Normal => 0.0,
        runtime_core::FontWeight::Medium => 0.23,
        runtime_core::FontWeight::SemiBold => 0.3,
        runtime_core::FontWeight::Bold => 0.4,
        runtime_core::FontWeight::ExtraBold => 0.56,
        runtime_core::FontWeight::Black => 0.62,
    }
}

/// Map framework Easing to UIView animation options bitmask.
pub fn easing_to_options(easing: &runtime_core::Easing) -> u64 {
    match easing {
        runtime_core::Easing::Linear => 3 << 16,
        runtime_core::Easing::Ease | runtime_core::Easing::EaseInOut => 0 << 16,
        runtime_core::Easing::EaseIn => 1 << 16,
        runtime_core::Easing::EaseOut => 2 << 16,
        runtime_core::Easing::CubicBezier(_, _, _, _) => 0 << 16,
    }
}

/// Pick the transition to use for a property change. With the
/// active-theme concept gone, the only source of a transition is the
/// stylesheet's per-property `Transition` field — explicit opt-in.
/// Snap by default.
fn effective_transition(
    explicit: Option<&runtime_core::Transition>,
) -> Option<runtime_core::Transition> {
    explicit.copied()
}

/// Run property changes inside a UIView animation block.
pub fn animate(transition: &runtime_core::Transition, changes: Rc<dyn Fn()>) {
    let duration = transition.duration_ms as CGFloat / 1000.0;
    let options = easing_to_options(&transition.easing);
    let block = ConcreteBlock::new(move || {
        changes();
    });
    let block = block.copy();
    let nil: *const NSObject = std::ptr::null();
    unsafe {
        let _: () = msg_send![
            objc2::class!(UIView),
            animateWithDuration: duration,
            delay: 0.0 as CGFloat,
            options: options,
            animations: &*block,
            completion: nil
        ];
    }
}

/// Add (or replace) a `CAGradientLayer` sublayer that fills the
/// view's bounds with the requested gradient. The sublayer is tagged
/// with `name = "idealyst_gradient"` so a subsequent apply removes
/// the prior layer cleanly. Passing `None` removes any existing
/// gradient layer — the view falls back to its solid `backgroundColor`.
/// Install (or refresh) a `CAGradientLayer` sublayer on `view`.
/// Returns `(layer, stops_srgb)` so the caller can stash the layer
/// reference and the per-stop sRGB colors on its per-node state.
/// Per-frame `GradientStopColor` writes (in the mobile crate's
/// `set_animated_color` handler) then mutate `stops_srgb[idx]` and
/// rewrite the layer's `colors` property without rebuilding the
/// sublayer or walking the view's sublayer list.
///
/// Returns `None` if the input gradient is `None` — callers can
/// use that to clear stored state.
pub fn install_gradient(
    view: &UIView,
    gradient: Option<&runtime_core::Gradient>,
) -> Option<(Retained<NSObject>, Vec<[f32; 4]>)> {
    let g = gradient?;
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let is_metal_view: bool = unsafe {
        msg_send![&layer, isKindOfClass: objc2::class!(CAMetalLayer)]
    };
    if is_metal_view {
        return None;
    }
    let (gradient_layer, stops_srgb) = build_gradient_layer(view, &layer, g)?;
    Some((gradient_layer, stops_srgb))
}

/// Per-frame writer for `AnimProp::GradientStopColor(idx)`. Updates
/// the cached `stops[idx]` and re-applies `setColors:` on the
/// stored gradient layer — no layer walk, no sublayer rebuild.
///
/// Quietly no-ops if `idx` is out of range. Out-of-range writes
/// happen when authors animate a stop that doesn't exist (typo or
/// mid-build state); the AV system can't catch this statically.
pub fn set_animated_gradient_stop(
    layer: &NSObject,
    stops: &mut Vec<[f32; 4]>,
    idx: usize,
    value: [f32; 4],
) {
    if idx >= stops.len() {
        return;
    }
    stops[idx] = value;
    write_colors_on_layer(layer, stops);
}

fn build_gradient_layer(
    view: &UIView,
    layer: &NSObject,
    g: &runtime_core::Gradient,
) -> Option<(Retained<NSObject>, Vec<[f32; 4]>)> {
    // Remove any previously-installed `idealyst_gradient` sublayer
    // so a re-apply doesn't stack layers. We pay the sublayer walk
    // only when there IS a gradient — the common no-gradient path
    // skips this entirely (caller short-circuits before calling).
    unsafe {
        let sublayers_ptr: *mut NSObject = msg_send![layer, sublayers];
        if !sublayers_ptr.is_null() {
            let count: usize = msg_send![sublayers_ptr, count];
            for i in 0..count {
                let sub_ptr: *mut NSObject =
                    msg_send![sublayers_ptr, objectAtIndex: i];
                if sub_ptr.is_null() {
                    continue;
                }
                let name_ptr: *mut objc2_foundation::NSString =
                    msg_send![sub_ptr, name];
                if name_ptr.is_null() {
                    continue;
                }
                let name_ref = &*name_ptr;
                if name_ref.to_string() == "idealyst_gradient" {
                    let _: () = msg_send![sub_ptr, removeFromSuperlayer];
                }
            }
        }
    }

    // Sort stops by ascending offset, then snapshot the resolved
    // sRGB colors so the caller can keep mutating them per frame.
    let mut stops = g.stops.clone();
    stops.sort_by(|a, b| {
        a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal)
    });
    let stops_srgb: Vec<[f32; 4]> = stops.iter().map(|s| color_to_srgb(&s.color)).collect();

    let gradient_class = objc2::class!(CAGradientLayer);
    let gradient_layer: Retained<NSObject> =
        unsafe { msg_send_id![gradient_class, layer] };

    // CGColor list — `setColors:` takes NSArray of CGColorRef.
    write_colors_on_layer(&gradient_layer, &stops_srgb);

    // Locations — NSArray of NSNumber(double).
    let locations_array: Retained<NSObject> = unsafe {
        let arr: Retained<NSObject> =
            msg_send_id![objc2::class!(NSMutableArray), array];
        for stop in &stops {
            let n: Retained<NSObject> = msg_send_id![
                objc2::class!(NSNumber),
                numberWithDouble: stop.offset.clamp(0.0, 1.0) as f64
            ];
            let _: () = msg_send![&arr, addObject: &*n];
        }
        arr
    };
    let _: () = unsafe { msg_send![&gradient_layer, setLocations: &*locations_array] };

    // Linear vs. radial setup.
    match g.kind {
        runtime_core::GradientKind::Linear { angle_deg } => {
            // Convert the framework's CSS-style angle (0° = bottom→top,
            // clockwise) into CAGradientLayer start/end points in
            // unit-square coords (0,0 = top-left, 1,1 = bottom-right).
            let theta_rad = (angle_deg as f64).to_radians();
            let dx = theta_rad.sin();
            let dy = -theta_rad.cos();
            let start = objc2_foundation::CGPoint {
                x: 0.5 - dx * 0.5,
                y: 0.5 - dy * 0.5,
            };
            let end = objc2_foundation::CGPoint {
                x: 0.5 + dx * 0.5,
                y: 0.5 + dy * 0.5,
            };
            let axial = objc2_foundation::NSString::from_str("axial");
            let _: () = unsafe { msg_send![&gradient_layer, setType: &*axial] };
            let _: () = unsafe { msg_send![&gradient_layer, setStartPoint: start] };
            let _: () = unsafe { msg_send![&gradient_layer, setEndPoint: end] };
        }
        runtime_core::GradientKind::Radial { center, radius, extent } => {
            // CAGradientLayer's `radial` type uses `startPoint` as
            // the center and `endPoint` as a point at the outermost
            // stop. The gradient is parametrised elliptically with
            // unit-square semi-axes equal to `endPoint - startPoint`.
            // In pixel space the semi-axes scale by W and H
            // independently, so the same unit-square offset produces
            // an ellipse stretched to the view's aspect ratio.
            //
            // For `ClosestSide` we want the offset-1.0 contour to
            // reach the closest-edge midpoint — semi-axes
            // (radius*0.5, radius*0.5) in unit-square coords. At
            // `radius: 1.0` the ellipse passes through all four edge
            // midpoints in unit-square (which is also the closest
            // pixel edge midpoint when the box is square; on
            // non-square boxes both axes match an inscribed ellipse
            // — a slight asymmetry that matches CSS's default
            // "ellipse closest-side" sizing).
            //
            // For `FarthestCorner` we want the offset-1.0 contour
            // to pass through all four corners. Setting the
            // unit-square semi-axes to `radius*0.707` makes that
            // happen for *any* aspect ratio:
            //     t at corner = √((0.5/0.707)² + (0.5/0.707)²)
            //                 = √(0.5 + 0.5) = 1.0
            // — corners always land at the last stop, edge
            // midpoints land at 0.707. Same shape CSS produces with
            // `radial-gradient(ellipse farthest-corner, ...)`.
            let radial = objc2_foundation::NSString::from_str("radial");
            let _: () = unsafe { msg_send![&gradient_layer, setType: &*radial] };
            let start = objc2_foundation::CGPoint {
                x: center.0 as f64,
                y: center.1 as f64,
            };
            let axis_offset = match extent {
                runtime_core::RadialExtent::ClosestSide => radius * 0.5,
                runtime_core::RadialExtent::FarthestCorner => radius * std::f32::consts::FRAC_1_SQRT_2,
            };
            let end = objc2_foundation::CGPoint {
                x: (center.0 + axis_offset) as f64,
                y: (center.1 + axis_offset) as f64,
            };
            let _: () = unsafe { msg_send![&gradient_layer, setStartPoint: start] };
            let _: () = unsafe { msg_send![&gradient_layer, setEndPoint: end] };
        }
    }

    // Tag with a name so the iOS backend's layout pass can find this
    // sublayer later (and resize its frame to match the parent view's
    // bounds — see `imp/mod.rs::sync_gradient_sublayer`). CALayer's
    // `autoresizingMask` is documented on iOS but in practice doesn't
    // auto-resize sublayers without a `CAConstraintLayoutManager`, so
    // we drive the resize explicitly.
    let marker = objc2_foundation::NSString::from_str("idealyst_gradient");
    let _: () = unsafe { msg_send![&gradient_layer, setName: &*marker] };

    // Set the initial frame to the view's current bounds (typically
    // 0×0 at apply-time — Taffy resizes the view later, and the
    // layout pass mirrors that resize onto this sublayer).
    let bounds: objc2_foundation::CGRect = unsafe { msg_send![view, bounds] };
    let _: () = unsafe { msg_send![&gradient_layer, setFrame: bounds] };
    let _: () = unsafe { msg_send![&gradient_layer, setNeedsDisplayOnBoundsChange: true] };

    // Insert at index 0 — below any author-managed sublayers but
    // above the view's solid `backgroundColor` fill.
    let _: () =
        unsafe { msg_send![layer, insertSublayer: &*gradient_layer, atIndex: 0u32] };

    Some((gradient_layer, stops_srgb))
}

/// Write `stops` onto the gradient layer's `colors` property. The
/// caller owns the stops Vec — this just builds an NSArray of
/// `CGColor`s and hands it to the layer. Called from
/// `build_gradient_layer` (initial apply) AND
/// `set_animated_gradient_stop` (per-frame writes).
fn write_colors_on_layer(layer: &NSObject, stops: &[[f32; 4]]) {
    unsafe {
        let arr: Retained<NSObject> =
            msg_send_id![objc2::class!(NSMutableArray), array];
        for c in stops {
            let ui = srgb_to_uicolor(*c);
            let cg: CGColorRef = msg_send![&ui, CGColor];
            // CGColorRef is a CFTypeRef pointer; the array wants an
            // Objective-C `id`. Cast through `*mut NSObject` so the
            // `msg_send!` debug encoding check sees `@` instead of
            // `^v` — Cocoa bridges CGColor as id-shaped at runtime,
            // but the static encoding check doesn't know that.
            let id_ptr = cg.0 as *mut NSObject;
            let _: () = msg_send![&arr, addObject: id_ptr];
        }
        let _: () = msg_send![layer, setColors: &*arr];
    }
}

/// Resolve a `runtime_core::Color` to sRGB `[r, g, b, a]` in
/// `0..=1`. Mirrors `color_to_uicolor`'s parsing but skips the
/// UIColor construction — useful for caching colors in animation
/// state and rebuilding the UIColor on each apply.
fn color_to_srgb(color: &Color) -> [f32; 4] {
    runtime_core::color::parse_or(&color.0, runtime_core::color::Rgba::BLACK).to_srgb_f32()
}

/// Inverse of `color_to_srgb`: build a `UIColor` from sRGB floats.
/// Used per-frame in `set_animated_gradient_stop` to convert the
/// stored stop color back into a CGColor for the layer.
fn srgb_to_uicolor(c: [f32; 4]) -> Retained<UIColor> {
    unsafe {
        UIColor::colorWithRed_green_blue_alpha(
            c[0] as CGFloat,
            c[1] as CGFloat,
            c[2] as CGFloat,
            c[3] as CGFloat,
        )
    }
}

/// Sync a view's `cornerRadius` against its current bounds. Called
/// from the iOS backend's layout pass — `apply_style_to_view` clamps
/// against `style.width` / `style.height` when they're explicit
/// pixels, but for percentage-sized views the px values aren't known
/// until layout completes. In that case we stash the requested
/// radius as an associated value on the layer and re-apply it here
/// with a proper clamp.
///
/// UIKit's `cornerRadius` is NOT clamped to the layer's bounds —
/// setting it above `min(W, H) / 2` renders nothing at all, so this
/// clamp is mandatory for the CSS-idiomatic `border-radius: 999px`
/// (a.k.a. "make it a circle") to work for percent-sized views.
pub fn sync_corner_radius(view: &UIView) {
    unsafe {
        let layer: Retained<NSObject> = msg_send_id![view, layer];
        // Read the stashed requested radius (set by apply_style when
        // it couldn't clamp). `valueForKey:` returns nil for missing
        // associations.
        let key = objc2_foundation::NSString::from_str("idealyst_requested_corner_radius");
        let value_ptr: *mut NSObject = msg_send![&layer, valueForKey: &*key];
        if value_ptr.is_null() {
            return;
        }
        let value: &NSObject = &*value_ptr;
        let requested: f64 = msg_send![value, doubleValue];
        if requested <= 0.0 {
            return;
        }
        let bounds: objc2_foundation::CGRect = msg_send![view, bounds];
        let half_w = bounds.size.width / 2.0;
        let half_h = bounds.size.height / 2.0;
        let cap = half_w.min(half_h);
        let effective = requested.min(cap.max(0.0));
        let _: () = msg_send![&layer, setCornerRadius: effective];
    }
}

/// Sync a view's `idealyst_gradient` CAGradientLayer (if present) to
/// the view's current bounds. Called from the iOS backend's layout
/// pass — every view has its bounds rewritten by Taffy after build,
/// but `CALayer.autoresizingMask` doesn't drive automatic sublayer
/// resizing on iOS in practice, so the layout pass mirrors the
/// resize explicitly. No-op for views with no gradient sublayer.
pub fn sync_gradient_sublayer(view: &UIView) {
    unsafe {
        let layer: Retained<NSObject> = msg_send_id![view, layer];
        let sublayers_ptr: *mut NSObject = msg_send![&layer, sublayers];
        if sublayers_ptr.is_null() {
            return;
        }
        let count: usize = msg_send![sublayers_ptr, count];
        if count == 0 {
            return;
        }
        for i in 0..count {
            let sub_ptr: *mut NSObject = msg_send![sublayers_ptr, objectAtIndex: i];
            if sub_ptr.is_null() {
                continue;
            }
            let name_ptr: *mut objc2_foundation::NSString = msg_send![sub_ptr, name];
            if name_ptr.is_null() {
                continue;
            }
            let name_ref = &*name_ptr;
            if name_ref.to_string() == "idealyst_gradient" {
                let bounds: objc2_foundation::CGRect = msg_send![view, bounds];
                let _: () = msg_send![sub_ptr, setFrame: bounds];
            }
        }
    }
}

/// Center (and scale-to-fit) a view's `idealyst_icon` CAShapeLayer
/// within the view's current bounds. Called from the layout pass for
/// the same reason as [`sync_gradient_sublayer`]: the icon's shape layer
/// is built at a fixed 24×24 top-left origin, but flex layout may size
/// the icon view larger than the glyph (cross-axis stretch in a row, or
/// centered inside a bigger pressable like a menu button). Without this
/// the glyph hugs the top-left corner instead of sitting centered.
///
/// The layer keeps its 24×24 path-space `bounds`; we move its `position`
/// to the view center (anchorPoint is the default 0.5,0.5) so the glyph
/// sits centered no matter how flex sized the icon view. No-op for views
/// with no icon sublayer or zero bounds (pre-layout).
pub fn sync_icon_sublayer(view: &UIView) {
    unsafe {
        let bounds: CGRect = msg_send![view, bounds];
        let (w, h) = (bounds.size.width, bounds.size.height);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let layer: Retained<NSObject> = msg_send_id![view, layer];
        let sublayers_ptr: *mut NSObject = msg_send![&layer, sublayers];
        if sublayers_ptr.is_null() {
            return;
        }
        let count: usize = msg_send![sublayers_ptr, count];
        for i in 0..count {
            let sub_ptr: *mut NSObject = msg_send![sublayers_ptr, objectAtIndex: i];
            if sub_ptr.is_null() {
                continue;
            }
            let name_ptr: *mut objc2_foundation::NSString = msg_send![sub_ptr, name];
            if name_ptr.is_null() {
                continue;
            }
            if (&*name_ptr).to_string() != "idealyst_icon" {
                continue;
            }
            // Center the 24×24 path-space layer at the view's midpoint.
            let center = CGPoint { x: w / 2.0, y: h / 2.0 };
            let _: () = msg_send![sub_ptr, setPosition: center];
        }
    }
}

pub fn apply_style_to_view(view: &UIView, style: &StyleRules) {
    let _t = phase_record::scope("apply_style_to_view");
    // Background color -- skip for Metal-backed views
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let is_metal_view: bool = unsafe {
        msg_send![&layer, isKindOfClass: objc2::class!(CAMetalLayer)]
    };
    if let Some(bg) = &style.background {
        if !is_metal_view {
            // `.resolve()` reads through the per-token registry and
            // subscribes the enclosing apply-style Effect to the
            // referenced token's signal. Token swaps re-fire only the
            // nodes that referenced the changed token.
            let bg_val = bg.resolve();
            let c = color_to_uicolor(&bg_val);
            // Choose between snap, per-component CSS transition, or
            // the global theme-transition default. The
            // `effective_transition` helper handles the precedence:
            // explicit > theme > snap.
            match effective_transition(style.background_transition.as_ref()) {
                Some(trans) => {
                    let view_ref: Retained<UIView> = unsafe {
                        Retained::retain(view as *const UIView as *mut UIView).unwrap()
                    };
                    let c2 = c.clone();
                    animate(&trans, Rc::new(move || {
                        view_ref.setBackgroundColor(Some(&c2));
                    }));
                }
                None => {
                    view.setBackgroundColor(Some(&c));
                }
            }
        }
    }

    // Gradient is handled outside this function — the mobile (and TV)
    // crates orchestrate it because they own the per-node state map
    // that the per-frame `set_animated_color(GradientStopColor)` path
    // needs to reach. See `install_gradient` below.

    // Flex direction, gap, justify_content, align_items, etc. are
    // ALL handled by Taffy now. They flow through
    // `LayoutTree::set_style` → Taffy's flex engine → frame
    // assignment via `apply_frames`. We deliberately do NOT forward
    // them to any UIView property: legacy backends used UIStackView
    // here, but UIStackView's own constraints conflict with Taffy's
    // frame writes (UISV-canvas-connection forces sizes Taffy didn't
    // choose). The framework's flex semantics live entirely in
    // runtime-layout.

    // Opacity
    if let Some(opacity) = style.opacity.as_ref().map(|t| t.resolve()) {
        if let Some(trans) = &style.opacity_transition {
            let view_ref: Retained<UIView> = unsafe { Retained::retain(view as *const UIView as *mut UIView).unwrap() };
            let trans = *trans;
            animate(&trans, Rc::new(move || {
                unsafe { view_ref.setAlpha(opacity as CGFloat) };
            }));
        } else {
            unsafe { view.setAlpha(opacity as CGFloat) };
        }
    }

    // Corner radius. Clamp to half the view's explicit width/height if
    // either is supplied — UIKit's `cornerRadius` is NOT clamped to
    // the view's dimensions, so the CSS-idiomatic `border-radius:
    // 999px` (meaning "make it a circle") makes the layer render
    // *nothing at all* on a 200pt-wide view. Clamping here lets the
    // 999 idiom work the way authors expect.
    let radius = crate::style_diff::requested_corner_radius_px(style);
    if radius > 0.0 {
        // The clamp source, in precedence order (see
        // `style_diff::resolve_corner_radius`):
        //   1. explicit px width/height in the style (stable across
        //      relayout) → clamp now.
        //   2. else the view's CURRENT laid-out bounds — clamp now if
        //      it already has real bounds. This is the fix for the
        //      "rounded corner goes square after any button press" bug:
        //      a reactive paint-only re-style produces NO frame change,
        //      so `apply_frames`' frame-key cache skips this view and
        //      `sync_corner_radius` never re-fires. Reading live bounds
        //      here lets the radius survive the re-style without
        //      depending on a layout pass that won't run.
        //   3. else (pre-first-layout, bounds 0×0) defer to the layout
        //      pass via the stashed NSNumber + `sync_corner_radius`.
        //
        // We ALWAYS stash the requested value (every branch), so a
        // later resize — which DOES change the frame and therefore runs
        // `sync_corner_radius` — re-clamps against the new bounds.
        // Invariant project_ios_cornerradius_unclamped: an unclamped
        // `cornerRadius > min(w, h)/2` makes the layer render nothing.
        let px_cap = crate::style_diff::px_cap_from_style(style);
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let bounds_min_half = {
            let m = bounds.size.width.min(bounds.size.height) / 2.0;
            if m > 0.0 { Some(m) } else { None }
        };

        // Stash the requested radius on the layer so `sync_corner_radius`
        // can re-clamp on a later resize, independent of which branch
        // we take below.
        let key = objc2_foundation::NSString::from_str(
            "idealyst_requested_corner_radius",
        );
        let cls = objc2::class!(NSNumber);
        let number: *mut NSObject = unsafe {
            msg_send![cls, numberWithDouble: radius]
        };
        let _: () = unsafe {
            msg_send![&layer, setValue: number, forKey: &*key]
        };

        match crate::style_diff::resolve_corner_radius(radius, px_cap, bounds_min_half) {
            crate::style_diff::CornerRadiusDecision::Apply(effective) => {
                let _: () = unsafe { msg_send![&layer, setCornerRadius: effective] };
            }
            crate::style_diff::CornerRadiusDecision::Defer(_) => {
                // Bounds still 0×0 and no px clamp. Set 0 in the
                // meantime so we don't render the "999 on a 0×0 view →
                // blank" state; `sync_corner_radius` clamps once Taffy
                // assigns a real frame.
                let _: () = unsafe { msg_send![&layer, setCornerRadius: 0.0_f64] };
            }
            crate::style_diff::CornerRadiusDecision::None => {}
        }
        unsafe { view.setClipsToBounds(true) };
    }

    // Border. The framework exposes a CSS-style per-side API
    // (`border_{top,right,bottom,left}_{width,color}`), but the two
    // rendering mechanisms available on UIKit have a sharp split:
    //
    //   * `CALayer.borderWidth`/`borderColor` strokes ONE uniform
    //     border that follows the layer's `cornerRadius` exactly —
    //     the stroke curves around rounded corners with no seams.
    //   * Per-side `UIView` bars can express asymmetric widths/colors,
    //     but each bar is a straight rectangle. With a corner radius
    //     the parent's `clipsToBounds` rounded-corner mask slices the
    //     ends off every bar, leaving notches/gaps at each corner —
    //     a straight bar can't trace a curve. See the regression test
    //     `regression_ios_uniform_rounded_border_uses_calayer`.
    //
    // So: when the border is *uniform* (all four sides the same width
    // and the same effective color) route it through CALayer, which is
    // both simpler and the only path that renders rounded corners
    // cleanly. Fall back to per-side bars only for the genuinely
    // asymmetric case that CALayer can't represent.
    let widths = [
        style.border_top_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        style.border_right_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        style.border_bottom_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        style.border_left_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
    ];
    // Tear down any previous per-side border subviews so reapplies
    // (state overlays, theme swap) replace rather than stack them.
    remove_border_subviews(view);
    let any_width = widths.iter().any(|w| *w > 0.0);
    if any_width {
        let colors: [Option<Color>; 4] = [
            style.border_top_color.as_ref().map(|t| t.resolve()),
            style.border_right_color.as_ref().map(|t| t.resolve()),
            style.border_bottom_color.as_ref().map(|t| t.resolve()),
            style.border_left_color.as_ref().map(|t| t.resolve()),
        ];
        if let Some((width, color)) = crate::border::uniform_border(widths, &colors) {
            // Uniform border → CALayer stroke. Follows `cornerRadius`
            // (set above, or synced later for percent-sized views)
            // with no corner seams.
            let ui_color = color_to_uicolor(&color);
            let cg: CGColorRef = unsafe { msg_send![&ui_color, CGColor] };
            if !cg.0.is_null() {
                let _: () = unsafe { msg_send![&layer, setBorderColor: cg] };
            }
            let _: () =
                unsafe { msg_send![&layer, setBorderWidth: width as f64] };
        } else {
            // Asymmetric border → per-side bars. Clear any CALayer
            // stroke a prior uniform apply may have left, then paint
            // each non-zero side. (Rounded corners with an asymmetric
            // border remain imperfect — the bars are straight — but
            // this case has no clean UIKit primitive and is vanishingly
            // rare; the common uniform card takes the branch above.)
            let _: () = unsafe { msg_send![&layer, setBorderWidth: 0.0_f64] };
            let fallback_color = colors.iter().find_map(|c| c.clone());
            let parent_bounds: CGRect = unsafe { msg_send![view, bounds] };
            for (idx, &w) in widths.iter().enumerate() {
                if w <= 0.0 {
                    continue;
                }
                let Some(color) = colors[idx].clone().or_else(|| fallback_color.clone())
                else {
                    continue;
                };
                install_border_side(view, idx, w as CGFloat, &color, parent_bounds);
            }
        }
    } else {
        // No border requested — clear any CALayer stroke a prior apply
        // (or a `Card` SDK call touching the layer directly) may have
        // left, so it doesn't paint a ghost frame.
        let _: () = unsafe { msg_send![&layer, setBorderWidth: 0.0_f64] };
    }

    // Shadow
    if let Some(shadow) = &style.shadow {
        let shadow_color = color_to_uicolor(&shadow.color);
        let cg: CGColorRef = unsafe { msg_send![&shadow_color, CGColor] };
        if !cg.0.is_null() {
            let _: () = unsafe { msg_send![&layer, setShadowColor: cg] };
        }
        let offset = CGSize {
            width: shadow.x as CGFloat,
            height: shadow.y as CGFloat,
        };
        let _: () = unsafe { msg_send![&layer, setShadowOffset: offset] };
        let _: () = unsafe { msg_send![&layer, setShadowRadius: (shadow.blur as CGFloat / 2.0)] };
        let _: () = unsafe { msg_send![&layer, setShadowOpacity: 1.0_f32] };
        unsafe { view.setClipsToBounds(false) };
    }

    // Padding is handled entirely by Taffy for container views
    // (writes into the node's `padding` Rect, which insets the
    // content area inside the view's frame). We don't forward to
    // setLayoutMargins because UIView's layoutMargins are only
    // consulted by UIStackView's `layoutMarginsRelativeArrangement`,
    // which we no longer use.
    //
    // UILabel is the exception: it has no children to inset, so
    // Taffy padding would grow the label's outer frame without
    // pushing the glyphs in. `IdealystLabel` (backend-ios-mobile's
    // UILabel subclass) carries a per-side `textInsets` ivar that
    // its overridden `drawText(in:)` / `sizeThatFits:` honor; copy
    // the style's padding values into it via the obj-c runtime so
    // this `core` crate doesn't need to depend on the mobile crate.
    apply_text_insets_if_label(view, style);

    // Overflow
    if let Some(overflow) = &style.overflow {
        match overflow {
            runtime_core::Overflow::Hidden => unsafe { view.setClipsToBounds(true) },
            runtime_core::Overflow::Visible => unsafe { view.setClipsToBounds(false) },
        }
    }

    // Width / height: owned entirely by Taffy. Authors' explicit
    // `width` / `height` flow through `translate_style` into Taffy's
    // `size`, then Taffy writes `view.frame` via `apply_frames`. We
    // do NOT install Auto Layout constraints here — the goal of the
    // Taffy migration is to make UIView's Auto Layout system
    // redundant for framework-managed views.
}

pub fn apply_text_style(
    view: &UIView,
    style: &StyleRules,
    is_label: bool,
    font_registry: &crate::font::FontRegistry,
) {
    // Text color: same precedence as background (explicit > theme
    // transition default > snap).
    //
    // For display text (labels, button titles — `is_label`), an ABSENT
    // color must NOT fall back to UIKit's default `labelColor`: that
    // color tracks the OS appearance and is *white* in dark mode, so an
    // app installing a light theme would show invisible white text over
    // a light surface on a dark-mode device (the ranking-option / mood-
    // chip bug). Instead we resolve the installed theme's `color-text`
    // token through the SAME `Tokenized<Color>::resolve()` path an
    // authored `color:` uses, so the value matches web + macOS exactly
    // (CLAUDE.md §7). Explicit colors still win — see
    // `style_diff::effective_text_color`. Editable widgets (TextField /
    // TextView, `is_label = false`) keep their native default until the
    // author sets a color.
    let resolved_color = if is_label {
        Some(crate::style_diff::effective_text_color(style.color.as_ref()).resolve())
    } else {
        style.color.as_ref().map(|color| color.resolve())
    };
    if let Some(color_val) = resolved_color {
        let c = color_to_uicolor(&color_val);
        match effective_transition(style.color_transition.as_ref()) {
            Some(trans) => {
                let view_ref: Retained<UIView> = unsafe {
                    Retained::retain(view as *const UIView as *mut UIView).unwrap()
                };
                animate(&trans, Rc::new(move || {
                    let _: () = unsafe { msg_send![&view_ref, setTextColor: &*c] };
                }));
            }
            None => {
                let _: () = unsafe { msg_send![view, setTextColor: &*c] };
            }
        }
    }

    // Font: route family + weight + style + size through the font
    // registry first. Falls back to the system-font path below if no
    // custom typeface applies. Apply only when the author actually
    // set a typography knob — leaving every untyped view's default
    // system font alone matches the prior behavior.
    let has_typography = style.font_family.is_some()
        || style.font_size.is_some()
        || style.font_weight.is_some()
        || style.font_style.is_some();
    if has_typography {
        let weight = style
            .font_weight
            .as_ref()
            .copied()
            .unwrap_or(runtime_core::FontWeight::Normal);
        let fstyle = style
            .font_style
            .as_ref()
            .copied()
            .unwrap_or(runtime_core::FontStyle::Normal);
        let size = match style.font_size.as_ref().map(|t| t.resolve()) {
            Some(len) => {
                let px = length_to_px(&len);
                if px > 0.0 { px } else { 17.0 as CGFloat }
            }
            None => 17.0 as CGFloat,
        };
        let applied = crate::font::apply_resolved_font(
            view,
            font_registry,
            style.font_family.as_ref(),
            weight,
            fstyle,
            size,
        );
        if !applied {
            let ui_weight = font_weight_to_uikit(weight);
            let font: Retained<NSObject> = unsafe {
                msg_send_id![
                    objc2::class!(UIFont),
                    systemFontOfSize: size,
                    weight: ui_weight
                ]
            };
            let _: () = unsafe { msg_send![view, setFont: &*font] };
        }
    }

    // Text alignment
    if let Some(ta) = &style.text_align {
        let align: isize = match ta {
            runtime_core::TextAlign::Left => 0,
            runtime_core::TextAlign::Center => 1,
            runtime_core::TextAlign::Right => 2,
            runtime_core::TextAlign::Justify => 3,
        };
        let _: () = unsafe { msg_send![view, setTextAlignment: align] };
    }

    // Number of lines = 0 for wrapping (UILabel only). Also pin
    // lineBreakMode to byWordWrapping (= 0) so wrapping happens
    // instead of mid-line ellipsis when the assigned frame is a
    // hair narrower than the text wants (rounding off `sizeThatFits:`).
    if is_label {
        let _: () = unsafe { msg_send![view, setNumberOfLines: 0isize] };
        let _: () = unsafe { msg_send![view, setLineBreakMode: 0isize] };
    }
}

// ==========================================================================
// Per-side border subviews
// ==========================================================================
//
// CALayer's `borderWidth` / `borderColor` are uniform — they stroke
// all four sides with the same value. The framework's `StyleRules`
// expose `border_{top,right,bottom,left}_{width,color}` independently
// (matching CSS), so a one-side rule like `border_bottom_width: 1.0`
// can't ride the CALayer path without spilling the stroke onto the
// other three sides. We install a thin UIView for each side that
// the author opted into, tagged by accessibility identifier so
// subsequent style applies can find and rebuild them.
//
// Autoresizing keeps the side views pinned to their respective edges
// when Taffy resizes the parent: flexible width for top/bottom,
// flexible height for left/right, plus the opposite-side margin so
// the bar tracks its anchor edge.

const BORDER_ID_TOP: &str = "idealyst_border_top";
const BORDER_ID_RIGHT: &str = "idealyst_border_right";
const BORDER_ID_BOTTOM: &str = "idealyst_border_bottom";
const BORDER_ID_LEFT: &str = "idealyst_border_left";

fn border_id_for(idx: usize) -> &'static str {
    match idx {
        0 => BORDER_ID_TOP,
        1 => BORDER_ID_RIGHT,
        2 => BORDER_ID_BOTTOM,
        _ => BORDER_ID_LEFT,
    }
}

// ==========================================================================
// IdealystLabel padding adapter
// ==========================================================================
//
// Detect whether `view` is an `IdealystLabel` (the UILabel subclass
// declared in `backend-ios-mobile::imp::text_inset`) via the
// obj-c-runtime class lookup, and if so, write the style's padding
// values into its `text_insets` ivar by sending `setTextInsets:`.
// We avoid a Rust-level dep on the mobile crate by reaching through
// the runtime — the class name + the setter selector are the only
// surface we need.

/// `setTextInsets:` payload — matches `UIEdgeInsets` (top, left,
/// bottom, right) per Apple's struct definition. The packing order
/// matters: `objc2::msg_send!` writes the fields in declaration
/// order into the ABI's argument frame, and a transposed pair
/// renders padding on the wrong edges.
#[repr(C)]
#[derive(Clone, Copy)]
struct UIEdgeInsetsPayload {
    top: CGFloat,
    left: CGFloat,
    bottom: CGFloat,
    right: CGFloat,
}

unsafe impl Encode for UIEdgeInsetsPayload {
    const ENCODING: Encoding = Encoding::Struct(
        "UIEdgeInsets",
        &[CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING],
    );
}

/// Memoized class lookup so the obj-c-runtime probe runs once for
/// the whole process instead of allocating a `CString` and querying
/// `objc_lookUpClass` on every `apply_style` call (hot path: fires
/// once per styled view during screen mount, and screen mount is
/// what the website does for every nav-link tap).
///
/// `AtomicPtr` instead of `OnceLock<Option<usize>>` so we cache ONLY
/// successful lookups. The `IdealystLabel` class is registered with
/// the obj-c runtime lazily — `declare_class!` (in `backend-ios-
/// mobile`) emits a `class()` impl that calls `objc_allocateClassPair`
/// on first reference, which happens the first time `create_text`
/// runs. `apply_style` runs on container views before any text leaf
/// exists in the tree, so the first `apply_text_insets_if_label` call
/// can fire BEFORE any `IdealystLabel` is alive — and the obj-c
/// runtime returns NULL because the class doesn't yet exist. A
/// `OnceLock` would cache that NULL forever, leaving every subsequent
/// text node without its insets. The atomic-pointer pattern below
/// retries until the first success and only then memoizes.
static IDEALYST_LABEL_CLASS: std::sync::atomic::AtomicPtr<objc2::runtime::AnyClass> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

fn idealyst_label_class() -> Option<&'static objc2::runtime::AnyClass> {
    use std::sync::atomic::Ordering;
    let cached = IDEALYST_LABEL_CLASS.load(Ordering::Relaxed);
    if !cached.is_null() {
        return Some(unsafe { &*cached });
    }
    let name = std::ffi::CString::new("IdealystLabel").ok()?;
    let p = unsafe { objc2::ffi::objc_lookUpClass(name.as_ptr()) };
    if p.is_null() {
        return None;
    }
    IDEALYST_LABEL_CLASS.store(p as *mut _, Ordering::Relaxed);
    Some(unsafe { &*(p as *const objc2::runtime::AnyClass) })
}

fn apply_text_insets_if_label(
    view: &UIView,
    style: &runtime_core::StyleRules,
) {
    let Some(cls_ref) = idealyst_label_class() else {
        return;
    };
    let is_match: bool = unsafe { msg_send![view, isKindOfClass: cls_ref] };
    if !is_match {
        return;
    }

    // Resolve each side's padding. `None` reads as zero. Resolving
    // the `Tokenized` inside the wrapping apply-style Effect
    // subscribes to the token signal automatically, so a theme swap
    // that changes a padding token re-fires this branch with the
    // new value.
    let resolve_side = |t: Option<&Tokenized<Length>>| -> f32 {
        t.map(|tok| match tok.resolve() {
            Length::Px(px) => px,
            // Percent paddings on a UILabel don't have a defined
            // sizing parent (the label is a leaf), so fall back to
            // zero rather than guessing.
            _ => 0.0,
        })
        .unwrap_or(0.0)
    };
    let insets = UIEdgeInsetsPayload {
        top: resolve_side(style.padding_top.as_ref()) as CGFloat,
        left: resolve_side(style.padding_left.as_ref()) as CGFloat,
        bottom: resolve_side(style.padding_bottom.as_ref()) as CGFloat,
        right: resolve_side(style.padding_right.as_ref()) as CGFloat,
    };
    let _: () = unsafe { msg_send![view, setTextInsets: insets] };
}

fn is_border_id(s: &str) -> bool {
    matches!(s, BORDER_ID_TOP | BORDER_ID_RIGHT | BORDER_ID_BOTTOM | BORDER_ID_LEFT)
}

/// Sentinel value we write into `UIView.tag` whenever
/// `install_border_side` installs at least one border subview. The
/// next `apply_style` consult flips back to the slow path (walk
/// subviews, check accessibility identifiers, removeFromSuperview)
/// only when this tag is present — for the vast majority of styled
/// views the author never asked for a border, so the previous
/// unconditional walk burned a `view.subviews()` allocation and an
/// N-pass identifier compare for every node every time the style
/// effect fired (5 k microseconds across the 100-view trees the
/// website ships). The value is arbitrary but distinctive enough to
/// not collide with author-set tag values in real apps.
const BORDER_TAG_MARKER: isize = 0x0BDE_7A60;

fn remove_border_subviews(view: &UIView) {
    let tag: isize = unsafe { msg_send![view, tag] };
    if tag != BORDER_TAG_MARKER {
        // Fast path: we've never installed border subviews on this
        // view, so there's nothing to tear down. Saves an
        // `NSArray *subviews` alloc and an O(N_children) identifier
        // walk per `apply_style` call.
        return;
    }
    let _t = phase_record::scope("remove_border_subviews");
    let subviews = view.subviews();
    for sub in subviews.iter() {
        let id_obj: Option<Retained<NSString>> = unsafe {
            msg_send_id![sub, accessibilityIdentifier]
        };
        let is_border = id_obj
            .as_deref()
            .map(|s| is_border_id(&s.to_string()))
            .unwrap_or(false);
        if is_border {
            unsafe { sub.removeFromSuperview() };
        }
    }
    // Clear the marker — if the next `apply_style` re-installs
    // borders it'll set it again, otherwise subsequent calls
    // short-circuit via the fast path above.
    let _: () = unsafe { msg_send![view, setTag: 0isize] };
}

fn install_border_side(
    view: &UIView,
    idx: usize,
    width: CGFloat,
    color: &Color,
    _parent_bounds: CGRect,
) {
    let _t = phase_record::scope("install_border_side");
    // apply_style runs on the main thread per framework contract.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let bar = unsafe { UIView::new(mtm) };
    let ui_color = color_to_uicolor(color);
    bar.setBackgroundColor(Some(&ui_color));
    let _: () = unsafe { msg_send![&bar, setUserInteractionEnabled: false] };
    let id_str = NSString::from_str(border_id_for(idx));
    let _: () = unsafe { msg_send![&bar, setAccessibilityIdentifier: &*id_str] };

    // Auto Layout, not frame + autoresizingMask. `apply_style` runs
    // before Taffy assigns the parent's frame on initial mount, so a
    // frame-based bar would be installed at (0, 0, 0, 0) and never
    // grow — autoresizing only scales existing extents proportionally
    // from a zero base, leaving the border invisible. Pinning to the
    // parent's anchors makes UIKit recompute on every layout pass.
    let _: () = unsafe {
        msg_send![&bar, setTranslatesAutoresizingMaskIntoConstraints: false]
    };
    unsafe { view.addSubview(&bar) };
    // Mark the parent so the next `remove_border_subviews` knows it
    // can't take the fast path. Setting the tag once per
    // `install_border_side` is cheap (a single message send);
    // toggling it via author-set tag values is the corner case the
    // sentinel constant is picked to avoid.
    let _: () = unsafe { msg_send![view, setTag: BORDER_TAG_MARKER] };
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

        match idx {
            0 => {
                // top: full width pinned to parent top
                let c1: Retained<NSObject> =
                    msg_send_id![&b_top, constraintEqualToAnchor: &*p_top];
                let c2: Retained<NSObject> =
                    msg_send_id![&b_lead, constraintEqualToAnchor: &*p_lead];
                let c3: Retained<NSObject> =
                    msg_send_id![&b_trail, constraintEqualToAnchor: &*p_trail];
                let c4: Retained<NSObject> =
                    msg_send_id![&b_height, constraintEqualToConstant: width];
                activate(&c1);
                activate(&c2);
                activate(&c3);
                activate(&c4);
            }
            1 => {
                // right: full height pinned to parent trailing
                let c1: Retained<NSObject> =
                    msg_send_id![&b_top, constraintEqualToAnchor: &*p_top];
                let c2: Retained<NSObject> =
                    msg_send_id![&b_bot, constraintEqualToAnchor: &*p_bot];
                let c3: Retained<NSObject> =
                    msg_send_id![&b_trail, constraintEqualToAnchor: &*p_trail];
                let c4: Retained<NSObject> =
                    msg_send_id![&b_width, constraintEqualToConstant: width];
                activate(&c1);
                activate(&c2);
                activate(&c3);
                activate(&c4);
            }
            2 => {
                // bottom: full width pinned to parent bottom
                let c1: Retained<NSObject> =
                    msg_send_id![&b_bot, constraintEqualToAnchor: &*p_bot];
                let c2: Retained<NSObject> =
                    msg_send_id![&b_lead, constraintEqualToAnchor: &*p_lead];
                let c3: Retained<NSObject> =
                    msg_send_id![&b_trail, constraintEqualToAnchor: &*p_trail];
                let c4: Retained<NSObject> =
                    msg_send_id![&b_height, constraintEqualToConstant: width];
                activate(&c1);
                activate(&c2);
                activate(&c3);
                activate(&c4);
            }
            _ => {
                // left: full height pinned to parent leading
                let c1: Retained<NSObject> =
                    msg_send_id![&b_top, constraintEqualToAnchor: &*p_top];
                let c2: Retained<NSObject> =
                    msg_send_id![&b_bot, constraintEqualToAnchor: &*p_bot];
                let c3: Retained<NSObject> =
                    msg_send_id![&b_lead, constraintEqualToAnchor: &*p_lead];
                let c4: Retained<NSObject> =
                    msg_send_id![&b_width, constraintEqualToConstant: width];
                activate(&c1);
                activate(&c2);
                activate(&c3);
                activate(&c4);
            }
        }
    }
}
