//! Gradient backgrounds for the macOS backend.
//!
//! Same shape as `backend-ios-core/src/style.rs::install_gradient`
//! — install a `CAGradientLayer` as the view's lowest sublayer
//! and let Core Animation render it on every paint. Layer-level
//! code is identical across UIKit and AppKit; only the
//! `Color → CGColor` adapter (UIColor vs NSColor) differs.
//!
//! Layout-pass sync (`sync_gradient_sublayer`) is required because
//! CALayer's `autoresizingMask` doesn't drive automatic sublayer
//! resizing on iOS in practice (per [[project_gradient_native]]),
//! and the same constraint applies on macOS — we drive the
//! resize explicitly during the frame-apply walk.

use runtime_core::Gradient;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect, NSObject, NSString};

use crate::imp::CGColorRef;

/// Per-view gradient state. Held in `MacosBackend::gradient_states`
/// keyed by view pointer; lets `AnimProp::GradientStopColor(idx)`
/// rewrite a single stop without rebuilding the sublayer.
pub(crate) struct GradientState {
    /// Retained handle to the `CAGradientLayer` sublayer.
    pub(crate) layer: Retained<NSObject>,
    /// Current sRGB colors of each stop, in the same order
    /// `setColors:` was last written. Mutated by
    /// [`set_animated_gradient_stop`] when a per-frame color write
    /// arrives.
    pub(crate) stops_srgb: Vec<[f32; 4]>,
}

/// Install (or refresh) a `CAGradientLayer` sublayer on `view` that
/// fills its bounds with `gradient`. Removes any previous
/// `idealyst_gradient` sublayer so re-applies don't stack layers.
///
/// Returns the new [`GradientState`] (sublayer + per-stop sRGB
/// cache) so the caller can stash it for animated-stop writes.
/// Returns `None` if the view's backing layer is unavailable or is
/// a `CAMetalLayer` (Graphics-primitive views — we don't paint
/// gradients onto a render surface).
pub(crate) fn install_gradient(view: &NSView, gradient: &Gradient) -> Option<GradientState> {
    // Ensure the view is layer-backed. AppKit defers layer creation
    // until `setWantsLayer:true`; without this, `view.layer` returns
    // nil and the gradient can't be inserted.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Retained<NSObject> = unsafe { objc2::msg_send_id![view, layer] };

    // Don't paint a gradient on a Metal-backed view; the GPU layer
    // owns its own content and our sublayer would be hidden / fight
    // with frame swapping.
    let is_metal: bool = unsafe {
        msg_send![&layer, isKindOfClass: objc2::class!(CAMetalLayer)]
    };
    if is_metal {
        return None;
    }

    // Remove any previously installed `idealyst_gradient` sublayer.
    remove_existing(&layer);

    // Sort stops by ascending offset.
    let mut stops = gradient.stops.clone();
    stops.sort_by(|a, b| {
        a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Snapshot the resolved sRGB colors so animated-stop writes can
    // mutate one entry and re-apply without re-parsing colors.
    let stops_srgb: Vec<[f32; 4]> = stops.iter().map(|s| color_to_srgb(&s.color)).collect();

    let gradient_class = objc2::class!(CAGradientLayer);
    let gradient_layer: Retained<NSObject> =
        unsafe { objc2::msg_send_id![gradient_class, layer] };

    // Colors array — NSArray of CGColorRef.
    write_colors_from_srgb(&gradient_layer, &stops_srgb);

    // Locations — NSArray of NSNumber(double).
    let locations: Retained<NSObject> = unsafe {
        let arr: Retained<NSObject> =
            objc2::msg_send_id![objc2::class!(NSMutableArray), array];
        for stop in &stops {
            let n: Retained<NSObject> = objc2::msg_send_id![
                objc2::class!(NSNumber),
                numberWithDouble: stop.offset.clamp(0.0, 1.0) as f64
            ];
            let _: () = msg_send![&arr, addObject: &*n];
        }
        arr
    };
    let _: () = unsafe { msg_send![&gradient_layer, setLocations: &*locations] };

    // Linear vs. radial setup. Same math the iOS backend uses; see
    // `backend-ios-core/src/style.rs::build_gradient_layer` for the
    // derivation of the start/end-point formulas (CSS angle →
    // unit-square coords; `radius * 0.5` for ClosestSide,
    // `radius * 1/√2` for FarthestCorner).
    match gradient.kind {
        runtime_core::GradientKind::Linear { angle_deg } => {
            let theta_rad = (angle_deg as f64).to_radians();
            let dx = theta_rad.sin();
            let dy = -theta_rad.cos();
            let start = CGPoint {
                x: 0.5 - dx * 0.5,
                y: 0.5 - dy * 0.5,
            };
            let end = CGPoint {
                x: 0.5 + dx * 0.5,
                y: 0.5 + dy * 0.5,
            };
            let axial = NSString::from_str("axial");
            let _: () = unsafe { msg_send![&gradient_layer, setType: &*axial] };
            let _: () = unsafe { msg_send![&gradient_layer, setStartPoint: start] };
            let _: () = unsafe { msg_send![&gradient_layer, setEndPoint: end] };
        }
        runtime_core::GradientKind::Radial {
            center,
            radius,
            extent,
        } => {
            let radial = NSString::from_str("radial");
            let _: () = unsafe { msg_send![&gradient_layer, setType: &*radial] };
            let start = CGPoint {
                x: center.0 as f64,
                y: center.1 as f64,
            };
            let axis_offset = match extent {
                runtime_core::RadialExtent::ClosestSide => radius * 0.5,
                runtime_core::RadialExtent::FarthestCorner => {
                    radius * std::f32::consts::FRAC_1_SQRT_2
                }
            };
            let end = CGPoint {
                x: (center.0 + axis_offset) as f64,
                y: (center.1 + axis_offset) as f64,
            };
            let _: () = unsafe { msg_send![&gradient_layer, setStartPoint: start] };
            let _: () = unsafe { msg_send![&gradient_layer, setEndPoint: end] };
        }
    }

    // Tag so the layout pass can find this sublayer for resize.
    let marker = NSString::from_str("idealyst_gradient");
    let _: () = unsafe { msg_send![&gradient_layer, setName: &*marker] };

    // Initial frame matches view bounds; the layout pass keeps it
    // in sync via `sync_gradient_sublayer`.
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let _: () = unsafe { msg_send![&gradient_layer, setFrame: bounds] };
    let _: () = unsafe { msg_send![&gradient_layer, setNeedsDisplayOnBoundsChange: true] };

    // Insert at index 0 — beneath any author-managed sublayers but
    // above the layer's solid `backgroundColor` fill (so a solid
    // background set by `apply_style_to_view` shows through where
    // the gradient is fully transparent).
    let _: () = unsafe {
        msg_send![&layer, insertSublayer: &*gradient_layer, atIndex: 0u32]
    };

    Some(GradientState {
        layer: gradient_layer,
        stops_srgb,
    })
}

/// Per-frame writer for `AnimProp::GradientStopColor(idx)`. Updates
/// the cached stop color and re-applies `setColors:` on the cached
/// gradient layer — no sublayer walk, no rebuild. No-op if `idx` is
/// out of range (which can happen mid-build when an author animates
/// a stop that doesn't exist).
pub(crate) fn set_animated_gradient_stop(
    state: &mut GradientState,
    idx: usize,
    value: [f32; 4],
) {
    if idx >= state.stops_srgb.len() {
        return;
    }
    state.stops_srgb[idx] = value;
    write_colors_from_srgb(&state.layer, &state.stops_srgb);
}

fn color_to_srgb(color: &runtime_core::Color) -> [f32; 4] {
    runtime_core::color::parse_or(&color.0, runtime_core::color::Rgba::BLACK).to_srgb_f32()
}

fn write_colors_from_srgb(layer: &NSObject, stops: &[[f32; 4]]) {
    unsafe {
        let arr: Retained<NSObject> =
            objc2::msg_send_id![objc2::class!(NSMutableArray), array];
        for &c in stops {
            let ns_color = objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
                c[0] as f64,
                c[1] as f64,
                c[2] as f64,
                c[3] as f64,
            );
            let cg: CGColorRef = msg_send![&ns_color, CGColor];
            let id_ptr = cg.0 as *mut NSObject;
            let _: () = msg_send![&arr, addObject: id_ptr];
        }
        let _: () = msg_send![layer, setColors: &*arr];
    }
}

/// Resize any installed `idealyst_gradient` sublayer to match the
/// view's current bounds. Called from the layout pass after Taffy
/// has computed and applied frames — CALayer's autoresizingMask
/// doesn't drive automatic sublayer resizing in practice, so we
/// mirror the resize here.
pub(crate) fn sync_gradient_sublayer(view: &NSView) {
    // NSView is layer-optional on AppKit (unlike UIView, which is
    // layer-mandatory). `[view layer]` returns nil unless something
    // earlier set `wantsLayer=true` — and views without a layer can't
    // own an `idealyst_gradient` sublayer in the first place, so the
    // sync is a no-op for them. Use raw `msg_send!` + null-check
    // instead of `msg_send_id!` (which asserts non-nil and panics
    // when this function runs against an unstyled NSView in the
    // layout pass).
    let layer_ptr: *mut NSObject = unsafe { msg_send![view, layer] };
    if layer_ptr.is_null() {
        return;
    }
    let sublayers_ptr: *mut NSObject = unsafe { msg_send![layer_ptr, sublayers] };
    if sublayers_ptr.is_null() {
        return;
    }
    let count: usize = unsafe { msg_send![sublayers_ptr, count] };
    if count == 0 {
        return;
    }
    for i in 0..count {
        let sub_ptr: *mut NSObject = unsafe { msg_send![sublayers_ptr, objectAtIndex: i] };
        if sub_ptr.is_null() {
            continue;
        }
        let name_ptr: *mut NSString = unsafe { msg_send![sub_ptr, name] };
        if name_ptr.is_null() {
            continue;
        }
        let name_ref = unsafe { &*name_ptr };
        if name_ref.to_string() == "idealyst_gradient" {
            let bounds: CGRect = unsafe { msg_send![view, bounds] };
            let _: () = unsafe { msg_send![sub_ptr, setFrame: bounds] };
        }
    }
}

fn remove_existing(layer: &NSObject) {
    let sublayers_ptr: *mut NSObject = unsafe { msg_send![layer, sublayers] };
    if sublayers_ptr.is_null() {
        return;
    }
    // `[CALayer sublayers]` returns a *live* `CALayerArray` proxy, not a
    // snapshot: calling `removeFromSuperlayer` inside an index walk shrinks
    // it underneath us, so a later `objectAtIndex: i` runs past the new end
    // and AppKit aborts with `-[CALayerArray objectAtIndex:] out of bounds`.
    // This fires on every gradient *re-apply* (reactive re-render) where the
    // gradient sublayer is followed by any other sublayer. Collect the
    // matching layers in one read-only pass, then remove them.
    let count: usize = unsafe { msg_send![sublayers_ptr, count] };
    let mut doomed: Vec<*mut NSObject> = Vec::new();
    for i in 0..count {
        let sub_ptr: *mut NSObject = unsafe { msg_send![sublayers_ptr, objectAtIndex: i] };
        if sub_ptr.is_null() {
            continue;
        }
        let name_ptr: *mut NSString = unsafe { msg_send![sub_ptr, name] };
        if name_ptr.is_null() {
            continue;
        }
        let name_ref = unsafe { &*name_ptr };
        if name_ref.to_string() == "idealyst_gradient" {
            doomed.push(sub_ptr);
        }
    }
    for sub_ptr in doomed {
        let _: () = unsafe { msg_send![sub_ptr, removeFromSuperlayer] };
    }
}

#[cfg(test)]
mod tests {
    //! `remove_existing` itself can't be unit-tested without a live AppKit
    //! layer tree, so these tests model the one property that caused the
    //! crash: `[CALayer sublayers]` is a *live* array whose `objectAtIndex:`
    //! reads current contents and whose `removeFromSuperlayer` shrinks it in
    //! place. The bug class is "mutate while index-walking that live array";
    //! the fix is the two-phase "collect every match, then remove."

    /// Stand-in for the live `CALayerArray` proxy. `name_at` panics on an
    /// out-of-range index exactly as `-[CALayerArray objectAtIndex:]` aborts.
    struct LiveLayerArray {
        names: Vec<&'static str>,
    }

    impl LiveLayerArray {
        fn count(&self) -> usize {
            self.names.len()
        }
        fn name_at(&self, i: usize) -> &'static str {
            self.names[i]
        }
        fn remove_first_gradient(&mut self) {
            if let Some(p) = self.names.iter().position(|n| *n == "idealyst_gradient") {
                self.names.remove(p);
            }
        }
    }

    /// The original one-phase walk: snapshot `count`, then remove while
    /// indexing. When a removed gradient is followed by another sublayer,
    /// the array shrinks and the next `objectAtIndex:` runs past the end.
    #[test]
    #[should_panic]
    fn one_phase_walk_indexes_past_shrunk_array() {
        let mut arr = LiveLayerArray {
            names: vec!["idealyst_gradient", "author_content"],
        };
        let count = arr.count();
        for i in 0..count {
            if arr.name_at(i) == "idealyst_gradient" {
                arr.remove_first_gradient();
            }
        }
    }

    /// The two-phase fix: collect all matches in a read-only pass, then
    /// remove. The removal phase never re-indexes the shrinking array, so
    /// no out-of-bounds access occurs and every gradient layer is removed.
    #[test]
    fn two_phase_collect_then_remove_is_safe_and_complete() {
        let mut arr = LiveLayerArray {
            names: vec!["idealyst_gradient", "author_content", "idealyst_gradient"],
        };
        let count = arr.count();
        let mut doomed = Vec::new();
        for i in 0..count {
            if arr.name_at(i) == "idealyst_gradient" {
                doomed.push(i);
            }
        }
        assert_eq!(doomed, vec![0, 2]);
        for _ in &doomed {
            arr.remove_first_gradient();
        }
        assert_eq!(arr.names, vec!["author_content"]);
    }
}

