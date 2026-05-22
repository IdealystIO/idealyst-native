use framework_core::{Color, Length, StyleRules, Tokenized};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGSize, NSObject};
use objc2_ui_kit::{UIColor, UIView};
use std::rc::Rc;
use block2::ConcreteBlock;

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

pub fn font_weight_to_uikit(weight: framework_core::FontWeight) -> CGFloat {
    match weight {
        framework_core::FontWeight::Thin => -0.6,
        framework_core::FontWeight::ExtraLight => -0.5,
        framework_core::FontWeight::Light => -0.4,
        framework_core::FontWeight::Normal => 0.0,
        framework_core::FontWeight::Medium => 0.23,
        framework_core::FontWeight::SemiBold => 0.3,
        framework_core::FontWeight::Bold => 0.4,
        framework_core::FontWeight::ExtraBold => 0.56,
        framework_core::FontWeight::Black => 0.62,
    }
}

/// Map framework Easing to UIView animation options bitmask.
pub fn easing_to_options(easing: &framework_core::Easing) -> u64 {
    match easing {
        framework_core::Easing::Linear => 3 << 16,
        framework_core::Easing::Ease | framework_core::Easing::EaseInOut => 0 << 16,
        framework_core::Easing::EaseIn => 1 << 16,
        framework_core::Easing::EaseOut => 2 << 16,
        framework_core::Easing::CubicBezier(_, _, _, _) => 0 << 16,
    }
}

/// Pick the transition to use for a property change. With the
/// active-theme concept gone, the only source of a transition is the
/// stylesheet's per-property `Transition` field — explicit opt-in.
/// Snap by default.
fn effective_transition(
    explicit: Option<&framework_core::Transition>,
) -> Option<framework_core::Transition> {
    explicit.copied()
}

/// Run property changes inside a UIView animation block.
pub fn animate(transition: &framework_core::Transition, changes: Rc<dyn Fn()>) {
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
    gradient: Option<&framework_core::Gradient>,
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
    g: &framework_core::Gradient,
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
        framework_core::GradientKind::Linear { angle_deg } => {
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
        framework_core::GradientKind::Radial { center, radius, extent } => {
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
                framework_core::RadialExtent::ClosestSide => radius * 0.5,
                framework_core::RadialExtent::FarthestCorner => radius * std::f32::consts::FRAC_1_SQRT_2,
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

/// Resolve a `framework_core::Color` to sRGB `[r, g, b, a]` in
/// `0..=1`. Mirrors `color_to_uicolor`'s parsing but skips the
/// UIColor construction — useful for caching colors in animation
/// state and rebuilding the UIColor on each apply.
fn color_to_srgb(color: &Color) -> [f32; 4] {
    framework_core::color::parse_or(&color.0, framework_core::color::Rgba::BLACK).to_srgb_f32()
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

pub fn apply_style_to_view(view: &UIView, style: &StyleRules) {
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
    // native-layout.

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
    let radius = [
        style.border_top_left_radius.as_ref(),
        style.border_top_right_radius.as_ref(),
        style.border_bottom_left_radius.as_ref(),
        style.border_bottom_right_radius.as_ref(),
    ]
    .iter()
    .filter_map(|r| r.map(|t| length_to_px(&t.resolve())))
    .fold(0.0_f64, f64::max);
    if radius > 0.0 {
        // Only `Px` lengths produce a clamp value at apply-style time.
        // `Percent` and `Auto` resolve against the parent / layout
        // pass and have no useful px value here. When neither axis
        // has a px clamp, defer the clamp to the layout pass via
        // `sync_corner_radius` — without it, `cornerRadius = 999`
        // on a percent-sized view sets the value before bounds are
        // known and UIKit renders nothing once bounds arrive.
        fn px_half(t: &Tokenized<Length>) -> Option<f64> {
            match t.resolve() {
                Length::Px(v) => Some(v as f64 / 2.0),
                _ => None,
            }
        }
        let half_w = style.width.as_ref().and_then(px_half);
        let half_h = style.height.as_ref().and_then(px_half);
        let cap = match (half_w, half_h) {
            (Some(w), Some(h)) => Some(w.min(h)),
            (Some(w), None) => Some(w),
            (None, Some(h)) => Some(h),
            (None, None) => None,
        };
        match cap {
            Some(c) => {
                let effective = radius.min(c);
                let _: () = unsafe { msg_send![&layer, setCornerRadius: effective] };
            }
            None => {
                // No px dimensions to clamp against. Stash the
                // requested value as an NSNumber association on the
                // layer; `sync_corner_radius` (called from the layout
                // pass) will clamp against the laid-out bounds. Set
                // an explicit cornerRadius of 0 in the meantime so we
                // don't render the "999 on tiny view → blank" state
                // while bounds are still 0×0.
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
                let _: () = unsafe { msg_send![&layer, setCornerRadius: 0.0_f64] };
            }
        }
        unsafe { view.setClipsToBounds(true) };
    }

    // Border width
    let border_w = [
        style.border_top_width.as_ref(),
        style.border_right_width.as_ref(),
        style.border_bottom_width.as_ref(),
        style.border_left_width.as_ref(),
    ]
    .iter()
    .filter_map(|w| w.map(|t| t.resolve()))
    .fold(0.0_f32, f32::max);
    if border_w > 0.0 {
        let _: () = unsafe { msg_send![&layer, setBorderWidth: border_w as CGFloat] };
    }

    // Border color
    let border_color = style
        .border_top_color
        .as_ref()
        .or(style.border_right_color.as_ref())
        .or(style.border_bottom_color.as_ref())
        .or(style.border_left_color.as_ref());
    if let Some(bc) = border_color {
        let bc_val = bc.resolve();
        let c = color_to_uicolor(&bc_val);
        let cg: CGColorRef = unsafe { msg_send![&c, CGColor] };
        if !cg.0.is_null() {
            let _: () = unsafe { msg_send![&layer, setBorderColor: cg] };
        }
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

    // Padding is handled entirely by Taffy now (writes into the
    // node's `padding` Rect, which insets the content area inside
    // the view's frame). We don't forward to setLayoutMargins
    // because UIView's layoutMargins are only consulted by
    // UIStackView's `layoutMarginsRelativeArrangement`, which we no
    // longer use.

    // Overflow
    if let Some(overflow) = &style.overflow {
        match overflow {
            framework_core::Overflow::Hidden => unsafe { view.setClipsToBounds(true) },
            framework_core::Overflow::Visible => unsafe { view.setClipsToBounds(false) },
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
    if let Some(color) = &style.color {
        let color_val = color.resolve();
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
            .unwrap_or(framework_core::FontWeight::Normal);
        let fstyle = style
            .font_style
            .as_ref()
            .copied()
            .unwrap_or(framework_core::FontStyle::Normal);
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
            framework_core::TextAlign::Left => 0,
            framework_core::TextAlign::Center => 1,
            framework_core::TextAlign::Right => 2,
            framework_core::TextAlign::Justify => 3,
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
