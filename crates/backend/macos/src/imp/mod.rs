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
pub(crate) mod callbacks;
pub(crate) mod gradient;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod node;
pub(crate) mod text_style;
pub(crate) mod view;
pub(crate) mod virtualizer;

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
    /// Decoded `NSImage` cache keyed by `AssetId`. Populated by
    /// `register_asset` for `AssetTag::Image`; queried by
    /// `create_image` / `update_image_src` when the framework hands
    /// us an `asset://{id}` src.
    pub(crate) image_cache: image::ImageCache,
    /// Third-party `Primitive::External` registry. Populated by
    /// `register_external::<T>(...)` calls from per-platform leaf
    /// crates (e.g. a future `maps-macos::register`). `create_external`
    /// looks up the handler by payload TypeId; unregistered kinds
    /// fall through to a "not supported" placeholder NSTextField.
    /// Mirrors the iOS pattern.
    pub(crate) external_handlers: runtime_core::ExternalRegistry<MacosBackend>,
    /// Per-virtualizer side state. NSCollectionView's `dataSource`
    /// and `delegate` are weak refs, so the data source needs to
    /// outlive the collection view via this map. Keyed by the
    /// outer NSScrollView's pointer (the mountable node).
    /// `release_virtualizer` removes the entry; on map removal the
    /// data source's Retained drops + the underlying Objective-C
    /// release count goes to zero.
    pub(crate) virtualizer_instances:
        HashMap<usize, virtualizer::VirtualizerInstance>,
    /// Registry of `Primitive::Navigator` handler factories. SDK
    /// leaf crates (`stack_navigator::register`, `tab_navigator::
    /// register`, `drawer_navigator::register`, …) install factories
    /// keyed by their presentation TypeId at app bootstrap. The
    /// macOS handlers — once added to those SDKs — implement the
    /// single-window-with-sidebar shape per
    /// `project_macos_navigator_design`. Until those SDK leaves
    /// land, `create_navigator` falls through to a "kind not
    /// registered" placeholder text node.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<MacosBackend>,
    /// Per-navigator-instance handler. Mirrors iOS's
    /// `nav_handler_instances`. Keyed by the navigator container's
    /// NSView pointer. `create_navigator` resolves the factory,
    /// runs `init`, stashes the handler here so subsequent
    /// `navigator_attach_initial` / `release_navigator` /
    /// `apply_navigator_slot_style` trait methods can route through
    /// it.
    pub(crate) nav_handler_instances: HashMap<
        usize,
        std::rc::Rc<
            std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<MacosBackend>>>,
        >,
    >,
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

/// Re-enter the globally-installed backend from a deferred closure
/// (microtask, NSTimer callback, NotificationCenter observer, …).
/// Used by SDK leaves (drawer/stack/tab navigator handlers) whose
/// per-frame work needs `&mut MacosBackend` but runs outside the
/// framework's borrow window.
///
/// Quietly no-ops if no backend is installed (pre-render), if the
/// install has been dropped (post-teardown), or if the backend is
/// already borrowed (call site re-entered itself; the outer borrow
/// will complete its own work).
///
/// Mirrors `backend_terminal::with_global_backend` and
/// `backend_ios::with_backend`.
pub fn with_global_backend<F: FnOnce(&mut MacosBackend)>(f: F) {
    let weak = MACOS_BACKEND_SELF.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let borrow = rc.try_borrow_mut();
    if let Ok(mut b) = borrow {
        f(&mut *b);
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
            image_cache: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            virtualizer_instances: HashMap::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: HashMap::new(),
        }
    }

    /// Register a `Primitive::Navigator` handler factory keyed by
    /// the presentation type `P`. SDK leaf crates call this once at
    /// bootstrap. Mirrors `IosBackend::register_navigator`.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<MacosBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }

    /// Register a handler for the third-party external primitive whose
    /// payload type is `T`. Called by per-platform leaf crates (e.g.
    /// a future `maps_macos::register`) during app bootstrap. The
    /// handler receives the typed payload plus a mutable borrow of
    /// the backend and produces the `MacosNode` to mount.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut MacosBackend) -> MacosNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    /// Useful for opt-in graceful degradation in user code (render a
    /// static fallback if the SDK isn't available on macOS).
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// SDK extension helper: register an NSView (or subclass) with
    /// the backend's Taffy layout tree so flex parents can size +
    /// position it. Third-party `register_external` handlers call
    /// this once after constructing their native view so the layout
    /// pass picks it up. Without it, the view is laid out as 0×0.
    ///
    /// The view's `frame` is written by the layout pass — leaf
    /// widgets that don't need a custom measure function are fully
    /// serviced by this call alone. Mirrors
    /// `IosBackend::register_external_view`.
    pub fn register_external_view(&mut self, view: &NSView) {
        let _ = self.layout_for_view(view);
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

    /// Install a Taffy `measure_fn` for an `NSImageView` so flex
    /// layout reads its `intrinsicContentSize` (driven by the
    /// assigned `NSImage.size`) instead of collapsing to 0×0.
    /// Re-installable — `update_image_src` calls this again after
    /// swapping the image so a new bitmap's size is picked up.
    ///
    /// Mirrors `IosBackend::install_image_measure` per the
    /// `project_ios_intrinsic_size_measurer` memory.
    pub(crate) fn install_image_measure(&mut self, view: &Retained<NSView>) {
        let layout = self.layout_for_view(view);
        let view_for_measure = view.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&view_for_measure, intrinsicContentSize] };
                // `intrinsicContentSize` returns `{-1, -1}`
                // (NSViewNoIntrinsicMetric) when no image is set or
                // for views without a natural size. Clamp negative
                // dimensions to 0 so Taffy doesn't try to size a
                // slot against an impossible value.
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
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
/// Inert `NavigatorOps` for `make_navigator_handle` callers that
/// land on a navigator container with no registered handler. The
/// trait is empty, so the inert impl is just a marker; the handle
/// constructed against it ignores all dispatch attempts.
struct NoopNavOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;

/// `true` if `view` is an `NSScrollView`. Used by `insert` to
/// redirect children into the scroll view's documentView so
/// the scroll machinery actually works. Implemented via
/// `isKindOfClass:` so subclasses (rare but possible) are also
/// recognized.
fn is_scroll_view(view: &NSView) -> bool {
    let cls = match objc2::runtime::AnyClass::get("NSScrollView") {
        Some(c) => c,
        None => return false,
    };
    unsafe { msg_send![view, isKindOfClass: cls] }
}

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
        on_click: &runtime_core::Action,
        _leading_icon: Option<&runtime_core::IconData>,
        _trailing_icon: Option<&runtime_core::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real NSButton. `+[NSButton buttonWithTitle:target:action:]`
        // produces a system-styled push button (rounded bezel, system
        // font) and wires the target/action in one call. Leading /
        // trailing icons are not yet rendered — NSButton's image slot
        // is single-position; we'd need a custom container view to
        // composite icon + label like UIButton's intrinsic layout.
        // Tracked as a follow-up; the label-only call site is what
        // user code hits today.
        let ns_title = NSString::from_str(label);
        let target = callbacks::CallbackTarget::new(self.mtm, on_click.fire.clone());
        let action = objc2::sel!(invoke:);
        let button: Retained<objc2_app_kit::NSButton> = unsafe {
            msg_send_id![
                objc2::class!(NSButton),
                buttonWithTitle: &*ns_title,
                target: &*target,
                action: action,
            ]
        };
        // Keep the target alive — NSButton holds it as a weak ref.
        // Once the backend drops, the Vec drops, the target drops; by
        // that point the button has also been dropped from the view
        // tree, so the weak ref is irrelevant.
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });
        // Upcast NSButton → NSView via the ObjC class hierarchy
        // (NSButton : NSControl : NSView). Same trick as the
        // NSTextField → NSView upcast in `create_text`.
        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&button) as *mut NSView)
                .expect("retain NSButton as NSView")
        };
        // Install an intrinsic-size measurer so Taffy gives the
        // button a sensible default rect. NSButton has a real
        // `intrinsicContentSize` (computed from label + bezel
        // padding); read it through the measure_fn so style-driven
        // sizes can still override.
        let layout = self.layout_for_view(&view);
        let button_for_measure = button.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&button_for_measure, intrinsicContentSize] };
                runtime_layout::Size {
                    width: known_dimensions
                        .width
                        .unwrap_or((intrinsic.width as f32).ceil()),
                    height: known_dimensions
                        .height
                        .unwrap_or((intrinsic.height as f32).ceil()),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Button,
            ),
        );
        node
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        // NSButton's title is set via `setTitle:` (NSString). Same
        // selector works whether the button was created by us or by
        // someone else's code — Obj-C dispatch on the concrete class.
        let view = node.as_view();
        let ns = NSString::from_str(label);
        let _: () = unsafe { msg_send![view, setTitle: &*ns] };
        // Title change can shift the intrinsic width; mark the layout
        // node dirty so the next pass picks up the new intrinsic size.
        if let Some(layout) = self.layout_of(view) {
            self.layout.mark_dirty(layout);
        }
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = image::create_image(&self.image_cache, src, alt);
        if let MacosNode::View(view) = &node {
            let view_clone = view.clone();
            self.install_image_measure(&view_clone);
        }
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::Image),
        );
        node
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        image::update_image_src(node, &self.image_cache, src);
        if let MacosNode::View(view) = node {
            // Image swap can change intrinsicContentSize; re-mark the
            // measurer so the next layout pass picks up the new size.
            let view_clone = view.clone();
            self.install_image_measure(&view_clone);
        }
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Build an editable NSTextField. `+[NSTextField alloc, init]`
        // gives a bezeled, editable field by default — the same shape
        // as iOS's `UITextField::new` (which starts editable but
        // unbordered; we set `setBorderStyle: 3` for the round-rect
        // bezel on iOS). On macOS the default bezel is appropriate.
        let field: Retained<objc2_app_kit::NSTextField> = unsafe {
            msg_send_id![msg_send_id![objc2::class!(NSTextField), alloc], init]
        };

        // Set initial value + placeholder. `setStringValue:` writes
        // the editable string; `setPlaceholderString:` (on the field
        // directly, not the cell, since macOS 10.10) shows when
        // empty.
        let ns_val = NSString::from_str(initial_value);
        let _: () = unsafe { msg_send![&field, setStringValue: &*ns_val] };
        if let Some(ph) = placeholder {
            let ns_ph = NSString::from_str(ph);
            let _: () = unsafe { msg_send![&field, setPlaceholderString: &*ns_ph] };
        }

        // Wire change notification. AppKit fires
        // `NSControlTextDidChangeNotification` (object = the field)
        // on every keystroke; route through the
        // `StringCallbackTarget` whose `controlTextDidChange:`
        // method reads the sender's `stringValue` and forwards.
        let target =
            callbacks::StringCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(controlTextDidChange:);
        let notification_name = NSString::from_str("NSControlTextDidChangeNotification");
        let center: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSNotificationCenter),
                defaultCenter
            ]
        };
        let _: () = unsafe {
            msg_send![
                &center,
                addObserver: &*target,
                selector: sel,
                name: &*notification_name,
                object: &*field,
            ]
        };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        // Upcast NSTextField → NSView via the ObjC class hierarchy.
        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&field) as *mut NSView)
                .expect("retain NSTextField as NSView")
        };

        // Intrinsic-size measurer. NSTextField's
        // `intrinsicContentSize` reports a sensible default height
        // and (for the editable variant) ~100pt width — same shape
        // as the Button measurer.
        let layout = self.layout_for_view(&view);
        let field_for_measure = field.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&field_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );

        let node = MacosNode::View(view);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let view = node.as_view();
        // Read the current `stringValue` first; only write if it
        // differs, otherwise we re-fire `controlTextDidChange:` and
        // loop with the framework's reactive update.
        let current: *mut NSString = unsafe { msg_send![view, stringValue] };
        if !current.is_null() {
            let current_ref: &NSString = unsafe { &*current };
            if current_ref.to_string() == value {
                return;
            }
        }
        let ns = NSString::from_str(value);
        let _: () = unsafe { msg_send![view, setStringValue: &*ns] };
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // NSTextView is the multi-line editable text widget. It's
        // typically embedded in an NSScrollView for clipping +
        // scrollbars; for v1 we mount it bare and let the layout
        // pass size it directly. Wrap-in-scroll-view lands as a
        // follow-up alongside the ScrollView wiring.
        let view: Retained<NSView> = unsafe {
            let allocated: *mut objc2::runtime::AnyObject =
                msg_send![objc2::class!(NSTextView), alloc];
            let inited: *mut objc2::runtime::AnyObject = msg_send![allocated, init];
            Retained::from_raw(inited.cast::<NSView>())
                .expect("NSTextView init returned nil")
        };
        let ns_val = NSString::from_str(initial_value);
        let _: () = unsafe { msg_send![&view, setString: &*ns_val] };
        let _: () = unsafe { msg_send![&view, setEditable: true] };
        let _: () = unsafe { msg_send![&view, setRichText: false] };

        // Wire change notification.
        // `NSTextDidChangeNotification` fires every edit on the
        // text view; the StringCallbackTarget's `textDidChange:`
        // method reads the sender's `string` and forwards. The
        // `notification.object` filter is the NSTextView itself,
        // so a sibling TextArea's edits don't fire this handler.
        let target = callbacks::StringCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(textDidChange:);
        let notification_name = NSString::from_str("NSTextDidChangeNotification");
        let center: Retained<NSObject> = unsafe {
            msg_send_id![
                objc2::class!(NSNotificationCenter),
                defaultCenter
            ]
        };
        let _: () = unsafe {
            msg_send![
                &center,
                addObserver: &*target,
                selector: sel,
                name: &*notification_name,
                object: &*view,
            ]
        };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        let layout = self.layout_for_view(&view);
        let view_for_measure = view.clone();
        // NSTextView's `intrinsicContentSize` reports its natural
        // text-content size; clamp negatives and bias toward a
        // reasonable default (matches the wgpu TEXT_AREA_DEFAULT_HEIGHT
        // posture — 4× line ≈ 80pt).
        const TEXT_AREA_DEFAULT_HEIGHT: f32 = 80.0;
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&view_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(TEXT_AREA_DEFAULT_HEIGHT);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );

        let node = MacosNode::View(view);
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        let view = node.as_view();
        let ns = NSString::from_str(value);
        let _: () = unsafe { msg_send![view, setString: &*ns] };
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // NSSwitch is the macOS 10.15+ equivalent of UISwitch — same
        // rounded-pill toggle visual. It's an NSControl so
        // target/action wiring is the same pattern as NSButton.
        let switch: Retained<objc2_app_kit::NSSwitch> = unsafe {
            msg_send_id![msg_send_id![objc2::class!(NSSwitch), alloc], init]
        };
        // NSControlStateValueOn = 1, Off = 0.
        let state: isize = if initial_value { 1 } else { 0 };
        let _: () = unsafe { msg_send![&switch, setState: state] };

        let target = callbacks::BoolCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe { msg_send![&switch, setTarget: &*target] };
        let _: () = unsafe { msg_send![&switch, setAction: sel] };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&switch) as *mut NSView)
                .expect("retain NSSwitch as NSView")
        };
        let layout = self.layout_for_view(&view);
        let switch_for_measure = switch.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&switch_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Toggle,
            ),
        );
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        let view = node.as_view();
        let state: isize = if value { 1 } else { 0 };
        // Read first; skip write if the state already matches so the
        // target/action callback doesn't re-fire and loop with the
        // framework's reactive update.
        let current: isize = unsafe { msg_send![view, state] };
        if current == state {
            return;
        }
        let _: () = unsafe { msg_send![view, setState: state] };
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // NSSlider with continuous tracking. `setMinValue:` /
        // `setMaxValue:` / `setDoubleValue:` configure the range +
        // initial. `setContinuous:true` makes the action fire as the
        // user drags (matches UISlider's default on iOS).
        //
        // Step snapping isn't wired yet — NSSlider has
        // `setAltIncrementValue:` for keyboard step but no native
        // continuous-drag snap. Authors expecting `step` semantics
        // get continuous output for now; a wrapper closure could
        // round in the callback if needed.
        let slider: Retained<objc2_app_kit::NSSlider> = unsafe {
            msg_send_id![msg_send_id![objc2::class!(NSSlider), alloc], init]
        };
        let _: () = unsafe { msg_send![&slider, setMinValue: min as f64] };
        let _: () = unsafe { msg_send![&slider, setMaxValue: max as f64] };
        let _: () = unsafe { msg_send![&slider, setDoubleValue: initial_value as f64] };
        let _: () = unsafe { msg_send![&slider, setContinuous: true] };

        let target = callbacks::FloatCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe { msg_send![&slider, setTarget: &*target] };
        let _: () = unsafe { msg_send![&slider, setAction: sel] };
        self.callback_targets.push(unsafe {
            Retained::cast::<NSObject>(target)
        });

        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&slider) as *mut NSView)
                .expect("retain NSSlider as NSView")
        };
        let layout = self.layout_for_view(&view);
        let slider_for_measure = slider.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&slider_for_measure, intrinsicContentSize] };
                // NSSlider's intrinsic width is `-1`
                // (NSViewNoIntrinsicMetric); height is a real value.
                // Default the width to a reasonable touch-friendly
                // size matching iOS's SLIDER_DEFAULT_WIDTH.
                const SLIDER_DEFAULT_WIDTH: f32 = 200.0;
                let w = if intrinsic.width >= 0.0 {
                    intrinsic.width as f32
                } else {
                    SLIDER_DEFAULT_WIDTH
                };
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Slider,
            ),
        );
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        let view = node.as_view();
        let current: f64 = unsafe { msg_send![view, doubleValue] };
        if (current - value as f64).abs() < f64::EPSILON {
            return;
        }
        let _: () = unsafe { msg_send![view, setDoubleValue: value as f64] };
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real two-view shape: outer NSScrollView wraps a
        // `FlippedView` documentView. Children mount inside the
        // documentView (via `insert`'s scroll-view-aware
        // redirect); the NSScrollView's clip view handles
        // overflow + scroll-bar machinery.
        //
        // We return the OUTER NSScrollView as the MacosNode so
        // layout / style / hit-testing target the scrolling
        // container (the framework's mental model). `insert`
        // checks `documentView` and routes children into the
        // documentView's subview tree + the Taffy graph.
        let document_view = FlippedView::new(self.mtm);
        let document_view: Retained<NSView> = Retained::into_super(document_view);

        // Build NSScrollView via alloc/initWithFrame:CGRect.zero.
        // Frame is set by the layout pass in `finish()`.
        let scroll_view: Retained<NSView> = unsafe {
            let allocated: *mut objc2::runtime::AnyObject =
                msg_send![objc2::class!(NSScrollView), alloc];
            let zero_rect = CGRect {
                origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            };
            let inited: *mut objc2::runtime::AnyObject =
                msg_send![allocated, initWithFrame: zero_rect];
            Retained::from_raw(inited.cast::<NSView>())
                .expect("NSScrollView init returned nil")
        };

        // Configure scrollers per axis. Vertical-default matches
        // iOS's default ScrollView. NSScrollView's API:
        //  - setHasVerticalScroller: / setHasHorizontalScroller:
        //  - setAutohidesScrollers: true so the scrollers fade
        //    when not actively scrolling (modern macOS feel).
        let _: () = unsafe {
            msg_send![&scroll_view, setHasVerticalScroller: !horizontal]
        };
        let _: () = unsafe {
            msg_send![&scroll_view, setHasHorizontalScroller: horizontal]
        };
        let _: () = unsafe { msg_send![&scroll_view, setAutohidesScrollers: true] };

        // Install the documentView. NSScrollView retains it; we
        // can drop our local Retained after this call without
        // losing the document.
        let _: () = unsafe { msg_send![&scroll_view, setDocumentView: &*document_view] };

        // Register BOTH views in the layout map. The outer
        // NSScrollView is what Taffy positions; the inner
        // documentView is what `insert` routes children into.
        // The documentView's frame is sized by NSScrollView
        // (via the content size we don't expose here — Taffy
        // controls per-child frames inside the document, and
        // NSScrollView's content size grows to fit).
        let _ = self.layout_for_view(&scroll_view);
        let _ = self.layout_for_view(&document_view);

        // Wire `on_scroll` via NSViewBoundsDidChangeNotification on
        // the scroll view's contentView (the NSClipView). We flip
        // `postsBoundsChangedNotifications` on, then register a
        // ScrollObserverTarget against the default NotificationCenter
        // keyed on the clip view. The target's `boundsDidChange:`
        // selector reads the clip view's bounds.origin and forwards
        // it as the (x, y) scroll offset \u{2014} same units as web
        // (CSS pixels) and iOS (UIKit points), so author code reads
        // identical values across platforms.
        if let Some(cb) = on_scroll {
            let target = crate::imp::callbacks::ScrollObserverTarget::new(self.mtm, cb);
            let clip_view: *mut objc2::runtime::AnyObject =
                unsafe { msg_send![&scroll_view, contentView] };
            if !clip_view.is_null() {
                let _: () =
                    unsafe { msg_send![clip_view, setPostsBoundsChangedNotifications: true] };
                let center: *mut objc2::runtime::AnyObject =
                    unsafe { msg_send![objc2::class!(NSNotificationCenter), defaultCenter] };
                let name: Retained<objc2_foundation::NSString> =
                    objc2_foundation::NSString::from_str("NSViewBoundsDidChangeNotification");
                let sel = objc2::sel!(boundsDidChange:);
                let _: () = unsafe {
                    msg_send![
                        center,
                        addObserver: &*target,
                        selector: sel,
                        name: &*name,
                        object: clip_view,
                    ]
                };
            }
            // Retain across the backend's lifetime so the observer
            // outlives the scroll view it watches. The notification
            // center holds a non-owning ref \u{2014} same pattern as
            // every other macOS callback target.
            self.callback_targets
                .push(unsafe { Retained::cast::<NSObject>(target) });
        }

        let node = MacosNode::View(scroll_view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::ScrollView,
            ),
        );
        node
    }

    fn create_activity_indicator(
        &mut self,
        size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
        // NSProgressIndicator in spinning style — same shape as
        // UIActivityIndicatorView. `setStyle: 1` =
        // NSProgressIndicatorStyleSpinning. `setIndeterminate: true`
        // is the default for spinning style but we set it
        // explicitly for safety.
        let spinner: Retained<objc2_app_kit::NSProgressIndicator> = unsafe {
            msg_send_id![
                msg_send_id![objc2::class!(NSProgressIndicator), alloc],
                init
            ]
        };
        let _: () = unsafe { msg_send![&spinner, setStyle: 1isize] };
        let _: () = unsafe { msg_send![&spinner, setIndeterminate: true] };
        // Map ActivityIndicatorSize → controlSize. NSControlSize:
        // 0 = regular, 1 = small, 3 = mini. Small for ::Small, large
        // for ::Large (use Regular as the "large" mapping — macOS's
        // spinner doesn't have an explicit large variant).
        let control_size: isize = match size {
            ActivityIndicatorSize::Small => 1,
            ActivityIndicatorSize::Large => 0,
        };
        let _: () = unsafe { msg_send![&spinner, setControlSize: control_size] };
        let _: () = unsafe { msg_send![&spinner, startAnimation: std::ptr::null::<NSObject>()] };

        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&spinner) as *mut NSView)
                .expect("retain NSProgressIndicator as NSView")
        };
        let layout = self.layout_for_view(&view);
        let spinner_for_measure = spinner.clone();
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                let intrinsic: CGSize =
                    unsafe { msg_send![&spinner_for_measure, intrinsicContentSize] };
                let w = (intrinsic.width as f32).max(0.0);
                let h = (intrinsic.height as f32).max(0.0);
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(w),
                    height: known_dimensions.height.unwrap_or(h),
                }
            }),
        );
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::ActivityIndicator,
            ),
        );
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_view = parent.as_view();
        let child_view = child.as_view();

        // ScrollView routing: if the framework's logical parent
        // is an NSScrollView (created via `create_scroll_view`),
        // children mount inside its documentView so they
        // participate in the scroll machinery. `documentView`
        // returns the inner FlippedView we installed. Without
        // this redirect, addSubview would add the child to the
        // scroll view's clip view at fixed coordinates and the
        // scroll wouldn't take effect.
        let target_view: Retained<NSView> = if is_scroll_view(parent_view) {
            let doc_ptr: *mut NSView =
                unsafe { msg_send![parent_view, documentView] };
            if doc_ptr.is_null() {
                // Defensive: fall back to the scroll view itself if
                // documentView is somehow nil. Shouldn't happen
                // because `create_scroll_view` always installs one.
                unsafe { Retained::retain(parent_view as *const NSView as *mut NSView) }
                    .unwrap()
            } else {
                unsafe { Retained::retain(doc_ptr) }
                    .expect("NSScrollView documentView retain")
            }
        } else {
            unsafe { Retained::retain(parent_view as *const NSView as *mut NSView) }
                .unwrap()
        };

        // AppKit: `addSubview:` mounts the child. Frame is determined
        // by the layout pass in `finish()`; here we just establish
        // the parent/child relationship in both the view tree AND
        // the Taffy tree.
        unsafe { target_view.addSubview(child_view) };

        // Mirror the parenting in Taffy against the same logical
        // target — children of a ScrollView live under its
        // documentView in Taffy too, so the layout pass sizes
        // them inside the document's coordinate space rather
        // than the outer scroll view's clip rect.
        let parent_layout = self.layout_for_view(&target_view);
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

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real NSCollectionView wrap with cell reuse — see
        // `imp/virtualizer.rs` for the AppKit-flavored adapter.
        // Mirrors iOS's UICollectionView pattern: data source +
        // delegate routed via a custom NSObject, items dequeued
        // from a reuse pool, per-item Scope released on
        // displayEnd / reuse / teardown.
        let view = virtualizer::create(
            self.mtm,
            &mut self.virtualizer_instances,
            callbacks,
            overscan,
            horizontal,
        );
        let _ = self.layout_for_view(&view);
        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::List),
        );
        node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Tell the NSCollectionView to `reloadData`. Future
        // performBatchUpdates optimisation lands when the iOS
        // counterpart does — same shape (key-keyed diff, batched
        // insert/remove ops).
        let view = node.as_view();
        virtualizer::data_changed(view);
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        // Mirrors iOS's teardown — disconnect dataSource/delegate so
        // queued AppKit events become no-ops, drain mounted scopes,
        // drop the side-state entry. Without this, AppKit's lingering
        // layout pass after the framework's scope drop would invoke
        // released `Signal` closures and panic.
        let view = node.as_view();
        virtualizer::release(&mut self.virtualizer_instances, view);
    }

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Consult the registry. SDK leaf crates (`drawer_navigator`,
        // `stack_navigator`, `tab_navigator`) call
        // `register_navigator::<TheirPresentation, _>(factory)` at
        // bootstrap; when one matches, we run `init` and stash the
        // handler under the returned view's pointer for subsequent
        // `navigator_attach_initial` / `release_navigator` /
        // `apply_navigator_slot_style` dispatches.
        //
        // No-match: render a visible placeholder noting WHICH kind
        // wasn't registered. This is the path for navigator SDKs
        // that haven't shipped a macOS handler yet — author code
        // running on macOS sees the missing wiring at runtime
        // (matching the External + placeholder posture across the
        // workspace; see `feedback_cpu_unsupported_placeholders`).
        if let Some(factory) = self.navigator_handlers.get(type_id) {
            let mut handler: Box<dyn runtime_core::NavigatorHandler<MacosBackend>> =
                (factory)();
            let node = handler.init(self, host, presentation);

            // Stash the handler keyed by the resolved node's NSView
            // pointer. Future trait calls
            // (`navigator_attach_initial`, etc.) look the handler up
            // by the same key.
            let view = node.as_view();
            let key = view as *const NSView as usize;
            self.nav_handler_instances.insert(
                key,
                std::rc::Rc::new(std::cell::RefCell::new(handler)),
            );

            a11y::apply(&node, a11y, None);
            return node;
        }

        let text = format!(
            "Navigator kind \"{type_name}\" not registered for macOS \
             — SDK leaf needs `register_navigator(&mut backend)` \
             on macOS targets (per `project_macos_navigator_design`)"
        );
        let ns = NSString::from_str(&text);
        let label: Retained<NSTextField> = unsafe {
            msg_send_id![objc2::class!(NSTextField), labelWithString: &*ns]
        };
        let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
        let _: () = unsafe { msg_send![&cell, setWraps: true] };
        let _: () = unsafe { msg_send![&cell, setUsesSingleLineMode: false] };
        let red: Retained<NSColor> = unsafe {
            msg_send_id![objc2::class!(NSColor), systemRedColor]
        };
        let _: () = unsafe { msg_send![&label, setTextColor: &*red] };
        let view: Retained<NSView> = unsafe {
            Retained::retain(Retained::as_ptr(&label) as *mut NSView)
                .expect("retain NSTextField as NSView")
        };
        let _ = self.layout_for_view(&view);
        let node = MacosNode::Label(label);
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let view = node.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.remove(&key) {
            // Run the SDK's `release` so it can drop native
            // resources. The handler's Box drops after the
            // borrow_mut block returns (the Rc's strong count
            // falls to zero once the map entry is gone).
            handler_cell.borrow_mut().release(self);
        }
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let view = navigator.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key).cloned() {
            handler_cell
                .borrow_mut()
                .attach_initial(self, screen, scope_id, options);
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let view = node.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key).cloned() {
            handler_cell
                .borrow_mut()
                .apply_slot_style(self, slot, style);
        }
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::navigator::NavigatorHandle {
        let view = node.as_view();
        let key = view as *const NSView as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key) {
            return handler_cell.borrow().make_handle();
        }
        // No handler — return the trait's default inert handle.
        // Author code that bound a `Ref<NavigatorHandle>` against
        // an unregistered navigator kind silently gets a no-op
        // handle (matches what the trait default would do).
        runtime_core::primitives::navigator::NavigatorHandle::new(
            std::rc::Rc::new(()),
            &NOOP_NAV_OPS,
        )
    }

    fn create_graphics(
        &mut self,
        on_ready: runtime_core::primitives::graphics::OnReady,
        on_resize: runtime_core::primitives::graphics::OnResize,
        on_lost: runtime_core::primitives::graphics::OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // CAMetalLayer-backed NSView wrapping a wgpu Surface.
        // Mirrors the iOS `imp/graphics.rs` pattern — MetalView
        // (NSView subclass with `-makeBackingLayer` returning
        // CAMetalLayer) + `raw_window_handle::AppKitWindowHandle`
        // provider + scheduled `on_ready` callback fired on the
        // next runloop turn once AppKit's layout has assigned a
        // real frame.
        let node = graphics::create_graphics(
            self.mtm,
            &mut self.callback_targets,
            on_ready,
            on_resize,
            on_lost,
        );
        if let MacosNode::View(view) = &node {
            let _ = self.layout_for_view(view);
        }
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Real vector-path rendering via `CAShapeLayer` with a
        // CGPath built by the shared SVG parser in
        // `backend_apple_core::icon_path`. Identical render output
        // to iOS — both backends now drive off the same parser, so
        // a Lucide icon looks the same on iOS and macOS without
        // duplicated path-handling code.
        let view = icon::create_icon(self.mtm, data, color);

        // Pin a 24×24 default intrinsic size so flex layout gives
        // the icon a stable footprint matching iOS. Style-driven
        // overrides on `width`/`height` still win because
        // known_dimensions short-circuits the closure body.
        const ICON_SIZE: f32 = 24.0;
        let layout = self.layout_for_view(&view);
        self.layout.set_measure_fn(
            layout,
            Rc::new(move |known_dimensions, _available_space| {
                runtime_layout::Size {
                    width: known_dimensions.width.unwrap_or(ICON_SIZE),
                    height: known_dimensions.height.unwrap_or(ICON_SIZE),
                }
            }),
        );

        let node = MacosNode::View(view);
        a11y::apply(
            &node,
            a11y,
            runtime_core::accessibility::default_role(
                runtime_core::accessibility::PrimitiveKind::Icon,
            ),
        );
        node
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        icon::update_icon_color(node, color);
    }

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // v1: full-viewport overlay attached to the host window's
        // contentView. Anchor positioning, scrim styling, named-slot
        // routing, on_dismiss event firing, and focus trapping are
        // deferred — match iOS's portal_instances surface when those
        // land. Today this is enough to keep author code mounting a
        // `<Portal>` from panicking; the subtree appears as a
        // top-of-window overlay.
        let _ = target; // No per-target behavior in v1.

        let content = FlippedView::new(self.mtm);
        let content: Retained<NSView> = Retained::into_super(content);

        // Attach to the host window's contentView if available. The
        // overlay is added as the topmost subview so it draws above
        // the rest of the tree. If no host yet (mid-build), defer —
        // the layout pass / `set_host_root` will attach the parent
        // chain later. Here we still return the orphan content view
        // so the framework can `insert` children into it.
        if let Some(host) = self.host_root.as_ref() {
            unsafe { host.addSubview(&content) };
        }

        // Register in the layout tree as a Taffy root. Match what
        // iOS does for FullScreen portals: the content fills the
        // viewport, so its style is a full-flex container. Anchor
        // positioning is a future addition.
        let layout_node = self.layout_for_view(&content);
        // Default style is the framework-default (full-stretch);
        // anchor-driven positioning lands when the anchor module
        // does, mirroring `portal::container_style_for_anchor`.
        let _ = layout_node;

        // Hold a strong ref so a future `release_portal` can
        // detach it cleanly. Without a side-map entry, removal
        // would require walking the tree.
        let node = MacosNode::View(content);
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_portal(&mut self, node: &Self::Node) {
        // v1 cleanup: detach the overlay from its superview. Once
        // `portal_instances` lands (mirroring iOS's per-portal
        // entry map), we'll also drop any KVO observers / dismiss
        // gesture recognizers attached at create time.
        let view = node.as_view();
        unsafe { view.removeFromSuperview() };
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = if let Some(handler) = self.external_handlers.get(type_id) {
            handler(payload, self)
        } else {
            // No handler registered — render an explicit placeholder
            // so the missing SDK binding is visible at run-time. The
            // user-facing pattern for graceful degradation is
            // `backend.has_external::<T>()` BEFORE building the
            // primitive; this placeholder is the safety net for code
            // that mounts unconditionally.
            external_placeholder_node(self, type_name)
        };
        // Third-party externals declare their own role via
        // `props.role` if needed — we don't infer one here.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_external(&mut self, _node: &Self::Node) {
        // No per-external bookkeeping today. Future SDK leaves that
        // hold instance state (KVO observers, CADisplayLink-equivalent,
        // etc.) would clean up here, keyed by view pointer like
        // portals do on iOS.
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
        // Font branch routes through `apple-core`'s shared registry;
        // image branch decodes `NSImage` into the per-backend cache
        // for `create_image` to resolve. Other asset tags are no-op
        // here (registered by future SDK leaves on demand).
        let _ = self.font_registry.register_asset(id, kind, source);
        image::register_asset(&mut self.image_cache, id, kind, source);
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

/// Build a visible "External X not supported on macOS" placeholder
/// for `create_external` when no handler is registered for the given
/// payload type. Mirrors the iOS placeholder pattern — single line
/// of red text so the dev / user immediately sees that a SDK binding
/// is missing rather than hitting an invisible empty rect.
fn external_placeholder_node(b: &mut MacosBackend, type_name: &'static str) -> MacosNode {
    let text = format!("External \"{type_name}\" not supported on macOS");
    let ns = NSString::from_str(&text);
    let label: Retained<NSTextField> = unsafe {
        msg_send_id![objc2::class!(NSTextField), labelWithString: &*ns]
    };
    // Multi-line wrap so a long type name (e.g. `webview::WebViewProps`)
    // doesn't get clipped.
    let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
    let _: () = unsafe { msg_send![&cell, setWraps: true] };
    let _: () = unsafe { msg_send![&cell, setUsesSingleLineMode: false] };
    // System red — matches iOS placeholder intent.
    let red: Retained<NSColor> = unsafe {
        msg_send_id![objc2::class!(NSColor), systemRedColor]
    };
    let _: () = unsafe { msg_send![&label, setTextColor: &*red] };

    let view: Retained<NSView> = unsafe {
        Retained::retain(Retained::as_ptr(&label) as *mut NSView)
            .expect("retain NSTextField as NSView")
    };
    let _ = b.layout_for_view(&view);
    MacosNode::Label(label)
}

