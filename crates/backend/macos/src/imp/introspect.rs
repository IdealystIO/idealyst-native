//! Platform-native render introspection for the macOS backend.
//!
//! Reads the **live AppKit object** — the `NSView` and its backing
//! `CALayer`, the `NSTextField`'s resolved `NSFont` — and reports the
//! values the platform actually applied, normalized to the canonical
//! [`runtime_core::introspect`] schema. This is the parity-testing surface:
//! the numbers here come from CoreGraphics/AppKit, never from the framework's
//! `StyleRules`, so a diff against the web backend catches the cases where
//! "the style we asked for" and "the style the platform applied" disagree.
//!
//! Available whenever the robot bridge can reach it (no extra feature); only
//! the phase-timer cost attribution stays behind `debug-stats`.

use objc2::rc::Retained;
use objc2::{class, msg_send, msg_send_id};
use objc2_app_kit::{NSColor, NSView};
use objc2_foundation::{CGFloat, CGRect, NSArray, NSString};

use runtime_core::introspect::{keys, collect_native_tree, NativeNode, NativeRect, NativeValue};

use super::{CGColorRef, MacosBackend, MacosNode};

impl MacosBackend {
    /// Read the native render tree for `node`. Entry point for
    /// `Backend::introspect_native`.
    pub(crate) fn introspect_native_impl(&self, node: &MacosNode) -> Option<NativeNode> {
        let _t = phase_timer();
        // The root view must be in a window to report meaningful geometry;
        // `abs_rect` returns None otherwise, which we surface as "not laid
        // out yet" (the bridge maps that to `null`).
        let root: Retained<NSView> = unsafe {
            Retained::retain(node.as_view() as *const NSView as *mut NSView)?
        };
        if abs_rect(&root).is_none() {
            return None;
        }
        Some(collect_native_tree(
            &root,
            &|v| read_view(v),
            &|v| subviews(v),
            &|v| self.is_framework_root(v),
        ))
    }

    /// A descendant `NSView` is a **framework element boundary** when it's a
    /// registered primitive root (present in `view_to_layout`). The native
    /// walk stops there — that subview is a separate element with its own
    /// `introspect_native`. Subviews absent from the map are platform/border
    /// internals belonging to *this* primitive.
    fn is_framework_root(&self, view: &Retained<NSView>) -> bool {
        let key = (&**view) as *const NSView as usize;
        self.view_to_layout.contains_key(&key)
    }
}

/// RAII phase timer so capture cost is attributable via `get_perf_counters`.
/// The timing (and the `runtime_core::debug` clock it reads) only exist under
/// `debug-stats`; without it the guard is a zero-field no-op the optimizer
/// strips — this is the feature's only remaining tie to `debug-stats`.
fn phase_timer() -> PhaseGuard {
    PhaseGuard {
        #[cfg(feature = "debug-stats")]
        start: runtime_core::debug::now_micros(),
    }
}
struct PhaseGuard {
    #[cfg(feature = "debug-stats")]
    start: u64,
}
#[cfg(feature = "debug-stats")]
impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let now = runtime_core::debug::now_micros();
        runtime_core::debug::record_apply_phase("introspect_native", now.saturating_sub(self.start));
    }
}

/// Shallow read of one `NSView`: class, frame, and the canonical resolved
/// props pulled from its CALayer (and, for text fields, its font/string).
fn read_view(view: &Retained<NSView>) -> NativeNode {
    let class_name = view.class().name().to_string();
    let frame = abs_rect(view).unwrap_or(NativeRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 });
    let mut node = NativeNode::leaf(class_name, frame);

    // hidden + opacity are view-level (alphaValue) combined with the layer's
    // opacity, matching how the framework applies alpha (view.setAlphaValue).
    let hidden: bool = unsafe { msg_send![&**view, isHidden] };
    node.set(keys::HIDDEN, Some(NativeValue::Flag(hidden)));
    let alpha: CGFloat = unsafe { msg_send![&**view, alphaValue] };

    // CALayer-backed visual props. A view without a layer reports neither —
    // absence is meaningful (the framework forces layer-backing on styled
    // views, so a missing layer means "unstyled", not "transparent").
    let layer: *mut objc2::runtime::AnyObject = unsafe { msg_send![&**view, layer] };
    if !layer.is_null() {
        let layer = unsafe { &*layer };

        let bg: CGColorRef = unsafe { msg_send![layer, backgroundColor] };
        node.set(keys::BACKGROUND_COLOR, cgcolor_rgba(bg).map(NativeValue::Color));

        let radius: CGFloat = unsafe { msg_send![layer, cornerRadius] };
        if radius > 0.0 {
            node.set(keys::CORNER_RADIUS, Some(NativeValue::Length(radius as f32)));
        }

        let border_w: CGFloat = unsafe { msg_send![layer, borderWidth] };
        if border_w > 0.0 {
            node.set(keys::BORDER_WIDTH, Some(NativeValue::Length(border_w as f32)));
            let border_c: CGColorRef = unsafe { msg_send![layer, borderColor] };
            node.set(keys::BORDER_COLOR, cgcolor_rgba(border_c).map(NativeValue::Color));
        }

        let layer_opacity: f32 = unsafe { msg_send![layer, opacity] };
        node.set(
            keys::OPACITY,
            Some(NativeValue::Number((alpha as f32) * layer_opacity)),
        );

        let shadow_opacity: f32 = unsafe { msg_send![layer, shadowOpacity] };
        if shadow_opacity > 0.0 {
            let shadow_radius: CGFloat = unsafe { msg_send![layer, shadowRadius] };
            node.set(keys::SHADOW_RADIUS, Some(NativeValue::Length(shadow_radius as f32)));
            let shadow_color: CGColorRef = unsafe { msg_send![layer, shadowColor] };
            node.set(keys::SHADOW_COLOR, cgcolor_rgba(shadow_color).map(NativeValue::Color));
        }
    } else {
        node.set(keys::OPACITY, Some(NativeValue::Number(alpha as f32)));
    }

    // Text fields: read the resolved font + displayed string + text color.
    if view.class().name() == "NSTextField"
        || unsafe { msg_send![&**view, isKindOfClass: class!(NSTextField)] }
    {
        node.role = Some("text".to_string());
        let s: *mut NSString = unsafe { msg_send![&**view, stringValue] };
        if !s.is_null() {
            node.set(keys::TEXT, Some(NativeValue::Text(unsafe { &*s }.to_string())));
        }
        let tc: *mut NSColor = unsafe { msg_send![&**view, textColor] };
        if !tc.is_null() {
            node.set(keys::TEXT_COLOR, nscolor_rgba(unsafe { &*tc }).map(NativeValue::Color));
        }
        read_font(view, &mut node);
    }

    node
}

/// Read the field's resolved `NSFont` → canonical family/size/weight.
fn read_font(view: &Retained<NSView>, node: &mut NativeNode) {
    let font: *mut objc2::runtime::AnyObject = unsafe { msg_send![&**view, font] };
    if font.is_null() {
        return;
    }
    let font = unsafe { &*font };
    let family: *mut NSString = unsafe { msg_send![font, familyName] };
    if !family.is_null() {
        node.set(keys::FONT_FAMILY, Some(NativeValue::Text(unsafe { &*family }.to_string())));
    }
    let size: CGFloat = unsafe { msg_send![font, pointSize] };
    node.set(keys::FONT_SIZE, Some(NativeValue::Length(size as f32)));
    node.set(keys::FONT_WEIGHT, Some(NativeValue::Number(font_weight_css(font))));
}

/// Map the resolved font's Apple weight trait (`NSFontWeightTrait`, a float in
/// roughly `[-1, 1]`) onto the CSS numeric weight axis (100–900) so it's
/// directly comparable with the web backend's computed `font-weight`. The
/// breakpoints follow Apple's documented system-weight constants
/// (ultralight…black); approximate by design, hence a nearest-bucket map
/// rather than a fabricated linear scale.
fn font_weight_css(font: &objc2::runtime::AnyObject) -> f32 {
    let descriptor: *mut objc2::runtime::AnyObject = unsafe { msg_send![font, fontDescriptor] };
    if descriptor.is_null() {
        return 400.0;
    }
    // traits = [descriptor objectForKey:NSFontTraitsAttribute]; weight =
    // [traits objectForKey:NSFontWeightTrait] (an NSNumber, -1.0..1.0).
    let traits_key = NSString::from_str("NSCTFontTraitsAttribute");
    let traits: *mut objc2::runtime::AnyObject =
        unsafe { msg_send![descriptor, objectForKey: &*traits_key] };
    if traits.is_null() {
        return 400.0;
    }
    let weight_key = NSString::from_str("NSCTFontWeightTrait");
    let weight_num: *mut objc2::runtime::AnyObject =
        unsafe { msg_send![traits, objectForKey: &*weight_key] };
    if weight_num.is_null() {
        return 400.0;
    }
    let w: CGFloat = unsafe { msg_send![weight_num, doubleValue] };
    // Apple system-weight constants → CSS weights.
    const TABLE: &[(f64, f32)] = &[
        (-0.80, 100.0), // ultralight
        (-0.60, 200.0), // thin
        (-0.40, 300.0), // light
        (0.00, 400.0),  // regular
        (0.23, 500.0),  // medium
        (0.30, 600.0),  // semibold
        (0.40, 700.0),  // bold
        (0.56, 800.0),  // heavy
        (0.62, 900.0),  // black
    ];
    TABLE
        .iter()
        .min_by(|a, b| (a.0 - w).abs().partial_cmp(&(b.0 - w).abs()).unwrap())
        .map(|(_, css)| *css)
        .unwrap_or(400.0)
}

/// A view's window-relative rect in logical px (top-left origin), matching
/// `absolute_frame` so it lines up with the web `getBoundingClientRect`.
fn abs_rect(view: &Retained<NSView>) -> Option<NativeRect> {
    let bounds: CGRect = unsafe { msg_send![&**view, bounds] };
    let window: *mut objc2::runtime::AnyObject = unsafe { msg_send![&**view, window] };
    if window.is_null() {
        return None;
    }
    let content: *mut NSView = unsafe { msg_send![window, contentView] };
    let to_view: *mut NSView = if content.is_null() { std::ptr::null_mut() } else { content };
    let in_window: CGRect = unsafe { msg_send![&**view, convertRect: bounds, toView: to_view] };
    let (tx, ty) = super::animated::view_layer_translate(&**view);
    Some(NativeRect {
        x: in_window.origin.x as f32 + tx as f32,
        y: in_window.origin.y as f32 + ty as f32,
        width: in_window.size.width as f32,
        height: in_window.size.height as f32,
    })
}

/// A view's direct subviews as retained handles. Indexed access (rather than
/// `NSArray::iter`) so each element is an owned `Retained` we can hold across
/// the synchronous walk.
fn subviews(view: &Retained<NSView>) -> Vec<Retained<NSView>> {
    let arr: Retained<NSArray<NSView>> = unsafe { msg_send_id![&**view, subviews] };
    let count: usize = unsafe { msg_send![&*arr, count] };
    (0..count)
        .map(|i| unsafe { msg_send_id![&*arr, objectAtIndex: i] })
        .collect()
}

/// Convert a (possibly non-sRGB) `CGColorRef` to straight sRGB RGBA `0..1`.
/// Routes through `NSColor` so AppKit does the color-space conversion — we do
/// not assume the layer color is already sRGB (that would bias the read).
fn cgcolor_rgba(cg: CGColorRef) -> Option<[f32; 4]> {
    if cg.0.is_null() {
        return None;
    }
    let color: *mut NSColor = unsafe { msg_send![class!(NSColor), colorWithCGColor: cg] };
    if color.is_null() {
        return None;
    }
    nscolor_rgba(unsafe { &*color })
}

/// Convert an `NSColor` to straight sRGB RGBA `0..1`. Returns `None` for
/// pattern/catalog colors that have no RGB representation.
fn nscolor_rgba(color: &NSColor) -> Option<[f32; 4]> {
    let srgb_space: *mut objc2::runtime::AnyObject =
        unsafe { msg_send![class!(NSColorSpace), sRGBColorSpace] };
    let converted: *mut NSColor = unsafe { msg_send![color, colorUsingColorSpace: srgb_space] };
    if converted.is_null() {
        return None;
    }
    let converted = unsafe { &*converted };
    let r: CGFloat = unsafe { msg_send![converted, redComponent] };
    let g: CGFloat = unsafe { msg_send![converted, greenComponent] };
    let b: CGFloat = unsafe { msg_send![converted, blueComponent] };
    let a: CGFloat = unsafe { msg_send![converted, alphaComponent] };
    Some([r as f32, g as f32, b as f32, a as f32])
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::read_view;
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSColor, NSView};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSObject};

    use crate::imp::{CGColorRef, FlippedView};
    use runtime_core::introspect::{keys, NativeValue};

    // Reads must come from the LIVE CALayer, not from any style struct: this
    // test sets the layer's own backgroundColor/cornerRadius/borderWidth
    // DIRECTLY (no `StyleRules` involved) and asserts `read_view` reports those
    // exact resolved values. A regression that echoed author input instead of
    // reading the layer would have nothing to echo here — proving the read is
    // genuinely sourced from the platform object.
    //
    // No `NSWindow`: `read_view`'s geometry degrades to a zero rect without a
    // window (asserted below), so the value-read path is exercised without a
    // WindowServer connection (which a headless test host lacks). The
    // window-relative geometry path is covered by `imp::handles`'s test.
    #[test]
    fn read_view_reads_resolved_layer_state() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };

        let view: Retained<NSView> = Retained::into_super(FlippedView::new(mtm));
        let frame = CGRect {
            origin: CGPoint { x: 30.0, y: 40.0 },
            size: CGSize { width: 120.0, height: 60.0 },
        };
        let _: () = unsafe { msg_send![&view, setFrame: frame] };
        let _: () = unsafe { msg_send![&view, setWantsLayer: true] };
        let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
        // A specific sRGB color so the read-back is exact (modulo color-space).
        let ns: Retained<NSColor> =
            unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.2, 0.4, 0.8, 1.0) };
        let cg: CGColorRef = unsafe { msg_send![&*ns, CGColor] };
        let _: () = unsafe { msg_send![&layer, setBackgroundColor: cg] };
        let _: () = unsafe { msg_send![&layer, setCornerRadius: 8.0_f64] };
        let _: () = unsafe { msg_send![&layer, setBorderWidth: 2.0_f64] };
        let _: () = unsafe { msg_send![&layer, setBorderColor: cg] };

        let node = read_view(&view);

        // Class read from the live object.
        assert!(node.class.contains("View"), "class was {}", node.class);

        // Resolved layer values, read back from CoreGraphics.
        match node.props.get(keys::CORNER_RADIUS) {
            Some(NativeValue::Length(r)) => assert!((r - 8.0).abs() < 0.01, "radius {r}"),
            other => panic!("expected corner_radius, got {other:?}"),
        }
        match node.props.get(keys::BORDER_WIDTH) {
            Some(NativeValue::Length(w)) => assert!((w - 2.0).abs() < 0.01, "border {w}"),
            other => panic!("expected border_width, got {other:?}"),
        }
        match node.props.get(keys::BACKGROUND_COLOR) {
            Some(NativeValue::Color(c)) => {
                assert!((c[0] - 0.2).abs() < 0.02, "r {}", c[0]);
                assert!((c[1] - 0.4).abs() < 0.02, "g {}", c[1]);
                assert!((c[2] - 0.8).abs() < 0.02, "b {}", c[2]);
                assert!((c[3] - 1.0).abs() < 0.02, "a {}", c[3]);
            }
            other => panic!("expected background_color, got {other:?}"),
        }
    }
}
