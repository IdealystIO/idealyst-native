//! macOS backend implementation. AppKit-flavored.
//!
//! Mirrors the iOS backend's shape (Taffy-driven layout, per-view
//! layout-node map, intrinsic-size measurers) with AppKit primitives
//! and desktop idioms — cursor instead of touch, NSToolbar instead
//! of UINavigationController, NSSplitView for drawer-style layouts.
//!
//! See `docs/macos-backend-plan.md` for the design.

pub(crate) mod a11y;
pub(crate) mod animated;
pub(crate) mod gradient;
pub(crate) mod handles;
pub(crate) mod node;
pub(crate) mod text_style;
pub(crate) mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::{Backend, Color, StyleRules};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::{NSColor, NSTextField, NSView};
use objc2_foundation::{CGFloat, CGRect, CGSize, MainThreadMarker, NSObject, NSString};

use backend_apple_core::color::parse_color;

pub use node::MacosNode;
pub use view::FlippedView;

// =========================================================================
// MacosBackend
// =========================================================================

pub struct MacosBackend {
    mtm: MainThreadMarker,
    host_root: Option<Retained<NSView>>,
    /// Parallel layout tree. Same shape as iOS — every NSView pointer
    /// maps to a Taffy node; `finish()` runs `compute(host_bounds)`
    /// and walks the map to assign `frame` on every registered view.
    pub(crate) layout: runtime_layout::LayoutTree,
    /// Map from view pointer → (retained NSView, layout node).
    /// Retained explicitly so views that aren't yet attached to the
    /// host (e.g. mid-build) survive the layout walk.
    pub(crate) view_to_layout:
        HashMap<usize, (Retained<NSView>, runtime_layout::LayoutNode)>,
    /// Process-registered custom fonts + per-`Typeface` lookup table.
    /// Same shape as iOS — driven by `register_asset` / `register_typeface`.
    /// Read by the text-style applier when constructing NSFont.
    pub(crate) font_registry: backend_apple_core::font::FontRegistry,
    /// Strong refs to retained Obj-C objects we own (callback targets,
    /// gesture recognizers, observers). Kept alive for the backend's
    /// lifetime so they outlive any closures they back.
    #[allow(dead_code)]
    callback_targets: Vec<Retained<NSObject>>,
    /// Per-view cached animation state. Keyed by view pointer;
    /// holds the translate/scale/rotate components so writing one
    /// doesn't destroy the others (CALayer's `transform` is a single
    /// matrix). Mirrors the iOS `animated_states` map.
    pub(crate) animated_states: animated::AnimatedStateMap,
    /// Per-view cached gradient state. Keyed by view pointer; holds
    /// the gradient `CAGradientLayer` retained handle plus the
    /// current sRGB stop colors so `AnimProp::GradientStopColor(idx)`
    /// can rewrite a single stop without rebuilding the sublayer.
    pub(crate) gradient_states: HashMap<usize, gradient::GradientState>,
}

// =========================================================================
// Global self-handle — lets navigator/drawer dispatch closures
// schedule a layout pass after they mount new screens. Mirrors the
// iOS pattern; populated by `install_global_self` after the host
// wraps the backend in Rc<RefCell<>>.
// =========================================================================

thread_local! {
    static MACOS_BACKEND_SELF: RefCell<Option<std::rc::Weak<RefCell<MacosBackend>>>> =
        const { RefCell::new(None) };
}

/// Install the backend's self-reference. Hosts call this once after
/// wrapping the backend in `Rc<RefCell<>>` so navigator-side closures
/// can reach back into the backend without capturing it directly.
pub fn install_global_self(weak: std::rc::Weak<RefCell<MacosBackend>>) {
    MACOS_BACKEND_SELF.with(|s| {
        *s.borrow_mut() = Some(weak);
    });
}

/// Push a scalar animation property update to `node` on the
/// installed global backend. The cross-platform animation system's
/// per-frame subscribers reach the macOS backend through this —
/// same shape as the iOS routing in `backend_ios_mobile`.
///
/// Quietly no-ops if no backend is installed yet (pre-render) or
/// the install has been dropped (post-teardown), or if the backend
/// is already borrowed (the in-flight Rust call will see the new
/// value on its next frame).
pub fn set_animated_f32(
    node: &MacosNode,
    prop: runtime_core::animation::AnimProp,
    value: f32,
) {
    let weak = MACOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    // Bind the try_borrow_mut result to a let so its temp drops
    // before `rc`. Without the binding, the if-let's scrutinee
    // outlives `rc` per the new borrow rules.
    let borrow = rc.try_borrow_mut();
    if let Ok(mut b) = borrow {
        use runtime_core::Backend;
        b.set_animated_f32(node, prop, value);
    }
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`.
pub fn set_animated_color(
    node: &MacosNode,
    prop: runtime_core::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = MACOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let borrow = rc.try_borrow_mut();
    if let Ok(mut b) = borrow {
        use runtime_core::Backend;
        b.set_animated_color(node, prop, value);
    }
}

// =========================================================================
// Construction + host wiring
// =========================================================================

impl MacosBackend {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            host_root: None,
            layout: runtime_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
            font_registry: backend_apple_core::font::FontRegistry::new(),
            callback_targets: Vec::new(),
            animated_states: HashMap::new(),
            gradient_states: HashMap::new(),
        }
    }

    /// `MainThreadMarker` accessor for third-party SDK extension code
    /// that needs to construct main-thread-only Obj-C objects. The
    /// marker is `Copy` so handing it out doesn't tie the SDK to the
    /// backend's borrow lifetime.
    pub fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    /// Install the host's root NSView. Subsequent `finish()` calls
    /// compute layout against this view's bounds and apply frames
    /// down the registered tree.
    pub fn set_host_root(&mut self, view: Retained<NSView>) {
        self.host_root = Some(view);
    }

    /// Convenience for host crates: construct a flipped NSView the
    /// host can mount as its NSWindow contentView. The host then
    /// calls [`MacosBackend::set_host_root`] with the same view so
    /// the layout pass knows where to compute against.
    ///
    /// Using `FlippedView` here (top-left origin) means every
    /// subview placed via the layout pass lands in the same
    /// coordinate space the framework / Taffy emits — no per-frame
    /// Y inversion needed.
    pub fn create_host_root(&self) -> Option<Retained<NSView>> {
        let view = FlippedView::new(self.mtm);
        let view: Retained<NSView> = Retained::into_super(view);
        Some(view)
    }

    /// Get or create a layout node for an NSView. Called from every
    /// `create_*` method so each native view has a corresponding node
    /// in the layout tree.
    pub(crate) fn layout_for_view(&mut self, view: &NSView) -> runtime_layout::LayoutNode {
        let key = view as *const NSView as usize;
        if let Some((_, node)) = self.view_to_layout.get(&key) {
            return *node;
        }
        let node = self.layout.new_node();
        let retained = unsafe {
            Retained::retain(view as *const NSView as *mut NSView).expect("retain NSView")
        };
        self.view_to_layout.insert(key, (retained, node));
        node
    }

    /// Look up an existing layout node by view pointer. Returns
    /// `None` for views that weren't created by this backend.
    #[allow(dead_code)]
    pub(crate) fn layout_of(&self, view: &NSView) -> Option<runtime_layout::LayoutNode> {
        let key = view as *const NSView as usize;
        self.view_to_layout.get(&key).map(|(_, n)| *n)
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Opaque wrapper for CoreGraphics' `CGColorRef` so `msg_send!`'s
/// debug-mode encoding check sees `^{CGColor=}` instead of `^v`.
/// Mirrors the iOS-side wrapper in `backend-ios-core/src/style.rs`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct CGColorRef(pub *const std::ffi::c_void);

unsafe impl Encode for CGColorRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

/// Build an `NSColor` from a framework `Color`. Uses the shared
/// apple-core color parser (sRGB float tuple) then routes through
/// AppKit's RGB initializer.
pub(crate) fn color_to_nscolor(color: &Color) -> Retained<NSColor> {
    let (r, g, b, a) = parse_color(&color.0);
    unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a) }
}

/// Apply the framework's `StyleRules` to an NSView's CALayer-backed
/// presentation. Minimum viable set: background color, opacity,
/// corner radius. More properties (gradients, shadows, borders) will
/// follow the iOS shape.
fn apply_style_to_view(view: &NSView, style: &StyleRules) {
    // Force CALayer backing so `backgroundColor`, `cornerRadius`,
    // etc. work. NSView is layer-optional by default; we want the
    // same CALayer-backed presentation iOS / Android have so styling
    // is uniform.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };

    // Background color → layer's `backgroundColor` (CGColorRef).
    if let Some(bg) = &style.background {
        let bg_val = bg.resolve();
        let ns = color_to_nscolor(&bg_val);
        let cg: CGColorRef = unsafe { msg_send![&ns, CGColor] };
        if !cg.0.is_null() {
            let _: () = unsafe { msg_send![&layer, setBackgroundColor: cg] };
        }
    }

    // Gradient background is handled by the caller (`apply_style`)
    // because the returned `GradientState` lives in the backend's
    // per-view cache — `apply_style_to_view` doesn't have access.

    // Opacity. Use `NSView.setAlphaValue:` (matches iOS's
    // `UIView.setAlpha:`), NOT `CALayer.setOpacity:`. The two
    // multiply in AppKit's rendering pipeline, so routing static
    // opacity through the layer while animated opacity goes through
    // the view (`AnimProp::Opacity` → `setAlphaValue:`) would make
    // any author who set both static AND animated opacity get
    // (static * animated) — visibly broken whenever the static
    // value is < 1. Welcome's planets hit this case exactly: the
    // sheet declares `opacity: 0.0` as the resting state and the
    // raf body animates `setAlphaValue:` up to `fade_in`. Pre-fix
    // the planets stayed invisible because the layer opacity
    // stayed at 0 regardless of `setAlphaValue:`. Both backends
    // now route static + animated through the same field.
    if let Some(opacity) = style.opacity.as_ref().map(|t| t.resolve()) {
        let _: () = unsafe { msg_send![view, setAlphaValue: opacity as f64] };
    }

    // Corner radius. Same caveat as iOS: AppKit's `cornerRadius`
    // isn't clamped to `min(W, H)/2`, so the CSS-idiomatic
    // `border-radius: 999px` renders nothing on a small box. We
    // clamp to half the smaller explicit dimension when available;
    // percent/auto dims defer to the layout pass.
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
        fn px_half(t: &runtime_core::Tokenized<runtime_core::Length>) -> Option<f64> {
            match t.resolve() {
                runtime_core::Length::Px(v) => Some(v as f64 / 2.0),
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
                // Defer clamping to the layout pass — without explicit
                // px dimensions, we can't know `min(W, H)/2` until
                // Taffy assigns a frame. Stash the requested value as
                // an associated `NSNumber` on the layer; the layout
                // pass reads it back in `sync_corner_radius` and
                // clamps against the real bounds. Same pattern iOS
                // uses; see [[project_ios_cornerradius_unclamped]].
                //
                // We set cornerRadius=0 in the meantime so the layer
                // doesn't render the broken "999 on tiny view → blank"
                // state during the first paint before layout lands.
                let key = NSString::from_str("idealyst_requested_corner_radius");
                let number: Retained<NSObject> = unsafe {
                    msg_send_id![objc2::class!(NSNumber), numberWithDouble: radius]
                };
                let _: () = unsafe { msg_send![&layer, setValue: &*number, forKey: &*key] };
                let _: () = unsafe { msg_send![&layer, setCornerRadius: 0.0_f64] };
            }
        }
        // AppKit's NSView has no `setMasksToBounds:` (that's UIView).
        // Equivalent is `layer.masksToBounds = true` on the CALayer
        // directly. Required so cornerRadius clips child content,
        // matching the UIView `clipsToBounds` behavior.
        let _: () = unsafe { msg_send![&layer, setMasksToBounds: true] };
    }
}

/// Resolve a deferred cornerRadius (stashed as `idealyst_requested_
/// corner_radius` on the layer by [`apply_style_to_view`] when the
/// view's dimensions weren't known at apply-style time) against the
/// view's now-laid-out bounds. Mirrors `backend_ios_core::style::
/// sync_corner_radius`.
fn sync_corner_radius(view: &NSView) {
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let key = NSString::from_str("idealyst_requested_corner_radius");
    let value_ptr: *mut NSObject = unsafe { msg_send![&layer, valueForKey: &*key] };
    if value_ptr.is_null() {
        return;
    }
    let value: &NSObject = unsafe { &*value_ptr };
    let requested: f64 = unsafe { msg_send![value, doubleValue] };
    if requested <= 0.0 {
        return;
    }
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let half_w = bounds.size.width / 2.0;
    let half_h = bounds.size.height / 2.0;
    let cap = half_w.min(half_h);
    let effective = requested.min(cap.max(0.0));
    let _: () = unsafe { msg_send![&layer, setCornerRadius: effective] };
}

fn length_to_px(len: &runtime_core::Length) -> CGFloat {
    match len {
        runtime_core::Length::Px(v) => *v as CGFloat,
        runtime_core::Length::Percent(_) | runtime_core::Length::Auto => 0.0,
    }
}

// =========================================================================
// Backend trait implementation
// =========================================================================

impl Backend for MacosBackend {
    type Node = MacosNode;

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::MacOs
    }

    fn color_scheme(&self) -> runtime_core::ColorScheme {
        // NSAppearance.currentAppearance.name → light/dark.
        // For now treat anything that isn't aqua as dark; refine
        // later if vibrant variants matter.
        let cls = objc2::class!(NSAppearance);
        let appearance: *const NSObject = unsafe { msg_send![cls, currentAppearance] };
        if appearance.is_null() {
            return runtime_core::ColorScheme::Auto;
        }
        let name_ptr: *const NSString = unsafe { msg_send![appearance, name] };
        if name_ptr.is_null() {
            return runtime_core::ColorScheme::Auto;
        }
        let s = unsafe { (*name_ptr).to_string() };
        if s.contains("Dark") {
            runtime_core::ColorScheme::Dark
        } else if s.contains("Aqua") {
            runtime_core::ColorScheme::Light
        } else {
            runtime_core::ColorScheme::Auto
        }
    }

    fn create_view(&mut self, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        let view = FlippedView::new(self.mtm);
        let view: Retained<NSView> = Retained::into_super(view);
        let _ = self.layout_for_view(&view);
        let node = MacosNode::View(view);
        // View has no default a11y role (transparent container); pass
        // None and let `a11y::apply` skip role writing entirely when
        // author code hasn't supplied one.
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::View,
            ),
        );
        node
    }

    fn create_text(&mut self, content: &str, a11y: &runtime_core::accessibility::AccessibilityProps) -> Self::Node {
        // NSTextField in label mode is AppKit's UILabel equivalent.
        // `+[NSTextField labelWithString:]` configures it as
        // non-editable, non-selectable, no border, no background.
        let ns = NSString::from_str(content);
        let label: Retained<NSTextField> = unsafe {
            msg_send_id![
                objc2::class!(NSTextField),
                labelWithString: &*ns
            ]
        };

        // Multi-line wrap: NSTextField's cell needs `wraps = true` +
        // `usesSingleLineMode = false` for the same behavior iOS's
        // `numberOfLines = 0 + lineBreakMode = byWordWrapping` gives.
        let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
        let _: () = unsafe { msg_send![&cell, setWraps: true] };
        let _: () = unsafe { msg_send![&cell, setUsesSingleLineMode: false] };

        // Install a measure_fn so Taffy queries `cellSizeForBounds:`
        // at compute time and gives the label the right wrap height.
        let view: Retained<NSView> = unsafe {
            // NSTextField inherits from NSControl → NSView, so this
            // upcast is safe via the ObjC class hierarchy.
            Retained::retain(Retained::as_ptr(&label) as *mut NSView).unwrap()
        };
        let layout = self.layout_for_view(&view);
        let label_for_measure = label.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, available_space| {
                let avail_w = known_dimensions
                    .width
                    .unwrap_or(match available_space.width {
                        runtime_layout::AvailableSpace::Definite(w) => w,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });
                let avail_h = known_dimensions
                    .height
                    .unwrap_or(match available_space.height {
                        runtime_layout::AvailableSpace::Definite(h) => h,
                        runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                        runtime_layout::AvailableSpace::MinContent => 0.0,
                    });
                let bounds = CGRect {
                    origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: if avail_w.is_finite() { avail_w as f64 } else { 10_000.0 },
                        height: if avail_h.is_finite() { avail_h as f64 } else { 10_000.0 },
                    },
                };
                let cell: *mut NSObject = unsafe { msg_send![&label_for_measure, cell] };
                let fitted: CGSize = unsafe { msg_send![cell, cellSizeForBounds: bounds] };
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or((fitted.width as f32).ceil()),
                    height: known_dimensions.height.unwrap_or((fitted.height as f32).ceil()),
                }
            }),
        );

        let node = MacosNode::Label(label);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Text,
            ),
        );
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading_icon: Option<&runtime_core::IconData>,
        _trailing_icon: Option<&runtime_core::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Minimum-viable stub. Real NSButton wiring (bezel style,
        // target/action, icon images, intrinsic measure) lands in
        // a follow-up — gets us a placeholder so user code that
        // contains a Button still renders without panicking.
        //
        // We still wire a11y on the stub view so VoiceOver hits a
        // labelled `Button`-role element even before the real NSButton
        // lands; the later impl can keep this call site as-is and add
        // its own NSButton-specific a11y in addition.
        let _ = label;
        let node = self.create_view(&runtime_core::accessibility::AccessibilityProps::default());
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Button,
            ),
        );
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();

        // AppKit: `addSubview:` mounts the child. Frame is determined
        // by the layout pass in `finish()`; here we just establish
        // the parent/child relationship in both the view tree AND
        // the Taffy tree.
        unsafe { parent_view.addSubview(child_view) };

        // Mirror the parenting in Taffy. If either node is missing
        // from `view_to_layout`, fall back to creating one — defensive
        // against early-render races.
        let parent_layout = self.layout_for_view(parent_view);
        let child_layout = self.layout_for_view(child_view);
        self.layout.add_child(parent_layout, child_layout);
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let MacosNode::Label(label) = node {
            let ns = NSString::from_str(content);
            let _: () = unsafe { msg_send![label, setStringValue: &*ns] };
            // Text-content change can shift wrapped height; mark the
            // layout node dirty so the next layout pass re-measures.
            let view = node.as_view();
            if let Some(layout) = self.layout_of(view) {
                self.layout.mark_dirty(layout);
            }
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let view = node.as_view();
        // AppKit: removeFromSuperview on each subview. Walk a
        // snapshot because removeFromSuperview mutates the
        // subviews array we'd otherwise iterate.
        let subviews_arr: *mut NSObject = unsafe { msg_send![view, subviews] };
        if subviews_arr.is_null() {
            return;
        }
        // NSArray copy → safe-to-iterate snapshot.
        let copy: Retained<NSObject> = unsafe {
            msg_send_id![subviews_arr, copy]
        };
        let count: usize = unsafe { msg_send![&copy, count] };
        for i in 0..count {
            let sub_ptr: *mut NSView = unsafe { msg_send![&copy, objectAtIndex: i] };
            if sub_ptr.is_null() {
                continue;
            }
            // Mirror iOS's clear_children Taffy sync (see
            // [[project_ios_clear_children_taffy_sync]]): detach the
            // Taffy edge BEFORE removeFromSuperview so a stale
            // child-set + cached parent size doesn't drive a ghost
            // layout on the next pass.
            let sub_ref: &NSView = unsafe { &*sub_ptr };
            if let (Some(p_layout), Some(c_layout)) =
                (self.layout_of(view), self.layout_of(sub_ref))
            {
                self.layout.remove_child(p_layout, c_layout);
                self.layout.mark_dirty(p_layout);
            }
            let _: () = unsafe { msg_send![sub_ptr, removeFromSuperview] };
        }
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let view = node.as_view();
        apply_style_to_view(view, style);

        // Gradient install lives outside `apply_style_to_view` because
        // we need to stash the returned state in the per-view cache —
        // the helper module returns the layer + per-stop sRGB cache
        // for later `set_animated_gradient_stop` writes.
        if let Some(g) = &style.background_gradient {
            if let Some(state) = gradient::install_gradient(view, g) {
                let key = view as *const NSView as usize;
                self.gradient_states.insert(key, state);
            }
        }

        // Static `transform: [...]` from the style. Px translates,
        // scale, and rotate go straight into the CALayer transform;
        // percent translates are stashed for resolution after layout
        // (bounds not known here). `sync_transform_after_layout` in
        // the layout pass re-applies once frames are computed.
        animated::apply_static_transform(node, style, &mut self.animated_states);

        // Push the framework's style into Taffy so flex props
        // (direction, gap, justify, align, width/height) flow into
        // the layout pass. `LayoutTree::set_style` accepts
        // `&StyleRules` directly — the translation lives inside
        // `runtime-layout`.
        if let Some(layout_node) = self.layout_of(view) {
            self.layout.set_style(layout_node, style);
        }

        // Per-node-type text styling. Labels and text views need
        // font/color/alignment applied directly to the AppKit
        // widget; generic views skip this. Mirrors the iOS
        // `match node { IosNode::Label(_) => apply_text_style(...) }`
        // pattern.
        match node {
            MacosNode::Label(_) => {
                text_style::apply_text_style(view, style, true, &self.font_registry);
                // Label height depends on font + width; both can
                // change via apply_style. Dirty the layout node so
                // the measure_fn runs again.
                if let Some(layout_node) = self.layout_of(view) {
                    self.layout.mark_dirty(layout_node);
                }
            }
            MacosNode::View(_) => {}
        }
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        animated::set_animated_f32(node, prop, value, &mut self.animated_states);
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        // Gradient stop writes need the per-view cache (layer +
        // sRGB stops), so they route here at the backend level
        // rather than into the standalone `animated::set_animated_color`
        // helper. Background / foreground writes still go through
        // the helper.
        if let runtime_core::animation::AnimProp::GradientStopColor(idx) = prop {
            let view = node.as_view();
            let key = view as *const NSView as usize;
            if let Some(state) = self.gradient_states.get_mut(&key) {
                gradient::set_animated_gradient_stop(state, idx as usize, value);
            }
            return;
        }
        animated::set_animated_color(node, prop, value);
    }

    // -----------------------------------------------------------------
    // Accessibility
    // -----------------------------------------------------------------
    //
    // `dump_accessibility_tree` stays at its default (returns `None`).
    // AppKit walks each NSView's NSAccessibility attributes directly —
    // there's no parallel semantics tree to dump.

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y_props: &runtime_core::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_core::accessibility::Role>,
    ) {
        a11y::apply(node, a11y_props, inferred_role);
    }

    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: runtime_core::accessibility::LiveRegionPriority,
    ) {
        a11y::announce(msg, priority);
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        handles::make_view_handle(node)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        handles::make_text_handle(node)
    }

    fn finish(&mut self, root: Self::Node) {
        // Compute layout against the host's bounds and walk every
        // registered view to assign frames. The iOS backend does the
        // same thing with `apply_frames`; this is the AppKit-flavored
        // equivalent.
        let host = match &self.host_root {
            Some(v) => v.clone(),
            None => {
                // Without a host root, treat the root node's NSView
                // as the host. Useful for one-off rendering tests.
                let view = root.as_view();
                unsafe {
                    Retained::retain(view as *const NSView as *mut NSView).expect("retain root")
                }
            }
        };

        // Parent the root view to the host's content view. The
        // framework's walker calls `insert(parent, child)` only for
        // child-of-child relationships within the user's primitive
        // tree — the *top* node has no framework parent, so it never
        // becomes a subview of the host without this explicit
        // `addSubview`. iOS does the same with `pin_to_edges`; AppKit
        // doesn't need Auto Layout here because we drive frames via
        // Taffy.
        let root_view = root.as_view();
        if !std::ptr::eq(root_view as *const NSView, &*host as *const NSView) {
            // Check if already a subview to avoid double-parenting on
            // re-renders. AppKit's `addSubview:` is idempotent for the
            // same parent (it reorders rather than dupes), but the
            // logical contract is clearer if we no-op when already
            // mounted.
            let superview: *mut NSView = unsafe { msg_send![root_view, superview] };
            if superview != (&*host as *const NSView as *mut NSView) {
                unsafe { host.addSubview(root_view) };
            }
        }

        // Read viewport from host bounds. If the host hasn't laid out
        // yet (bounds == 0×0 at first render before the window is
        // visible), fall back to the host's frame, then the window's
        // contentRectForFrameRect. The bail-on-zero in the iOS
        // backend works because UIKit lays out before render; AppKit
        // can run render before the first window paint.
        let mut bounds: CGRect = unsafe { msg_send![&host, bounds] };
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            let frame: CGRect = unsafe { msg_send![&host, frame] };
            bounds = frame;
        }
        let viewport = runtime_layout::Size {
            width: bounds.size.width as f32,
            height: bounds.size.height as f32,
        };
        if viewport.width <= 0.0 || viewport.height <= 0.0 {
            // Still nothing — nothing to compute against. The next
            // window resize will trigger a layout pass with real
            // bounds; bail rather than feeding Taffy zeros.
            backend_apple_core::log::apple_log(
                "[macos] finish: viewport is zero, skipping layout pass",
            );
            return;
        }

        // Compute layout starting from the root node.
        let Some(root_layout) = self.layout_of(root_view) else {
            return;
        };
        self.layout
            .compute(root_layout, viewport.width, viewport.height);

        // Apply frames to every registered view. We don't recurse
        // through `NSView.subviews` because some views may not yet
        // be attached at finish time (matches the iOS rationale).
        let snapshot: Vec<(usize, runtime_layout::LayoutNode)> = self
            .view_to_layout
            .iter()
            .map(|(k, (_, n))| (*k, *n))
            .collect();
        for (key, layout_node) in snapshot {
            let frame = self.layout.frame_of(layout_node);
            // The map's retained NSView is the source of truth for
            // applying the frame — we keep that strong ref so the
            // view is alive while we touch it.
            let Some((view, _)) = self.view_to_layout.get(&key) else {
                continue;
            };
            // Static percent translates (`transform: translate(50%, -50%)`
            // from style rules) are applied as frame-origin offsets
            // here rather than via layer.transform. Layer-backed
            // NSViews don't honor pure-static transform translates
            // reliably; baking them into the frame is unambiguous.
            // See `animated::static_translate_offset` for details.
            let (static_tx, static_ty) = animated::static_translate_offset(
                view,
                &self.animated_states,
                frame.width,
                frame.height,
            );
            let rect = CGRect {
                origin: objc2_foundation::CGPoint {
                    x: frame.x as f64 + static_tx,
                    y: frame.y as f64 + static_ty,
                },
                size: CGSize {
                    width: frame.width as f64,
                    height: frame.height as f64,
                },
            };
            let _: () = unsafe { msg_send![&**view, setFrame: rect] };
            // Sync any gradient sublayer to the new bounds. CALayer
            // autoresizingMask doesn't drive automatic sublayer
            // resizing in practice — mirror the resize here. No-op
            // when the view has no `idealyst_gradient` sublayer.
            gradient::sync_gradient_sublayer(view);
            // Re-apply a deferred cornerRadius (stashed when apply_style
            // ran before bounds were known). Clamps to half the
            // smaller bound — required so `border-radius: 999px` on a
            // percent-sized view ("make it a circle") actually
            // renders instead of blanking the layer. No-op when no
            // deferred radius was stashed.
            sync_corner_radius(view);
            // Re-resolve any percent transforms against the new
            // bounds. The sun glare's wrapper uses `translate(50%,
            // -50%)` to offset itself off-screen; that resolves to
            // 0 at apply-style time (bounds 0×0) and only becomes
            // correct here, post-layout. No-op for views with
            // identity transforms.
            animated::sync_transform_after_layout(view, &self.animated_states);
        }
    }

    // ---------------------------------------------------------------
    // Asset / typeface registry hooks. Same forwarding as iOS, just
    // routed through the cross-Apple `FontRegistry` lifted in Phase 0.
    // ---------------------------------------------------------------

    fn register_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
        source: &runtime_core::AssetSource,
    ) {
        // Font branch handled by apple-core; image branch will land
        // alongside `create_image`. For now non-font assets are no-ops
        // so user apps with image assets compile and run without
        // crashing — the images won't display until image.rs lands.
        let _ = self.font_registry.register_asset(id, kind, source);
    }

    fn unregister_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
    ) {
        self.font_registry.unregister_asset(id, kind);
    }

    fn register_typeface(
        &mut self,
        id: runtime_core::assets::TypefaceId,
        family_name: &str,
        faces: &[runtime_core::assets::TypefaceFace],
        fallback: runtime_core::assets::SystemFallback,
    ) {
        self.font_registry
            .register_typeface(id, family_name, faces, fallback);
    }

    fn unregister_typeface(&mut self, id: runtime_core::assets::TypefaceId) {
        self.font_registry.unregister_typeface(id);
    }
}

