//! iOS backend: builds UIKit views via objc2.
//!
//! Real `objc2-ui-kit` calls under `target_os = "ios"`;
//! a stub on other hosts so the crate type-checks during cross-compile.

use framework_core::{Backend, StyleRules};
use std::rc::Rc;

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use framework_core::primitives::activity_indicator::ActivityIndicatorSize;
    use framework_core::primitives::graphics::{
        GraphicsSurface, OnLost, OnReady, OnReadyEvent, OnResize,
    };
    use framework_core::primitives::link::LinkConfig;
    use framework_core::primitives::navigator::{
        NavCommand, NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps,
    };
    use framework_core::{Color, Length};
    use objc2::rc::Retained;
    use block2::ConcreteBlock;
    use objc2::encode::{Encode, Encoding};
    use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
        UiKitDisplayHandle, UiKitWindowHandle, WindowHandle,
    };
    use std::ptr::NonNull;
    use std::sync::Arc;

    /// Opaque wrapper for CoreGraphics' `CGColorRef` so `msg_send!`'s
    /// debug-mode encoding check sees `^{CGColor=}` instead of `^v`.
    #[repr(transparent)]
    #[derive(Clone, Copy)]
    struct CGColorRef(*const std::ffi::c_void);

    unsafe impl Encode for CGColorRef {
        const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
    }
    use objc2_foundation::{
        CGFloat, CGRect, CGSize, MainThreadMarker, NSObject, NSString,
    };
    use objc2_ui_kit::{
        UIActivityIndicatorView, UIActivityIndicatorViewStyle, UIButton, UIButtonType, UIColor,
        UILabel, UINavigationController, UIScrollView, UISlider, UIStackView, UISwitch,
        UITextField, UIView, UIViewController,
    };
    use std::cell::RefCell;
    use std::collections::HashMap;

    // =========================================================================
    // Callback helper: ObjC action target that calls a Rust closure
    // =========================================================================
    //
    // UIKit's target/action pattern needs an ObjC object as the target.
    // We define a minimal ObjC class that holds a boxed Rust closure and
    // calls it from its action method.

    struct CallbackTargetIvars {
        callback: RefCell<Option<Rc<dyn Fn()>>>,
    }

    declare_class!(
        struct CallbackTarget;

        unsafe impl ClassType for CallbackTarget {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystCallbackTarget";
        }

        impl DeclaredClass for CallbackTarget {
            type Ivars = CallbackTargetIvars;
        }

        unsafe impl CallbackTarget {
            #[method(invoke)]
            fn invoke(&self) {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    cb();
                }
            }
        }
    );

    impl CallbackTarget {
        fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn()>) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(CallbackTargetIvars {
                callback: RefCell::new(Some(callback)),
            });
            unsafe { msg_send_id![super(this), init] }
        }
    }

    // Variant for bool callbacks (UISwitch)
    struct BoolCallbackTargetIvars {
        callback: RefCell<Option<Rc<dyn Fn(bool)>>>,
    }

    declare_class!(
        struct BoolCallbackTarget;

        unsafe impl ClassType for BoolCallbackTarget {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystBoolCallbackTarget";
        }

        impl DeclaredClass for BoolCallbackTarget {
            type Ivars = BoolCallbackTargetIvars;
        }

        unsafe impl BoolCallbackTarget {
            #[method(invoke:)]
            fn invoke(&self, sender: &NSObject) {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    // Interpret sender as UISwitch to get isOn
                    let is_on: bool = unsafe { msg_send![sender, isOn] };
                    cb(is_on);
                }
            }
        }
    );

    impl BoolCallbackTarget {
        fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(bool)>) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(BoolCallbackTargetIvars {
                callback: RefCell::new(Some(callback)),
            });
            unsafe { msg_send_id![super(this), init] }
        }
    }

    // Variant for f32 callbacks (UISlider)
    struct FloatCallbackTargetIvars {
        callback: RefCell<Option<Rc<dyn Fn(f32)>>>,
    }

    declare_class!(
        struct FloatCallbackTarget;

        unsafe impl ClassType for FloatCallbackTarget {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystFloatCallbackTarget";
        }

        impl DeclaredClass for FloatCallbackTarget {
            type Ivars = FloatCallbackTargetIvars;
        }

        unsafe impl FloatCallbackTarget {
            #[method(invoke:)]
            fn invoke(&self, sender: &NSObject) {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    let value: f32 = unsafe { msg_send![sender, value] };
                    cb(value);
                }
            }
        }
    );

    impl FloatCallbackTarget {
        fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(f32)>) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(FloatCallbackTargetIvars {
                callback: RefCell::new(Some(callback)),
            });
            unsafe { msg_send_id![super(this), init] }
        }
    }

    // Variant for String callbacks (UITextField editing changed)
    struct StringCallbackTargetIvars {
        callback: RefCell<Option<Rc<dyn Fn(String)>>>,
    }

    declare_class!(
        struct StringCallbackTarget;

        unsafe impl ClassType for StringCallbackTarget {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystStringCallbackTarget";
        }

        impl DeclaredClass for StringCallbackTarget {
            type Ivars = StringCallbackTargetIvars;
        }

        unsafe impl StringCallbackTarget {
            #[method(invoke:)]
            fn invoke(&self, sender: &NSObject) {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    let text: Option<Retained<NSString>> = unsafe { msg_send_id![sender, text] };
                    let s = text.map(|ns| ns.to_string()).unwrap_or_default();
                    cb(s);
                }
            }
        }
    );

    impl StringCallbackTarget {
        fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(String)>) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(StringCallbackTargetIvars {
                callback: RefCell::new(Some(callback)),
            });
            unsafe { msg_send_id![super(this), init] }
        }
    }

    // =========================================================================
    // MetalView — UIView subclass backed by CAMetalLayer
    // =========================================================================

    declare_class!(
        struct MetalView;

        unsafe impl ClassType for MetalView {
            type Super = UIView;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "IdealystMetalView";
        }

        impl DeclaredClass for MetalView {
            type Ivars = ();
        }

        unsafe impl MetalView {
            /// Override +layerClass to return [CAMetalLayer class].
            /// This makes the view's backing layer a CAMetalLayer,
            /// which is what wgpu's Metal backend expects.
            #[method(layerClass)]
            fn layer_class() -> *const std::ffi::c_void {
                objc2::class!(CAMetalLayer) as *const _ as *const std::ffi::c_void
            }
        }
    );

    impl MetalView {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(());
            unsafe { msg_send_id![super(this), init] }
        }
    }

    // =========================================================================
    // IosSurfaceProvider — raw_window_handle bridge for wgpu
    // =========================================================================

    struct IosSurfaceProvider {
        view: *mut std::ffi::c_void,
    }

    unsafe impl Send for IosSurfaceProvider {}
    unsafe impl Sync for IosSurfaceProvider {}

    impl HasWindowHandle for IosSurfaceProvider {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let handle = UiKitWindowHandle::new(
                NonNull::new(self.view).expect("null UIView pointer"),
            );
            Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
        }
    }

    impl HasDisplayHandle for IosSurfaceProvider {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(unsafe { DisplayHandle::borrow_raw(UiKitDisplayHandle::new().into()) })
        }
    }

    // =========================================================================
    // IosBackend
    // =========================================================================

    pub struct IosBackend {
        mtm: MainThreadMarker,
        /// The host-provided root view. When set, `finish` adds the
        /// framework's root node as a subview of this view.
        host_root: Option<Retained<UIView>>,
        /// Per-navigator state.
        navigator_instances: HashMap<usize, NavigatorEntry>,
        /// Retained callback targets so they aren't deallocated. Keyed
        /// by the node's view pointer so we can look them up if needed.
        callback_targets: Vec<Retained<NSObject>>,
        /// ScrollView inner containers. `insert()` redirects children
        /// of a scroll view to its inner content view.
        scroll_view_inner: HashMap<usize, Retained<UIView>>,
    }

    pub(crate) struct NavigatorEntry {
        #[allow(dead_code)]
        pub(crate) controller: Retained<UINavigationController>,
        pub(crate) control: Rc<NavigatorControl>,
        #[allow(dead_code)]
        pub(crate) stack: Rc<RefCell<Vec<ScreenEntry>>>,
    }

    pub(crate) struct ScreenEntry {
        pub(crate) vc: Retained<UIViewController>,
        pub(crate) scope_id: u64,
    }

    impl IosBackend {
        pub fn new(mtm: MainThreadMarker) -> Self {
            Self {
                mtm,
                host_root: None,
                navigator_instances: HashMap::new(),
                callback_targets: Vec::new(),
                scroll_view_inner: HashMap::new(),
            }
        }

        pub fn set_host_root(&mut self, view: Retained<UIView>) {
            self.host_root = Some(view);
        }

        /// Retain a callback target so it stays alive.
        fn retain_target<T: objc2::Message>(&mut self, target: &Retained<T>) {
            // Upcast to NSObject for storage.
            let obj: Retained<NSObject> = unsafe {
                let ptr = Retained::as_ptr(target) as *mut NSObject;
                Retained::retain(ptr).unwrap()
            };
            self.callback_targets.push(obj);
        }

        fn node_key(node: &IosNode) -> usize {
            node.as_view() as *const UIView as usize
        }
    }

    // =========================================================================
    // IosNode
    // =========================================================================

    #[derive(Clone)]
    pub enum IosNode {
        View(Retained<UIView>),
        Label(Retained<UILabel>),
        Button(Retained<UIButton>),
        TextField(Retained<UITextField>),
        Switch(Retained<UISwitch>),
        Slider(Retained<UISlider>),
        ScrollView(Retained<UIScrollView>),
        ActivityIndicator(Retained<UIActivityIndicatorView>),
    }

    impl IosNode {
        fn as_view(&self) -> &UIView {
            match self {
                IosNode::View(v) => v,
                IosNode::Label(l) => l,
                IosNode::Button(b) => b,
                IosNode::TextField(t) => t,
                IosNode::Switch(s) => s,
                IosNode::Slider(s) => s,
                IosNode::ScrollView(s) => s,
                IosNode::ActivityIndicator(a) => a,
            }
        }

        fn view_key(&self) -> usize {
            self.as_view() as *const UIView as usize
        }
    }

    // =========================================================================
    // Color parsing helper
    // =========================================================================

    /// Parse a CSS-style color string into (r, g, b, a) in 0.0..1.0.
    fn parse_color(s: &str) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            let hex = hex.trim();
            let chars: Vec<char> = hex.chars().collect();
            match chars.len() {
                3 => {
                    let r = u8::from_str_radix(&format!("{}{}", chars[0], chars[0]), 16).unwrap_or(0);
                    let g = u8::from_str_radix(&format!("{}{}", chars[1], chars[1]), 16).unwrap_or(0);
                    let b = u8::from_str_radix(&format!("{}{}", chars[2], chars[2]), 16).unwrap_or(0);
                    (r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, 1.0)
                }
                6 => {
                    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                    (r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, 1.0)
                }
                8 => {
                    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                    let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
                    (r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, a as CGFloat / 255.0)
                }
                _ => (0.0, 0.0, 0.0, 1.0),
            }
        } else if s.starts_with("rgba(") || s.starts_with("RGBA(") {
            // rgba(r, g, b, a)
            let inner = s.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .trim_end_matches(')');
            let parts: Vec<&str> = inner.split(',').collect();
            if parts.len() >= 4 {
                let r: f64 = parts[0].trim().parse().unwrap_or(0.0);
                let g: f64 = parts[1].trim().parse().unwrap_or(0.0);
                let b: f64 = parts[2].trim().parse().unwrap_or(0.0);
                let a: f64 = parts[3].trim().parse().unwrap_or(1.0);
                (r / 255.0, g / 255.0, b / 255.0, a)
            } else {
                (0.0, 0.0, 0.0, 1.0)
            }
        } else if s.starts_with("rgb(") || s.starts_with("RGB(") {
            let inner = s.trim_start_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .trim_end_matches(')');
            let parts: Vec<&str> = inner.split(',').collect();
            if parts.len() >= 3 {
                let r: f64 = parts[0].trim().parse().unwrap_or(0.0);
                let g: f64 = parts[1].trim().parse().unwrap_or(0.0);
                let b: f64 = parts[2].trim().parse().unwrap_or(0.0);
                (r / 255.0, g / 255.0, b / 255.0, 1.0)
            } else {
                (0.0, 0.0, 0.0, 1.0)
            }
        } else if s == "transparent" {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            (0.0, 0.0, 0.0, 1.0)
        }
    }

    fn color_to_uicolor(color: &Color) -> Retained<UIColor> {
        let (r, g, b, a) = parse_color(&color.0);
        unsafe { UIColor::colorWithRed_green_blue_alpha(r, g, b, a) }
    }

    fn length_to_px(len: &Length) -> CGFloat {
        match len {
            Length::Px(v) => *v as CGFloat,
            Length::Percent(_) => 0.0, // Not directly translatable without parent size
            Length::Auto => 0.0,
        }
    }

    // =========================================================================
    // Style application
    // =========================================================================

    /// Map framework Easing to UIView animation options bitmask.
    fn easing_to_options(easing: &framework_core::Easing) -> u64 {
        // UIViewAnimationOptionCurveEaseInOut = 0 << 16
        // UIViewAnimationOptionCurveEaseIn   = 1 << 16
        // UIViewAnimationOptionCurveEaseOut  = 2 << 16
        // UIViewAnimationOptionCurveLinear   = 3 << 16
        match easing {
            framework_core::Easing::Linear => 3 << 16,
            framework_core::Easing::Ease | framework_core::Easing::EaseInOut => 0 << 16,
            framework_core::Easing::EaseIn => 1 << 16,
            framework_core::Easing::EaseOut => 2 << 16,
            framework_core::Easing::CubicBezier(_, _, _, _) => 0 << 16, // Fallback to ease-in-out
        }
    }

    /// Run property changes inside a UIView animation block.
    fn animate(transition: &framework_core::Transition, changes: Rc<dyn Fn()>) {
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

    fn apply_style_to_view(view: &UIView, style: &StyleRules) {
        // Background color — skip for Metal-backed views so the
        // GPU-rendered content isn't covered by an opaque fill.
        let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
        let is_metal_view: bool = unsafe {
            msg_send![&layer, isKindOfClass: objc2::class!(CAMetalLayer)]
        };
        if let Some(bg) = &style.background {
            if !is_metal_view {
                let c = color_to_uicolor(bg);
                if let Some(trans) = &style.background_transition {
                    let view_ref: Retained<UIView> = unsafe { Retained::retain(view as *const UIView as *mut UIView).unwrap() };
                    let trans = *trans;
                    animate(&trans, Rc::new(move || {
                        view_ref.setBackgroundColor(Some(&c));
                    }));
                } else {
                    view.setBackgroundColor(Some(&c));
                }
            }
        }

        // Flex direction -> UIStackView axis
        if let Some(dir) = &style.flex_direction {
            let is_stack: bool = unsafe {
                msg_send![view, isKindOfClass: objc2::class!(UIStackView)]
            };
            if is_stack {
                let axis: isize = match dir {
                    framework_core::FlexDirection::Row
                    | framework_core::FlexDirection::RowReverse => 0,     // Horizontal
                    framework_core::FlexDirection::Column
                    | framework_core::FlexDirection::ColumnReverse => 1,  // Vertical
                };
                let _: () = unsafe { msg_send![view, setAxis: axis] };
            }
        }

        // Gap -> UIStackView spacing
        if let Some(gap) = &style.gap {
            let is_stack: bool = unsafe {
                msg_send![view, isKindOfClass: objc2::class!(UIStackView)]
            };
            if is_stack {
                let px = length_to_px(gap);
                let _: () = unsafe { msg_send![view, setSpacing: px] };
            }
        }

        // Opacity
        if let Some(opacity) = style.opacity {
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

        // Corner radius — use the layer via msg_send to avoid
        // importing the full CALayer type. We use a uniform radius
        // (max of all corners) since per-corner requires layer masking.
        let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
        let radius = [
            style.border_top_left_radius.as_ref(),
            style.border_top_right_radius.as_ref(),
            style.border_bottom_left_radius.as_ref(),
            style.border_bottom_right_radius.as_ref(),
        ]
        .iter()
        .filter_map(|r| r.map(|l| length_to_px(l)))
        .fold(0.0_f64, f64::max);
        if radius > 0.0 {
            let _: () = unsafe { msg_send![&layer, setCornerRadius: radius] };
            unsafe { view.setClipsToBounds(true) };
        }

        // Border width (use max of all sides for the uniform layer border)
        let border_w = [
            style.border_top_width,
            style.border_right_width,
            style.border_bottom_width,
            style.border_left_width,
        ]
        .iter()
        .filter_map(|w| *w)
        .fold(0.0_f32, f32::max);
        if border_w > 0.0 {
            let _: () = unsafe { msg_send![&layer, setBorderWidth: border_w as CGFloat] };
        }

        // Border color (pick the first defined side)
        let border_color = style
            .border_top_color
            .as_ref()
            .or(style.border_right_color.as_ref())
            .or(style.border_bottom_color.as_ref())
            .or(style.border_left_color.as_ref());
        if let Some(bc) = border_color {
            let c = color_to_uicolor(bc);
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
            // Shadows require clipsToBounds = false to render
            unsafe { view.setClipsToBounds(false) };
        }

        // Padding via layoutMargins (works on UIStackView with
        // isLayoutMarginsRelativeArrangement = YES)
        let pad_top = style.padding_top.as_ref().map(length_to_px).unwrap_or(0.0);
        let pad_left = style.padding_left.as_ref().map(length_to_px).unwrap_or(0.0);
        let pad_bottom = style.padding_bottom.as_ref().map(length_to_px).unwrap_or(0.0);
        let pad_right = style.padding_right.as_ref().map(length_to_px).unwrap_or(0.0);
        if pad_top > 0.0 || pad_left > 0.0 || pad_bottom > 0.0 || pad_right > 0.0 {
            let is_stack: bool = unsafe {
                msg_send![view, isKindOfClass: objc2::class!(UIStackView)]
            };
            if is_stack {
                let _: () = unsafe { msg_send![view, setLayoutMarginsRelativeArrangement: true] };
            }
            // UIEdgeInsets { top, left, bottom, right }
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct UIEdgeInsets {
                top: CGFloat, left: CGFloat, bottom: CGFloat, right: CGFloat,
            }
            unsafe impl Encode for UIEdgeInsets {
                const ENCODING: Encoding = Encoding::Struct(
                    "UIEdgeInsets",
                    &[
                        CGFloat::ENCODING,
                        CGFloat::ENCODING,
                        CGFloat::ENCODING,
                        CGFloat::ENCODING,
                    ],
                );
            }
            let insets = UIEdgeInsets { top: pad_top, left: pad_left, bottom: pad_bottom, right: pad_right };
            let _: () = unsafe { msg_send![view, setLayoutMargins: insets] };
        }

        // Overflow
        if let Some(overflow) = &style.overflow {
            match overflow {
                framework_core::Overflow::Hidden => unsafe { view.setClipsToBounds(true) },
                framework_core::Overflow::Visible => unsafe { view.setClipsToBounds(false) },
            }
        }

        // Height / width via Auto Layout constraints. Frame-based
        // sizing doesn't work inside UIStackView arranged subviews.
        if let Some(w) = &style.width {
            if let Length::Px(px) = w {
                let anchor: Retained<NSObject> = unsafe { msg_send_id![view, widthAnchor] };
                let c: Retained<NSObject> = unsafe {
                    msg_send_id![&anchor, constraintEqualToConstant: *px as CGFloat]
                };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            }
        }
        if let Some(h) = &style.height {
            if let Length::Px(px) = h {
                let anchor: Retained<NSObject> = unsafe { msg_send_id![view, heightAnchor] };
                let c: Retained<NSObject> = unsafe {
                    msg_send_id![&anchor, constraintEqualToConstant: *px as CGFloat]
                };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            }
        }
    }

    fn font_weight_to_uikit(weight: framework_core::FontWeight) -> CGFloat {
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

    fn apply_text_style(view: &UIView, style: &StyleRules, is_label: bool) {
        // Text color
        if let Some(color) = &style.color {
            let c = color_to_uicolor(color);
            if let Some(trans) = &style.color_transition {
                let view_ref: Retained<UIView> = unsafe { Retained::retain(view as *const UIView as *mut UIView).unwrap() };
                let trans = *trans;
                animate(&trans, Rc::new(move || {
                    let _: () = unsafe { msg_send![&view_ref, setTextColor: &*c] };
                }));
            } else {
                let _: () = unsafe { msg_send![view, setTextColor: &*c] };
            }
        }

        // Font size
        if let Some(fs) = &style.font_size {
            let size = length_to_px(fs);
            if size > 0.0 {
                let weight = style.font_weight.as_ref().copied().unwrap_or(framework_core::FontWeight::Normal);
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
        } else if let Some(weight) = &style.font_weight {
            let ui_weight = font_weight_to_uikit(*weight);
            let font: Retained<NSObject> = unsafe {
                msg_send_id![
                    objc2::class!(UIFont),
                    systemFontOfSize: 17.0 as CGFloat,
                    weight: ui_weight
                ]
            };
            let _: () = unsafe { msg_send![view, setFont: &*font] };
        }

        // Text alignment
        if let Some(ta) = &style.text_align {
            let align: isize = match ta {
                framework_core::TextAlign::Left => 0,    // NSTextAlignmentLeft
                framework_core::TextAlign::Center => 1,  // NSTextAlignmentCenter
                framework_core::TextAlign::Right => 2,   // NSTextAlignmentRight
                framework_core::TextAlign::Justify => 3, // NSTextAlignmentJustified
            };
            let _: () = unsafe { msg_send![view, setTextAlignment: align] };
        }

        // Number of lines = 0 for wrapping (UILabel only, not UITextField)
        if is_label {
            let _: () = unsafe { msg_send![view, setNumberOfLines: 0isize] };
        }
    }

    /// Pin `child` inside `parent` using Auto Layout (fills parent).
    fn pin_to_edges(parent: &UIView, child: &UIView) {
        let _: () = unsafe {
            msg_send![child, setTranslatesAutoresizingMaskIntoConstraints: false]
        };
        unsafe { parent.addSubview(child) };

        let p_top: Retained<NSObject> = unsafe { msg_send_id![parent, topAnchor] };
        let p_bot: Retained<NSObject> = unsafe { msg_send_id![parent, bottomAnchor] };
        let p_lead: Retained<NSObject> = unsafe { msg_send_id![parent, leadingAnchor] };
        let p_trail: Retained<NSObject> = unsafe { msg_send_id![parent, trailingAnchor] };
        let c_top: Retained<NSObject> = unsafe { msg_send_id![child, topAnchor] };
        let c_bot: Retained<NSObject> = unsafe { msg_send_id![child, bottomAnchor] };
        let c_lead: Retained<NSObject> = unsafe { msg_send_id![child, leadingAnchor] };
        let c_trail: Retained<NSObject> = unsafe { msg_send_id![child, trailingAnchor] };

        for (a, b) in [(&c_top, &p_top), (&c_bot, &p_bot), (&c_lead, &p_lead), (&c_trail, &p_trail)] {
            let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
            let _: () = unsafe { msg_send![&c, setActive: true] };
        }
    }

    /// Mount a framework screen node into a UIViewController properly:
    /// adds as subview pinned to the VC's view with Auto Layout.
    /// Pins top/leading/trailing to edges, but uses `>=` for bottom so
    /// content packs to the top instead of stretching to fill.
    fn mount_screen_in_vc(mtm: MainThreadMarker, screen: &UIView) -> Retained<UIViewController> {
        let vc = unsafe { UIViewController::new(mtm) };
        let vc_view = vc.view().expect("vc.view");

        let _: () = unsafe {
            msg_send![screen, setTranslatesAutoresizingMaskIntoConstraints: false]
        };
        unsafe { vc_view.addSubview(screen) };

        // Pin top/leading/trailing to safe area, bottom >= safe area
        let guide: Retained<NSObject> = unsafe { msg_send_id![&vc_view, safeAreaLayoutGuide] };

        let g_top: Retained<NSObject> = unsafe { msg_send_id![&guide, topAnchor] };
        let g_bot: Retained<NSObject> = unsafe { msg_send_id![&guide, bottomAnchor] };
        let g_lead: Retained<NSObject> = unsafe { msg_send_id![&guide, leadingAnchor] };
        let g_trail: Retained<NSObject> = unsafe { msg_send_id![&guide, trailingAnchor] };

        let s_top: Retained<NSObject> = unsafe { msg_send_id![screen, topAnchor] };
        let s_bot: Retained<NSObject> = unsafe { msg_send_id![screen, bottomAnchor] };
        let s_lead: Retained<NSObject> = unsafe { msg_send_id![screen, leadingAnchor] };
        let s_trail: Retained<NSObject> = unsafe { msg_send_id![screen, trailingAnchor] };

        // Top, leading, trailing: equal
        for (a, b) in [(&s_top, &g_top), (&s_lead, &g_lead), (&s_trail, &g_trail)] {
            let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
            let _: () = unsafe { msg_send![&c, setActive: true] };
        }
        // Bottom: screen.bottom <= guide.bottom (content packs to top)
        let c_bot: Retained<NSObject> = unsafe {
            msg_send_id![&s_bot, constraintLessThanOrEqualToAnchor: &*g_bot]
        };
        let _: () = unsafe { msg_send![&c_bot, setActive: true] };

        // Match width to ensure full-width layout
        let s_width: Retained<NSObject> = unsafe { msg_send_id![screen, widthAnchor] };
        let g_width: Retained<NSObject> = unsafe { msg_send_id![&guide, widthAnchor] };
        let c_w: Retained<NSObject> = unsafe {
            msg_send_id![&s_width, constraintEqualToAnchor: &*g_width]
        };
        let _: () = unsafe { msg_send![&c_w, setActive: true] };

        vc
    }

    // =========================================================================
    // Backend trait implementation
    // =========================================================================

    impl Backend for IosBackend {
        type Node = IosNode;

        fn create_view(&mut self) -> Self::Node {
            let stack = unsafe { UIStackView::new(self.mtm) };
            // Vertical axis, fill alignment (children stretch to stack
            // width), equalSpacing distribution (content packs to top,
            // no stretching of individual views).
            let _: () = unsafe { msg_send![&stack, setAxis: 1isize] };          // Vertical
            let _: () = unsafe { msg_send![&stack, setAlignment: 0isize] };      // Fill
            let _: () = unsafe { msg_send![&stack, setDistribution: 0isize] };   // Fill
            IosNode::View(Retained::into_super(stack))
        }

        fn create_text(&mut self, content: &str) -> Self::Node {
            let label = unsafe { UILabel::new(self.mtm) };
            let ns_text = NSString::from_str(content);
            unsafe { label.setText(Some(&ns_text)) };
            // Allow multiline by default
            let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };
            IosNode::Label(label)
        }

        fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node {
            let button = unsafe {
                UIButton::buttonWithType(UIButtonType::System, self.mtm)
            };
            let ns_label = NSString::from_str(label);
            // setTitle:forState: — state 0 = UIControlStateNormal
            let _: () = unsafe { msg_send![&button, setTitle: &*ns_label, forState: 0u64] };

            // Create a callback target and wire it via addTarget:action:forControlEvents:
            let target = CallbackTarget::new(self.mtm, on_click);
            let sel = objc2::sel!(invoke);
            // UIControlEventTouchUpInside = 1 << 6 = 64
            let _: () = unsafe {
                msg_send![&button, addTarget: &*target, action: sel, forControlEvents: 64u64]
            };
            self.retain_target(&target);

            IosNode::Button(button)
        }

        fn update_button_label(&mut self, node: &Self::Node, label: &str) {
            if let IosNode::Button(button) = node {
                let ns = NSString::from_str(label);
                let _: () = unsafe { msg_send![button, setTitle: &*ns, forState: 0u64] };
            }
        }

        fn create_text_input(
            &mut self,
            initial_value: &str,
            placeholder: Option<&str>,
            on_change: Rc<dyn Fn(String)>,
        ) -> Self::Node {
            let field = unsafe { UITextField::new(self.mtm) };
            let ns_val = NSString::from_str(initial_value);
            unsafe { field.setText(Some(&ns_val)) };

            if let Some(ph) = placeholder {
                let ns_ph = NSString::from_str(ph);
                unsafe { field.setPlaceholder(Some(&ns_ph)) };
            }

            // Default styling: rounded rect border style
            let _: () = unsafe { msg_send![&field, setBorderStyle: 3isize] }; // UITextBorderStyleRoundedRect

            // Wire editing-changed event
            let target = StringCallbackTarget::new(self.mtm, on_change);
            let sel = objc2::sel!(invoke:);
            // UIControlEventEditingChanged = 1 << 17 = 131072
            let _: () = unsafe {
                msg_send![&field, addTarget: &*target, action: sel, forControlEvents: 131072u64]
            };
            self.retain_target(&target);

            IosNode::TextField(field)
        }

        fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
            if let IosNode::TextField(field) = node {
                // Avoid updating if the field already has this value (prevents
                // cursor jump during typing)
                let current: Option<Retained<NSString>> = unsafe { msg_send_id![field, text] };
                let current_str = current.map(|ns| ns.to_string()).unwrap_or_default();
                if current_str != value {
                    let ns = NSString::from_str(value);
                    unsafe { field.setText(Some(&ns)) };
                }
            }
        }

        fn create_toggle(
            &mut self,
            initial_value: bool,
            on_change: Rc<dyn Fn(bool)>,
        ) -> Self::Node {
            let switch = unsafe { UISwitch::new(self.mtm) };
            unsafe { switch.setOn_animated(initial_value, false) };

            let target = BoolCallbackTarget::new(self.mtm, on_change);
            let sel = objc2::sel!(invoke:);
            // UIControlEventValueChanged = 1 << 12 = 4096
            let _: () = unsafe {
                msg_send![&switch, addTarget: &*target, action: sel, forControlEvents: 4096u64]
            };
            self.retain_target(&target);

            IosNode::Switch(switch)
        }

        fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
            if let IosNode::Switch(switch) = node {
                let current: bool = unsafe { msg_send![switch, isOn] };
                if current != value {
                    unsafe { switch.setOn_animated(value, true) };
                }
            }
        }

        fn create_scroll_view(&mut self, _horizontal: bool) -> Self::Node {
            let scroll = unsafe { UIScrollView::new(self.mtm) };

            // Inner UIStackView for children — gives them automatic layout.
            let inner = unsafe { UIStackView::new(self.mtm) };
            let _: () = unsafe { msg_send![&inner, setAxis: 1isize] }; // Vertical
            let _: () = unsafe { msg_send![&inner, setAlignment: 0isize] }; // Fill
            let _: () = unsafe {
                msg_send![&inner, setTranslatesAutoresizingMaskIntoConstraints: false]
            };
            unsafe { scroll.addSubview(&inner) };

            // Pin inner to scroll view's content layout guide
            let content_guide: Retained<NSObject> = unsafe { msg_send_id![&scroll, contentLayoutGuide] };
            let frame_guide: Retained<NSObject> = unsafe { msg_send_id![&scroll, frameLayoutGuide] };

            // Pin all edges of inner to content guide
            let inner_top: Retained<NSObject> = unsafe { msg_send_id![&inner, topAnchor] };
            let inner_bottom: Retained<NSObject> = unsafe { msg_send_id![&inner, bottomAnchor] };
            let inner_leading: Retained<NSObject> = unsafe { msg_send_id![&inner, leadingAnchor] };
            let inner_trailing: Retained<NSObject> = unsafe { msg_send_id![&inner, trailingAnchor] };

            let cg_top: Retained<NSObject> = unsafe { msg_send_id![&content_guide, topAnchor] };
            let cg_bottom: Retained<NSObject> = unsafe { msg_send_id![&content_guide, bottomAnchor] };
            let cg_leading: Retained<NSObject> = unsafe { msg_send_id![&content_guide, leadingAnchor] };
            let cg_trailing: Retained<NSObject> = unsafe { msg_send_id![&content_guide, trailingAnchor] };

            for (a, b) in [(&inner_top, &cg_top), (&inner_bottom, &cg_bottom),
                           (&inner_leading, &cg_leading), (&inner_trailing, &cg_trailing)] {
                let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
                let _: () = unsafe { msg_send![&c, setActive: true] };
            }

            // Match inner width to scroll view frame width (vertical scroll only)
            let inner_width: Retained<NSObject> = unsafe { msg_send_id![&inner, widthAnchor] };
            let frame_width: Retained<NSObject> = unsafe { msg_send_id![&frame_guide, widthAnchor] };
            let wc: Retained<NSObject> = unsafe { msg_send_id![&inner_width, constraintEqualToAnchor: &*frame_width] };
            let _: () = unsafe { msg_send![&wc, setActive: true] };

            let inner_view: Retained<UIView> = Retained::into_super(inner);
            let key = &*scroll as *const UIScrollView as *const UIView as usize;
            self.scroll_view_inner.insert(key, inner_view);

            IosNode::ScrollView(scroll)
        }

        fn create_slider(
            &mut self,
            initial_value: f32,
            min: f32,
            max: f32,
            _step: Option<f32>,
            on_change: Rc<dyn Fn(f32)>,
        ) -> Self::Node {
            let slider = unsafe { UISlider::new(self.mtm) };
            unsafe {
                slider.setMinimumValue(min);
                slider.setMaximumValue(max);
                slider.setValue_animated(initial_value, false);
            };

            let target = FloatCallbackTarget::new(self.mtm, on_change);
            let sel = objc2::sel!(invoke:);
            // UIControlEventValueChanged = 4096
            let _: () = unsafe {
                msg_send![&slider, addTarget: &*target, action: sel, forControlEvents: 4096u64]
            };
            self.retain_target(&target);

            IosNode::Slider(slider)
        }

        fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
            if let IosNode::Slider(slider) = node {
                unsafe { slider.setValue_animated(value, true) };
            }
        }

        fn create_activity_indicator(
            &mut self,
            size: ActivityIndicatorSize,
            color: Option<&Color>,
        ) -> Self::Node {
            let style = match size {
                ActivityIndicatorSize::Small => UIActivityIndicatorViewStyle::Medium,
                ActivityIndicatorSize::Large => UIActivityIndicatorViewStyle::Large,
            };
            let indicator = unsafe {
                UIActivityIndicatorView::initWithActivityIndicatorStyle(
                    self.mtm.alloc(),
                    style,
                )
            };
            if let Some(c) = color {
                let ui_color = color_to_uicolor(c);
                unsafe { indicator.setColor(Some(&ui_color)) };
            }
            unsafe { indicator.startAnimating() };

            IosNode::ActivityIndicator(indicator)
        }

        fn create_graphics(
            &mut self,
            mut on_ready: OnReady,
            _on_resize: OnResize,
            _on_lost: OnLost,
        ) -> Self::Node {
            let metal_view = MetalView::new(self.mtm);
            // Upcast to UIView for the node
            let view: Retained<UIView> = Retained::into_super(metal_view);

            // Make the view transparent so Metal content isn't
            // hidden behind UIView's background fill.
            let clear = unsafe { UIColor::clearColor() };
            view.setBackgroundColor(Some(&clear));
            let _: () = unsafe { msg_send![&view, setOpaque: false] };

            // Set the Metal layer's contentsScale to the screen scale
            // so wgpu renders at native retina resolution.
            let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
            let screen_scale: CGFloat = unsafe {
                let screen: Retained<NSObject> = msg_send_id![objc2::class!(UIScreen), mainScreen];
                msg_send![&screen, scale]
            };
            let _: () = unsafe { msg_send![&layer, setContentsScale: screen_scale] };

            let view_ptr = &*view as *const UIView as *mut std::ffi::c_void;

            let provider = Arc::new(IosSurfaceProvider { view: view_ptr });
            let surface = GraphicsSurface::new(provider);

            // Defer on_ready to the next run loop iteration so Auto
            // Layout has sized the view (it's still (0,0) at this point).
            // We use a CallbackTarget + performSelector:afterDelay: which
            // fires after the current layout pass completes.
            let view_clone = view.clone();
            let on_ready_cell: Rc<RefCell<Option<OnReady>>> = Rc::new(RefCell::new(Some(on_ready)));
            let ready_callback: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(mut cb) = on_ready_cell.borrow_mut().take() {
                    let frame: CGRect = unsafe { msg_send![&view_clone, frame] };
                    let scale: CGFloat = unsafe { msg_send![&view_clone, contentScaleFactor] };
                    let w = (frame.size.width * scale).max(1.0) as u32;
                    let h = (frame.size.height * scale).max(1.0) as u32;
                    eprintln!("[ios-backend] create_graphics on_ready firing: {}x{} (frame: {}x{}, scale: {})", w, h, frame.size.width, frame.size.height, scale);
                    cb(OnReadyEvent {
                        surface: surface.clone(),
                        size: (w, h),
                    });
                    eprintln!("[ios-backend] on_ready callback returned");
                }
            });
            let target = CallbackTarget::new(self.mtm, ready_callback);
            let sel = objc2::sel!(invoke);
            // performSelector:withObject:afterDelay: — delay 0 defers
            // to the next run loop pass, after layout.
            let _: () = unsafe {
                msg_send![&target, performSelector: sel, withObject: std::ptr::null::<NSObject>(), afterDelay: 0.0 as CGFloat]
            };
            self.retain_target(&target);

            IosNode::View(view)
        }

        fn create_link(&mut self, config: LinkConfig) -> Self::Node {
            // On iOS, links are tappable containers. We use a UIButton
            // with the activation closure — same as a regular button.
            let button = unsafe {
                UIButton::buttonWithType(UIButtonType::System, self.mtm)
            };
            // Links don't have a visible label by default — children
            // are added as subviews by the framework. But we set an
            // accessibility label.
            let ns_route = NSString::from_str(config.route);
            let _: () = unsafe { msg_send![&button, setAccessibilityLabel: &*ns_route] };

            let target = CallbackTarget::new(self.mtm, config.on_activate);
            let sel = objc2::sel!(invoke);
            let _: () = unsafe {
                msg_send![&button, addTarget: &*target, action: sel, forControlEvents: 64u64]
            };
            self.retain_target(&target);

            IosNode::Button(button)
        }

        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            let parent_view = parent.as_view();
            let parent_key = parent_view as *const UIView as usize;
            let child_view = child.as_view();

            // If parent is a scroll view, redirect to its inner content view
            if let Some(inner) = self.scroll_view_inner.get(&parent_key) {
                // Inner is a UIStackView
                let _: () = unsafe { msg_send![inner, addArrangedSubview: child_view] };
            } else {
                // Try addArrangedSubview (works if parent is UIStackView,
                // which is our default container). If it's a plain UIView
                // this will just call addSubview via the method dispatch.
                let is_stack: bool = unsafe {
                    msg_send![parent_view, isKindOfClass: objc2::class!(UIStackView)]
                };
                if is_stack {
                    let _: () = unsafe { msg_send![parent_view, addArrangedSubview: child_view] };
                } else {
                    unsafe { parent_view.addSubview(child_view) };
                }
            }
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            match node {
                IosNode::Label(label) => {
                    let ns = NSString::from_str(content);
                    unsafe { label.setText(Some(&ns)) };
                }
                IosNode::Button(button) => {
                    let ns = NSString::from_str(content);
                    let _: () = unsafe { msg_send![button, setTitle: &*ns, forState: 0u64] };
                }
                _ => {}
            }
        }

        fn clear_children(&mut self, node: &Self::Node) {
            let parent = node.as_view();
            let subviews = parent.subviews();
            for sub in subviews.iter() {
                unsafe { sub.removeFromSuperview() };
            }
        }

        fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
            let view = node.as_view();
            apply_style_to_view(view, style);

            // Apply text-specific styles to text-bearing nodes
            match node {
                IosNode::Label(_) => apply_text_style(view, style, true),
                IosNode::Button(button) => {
                    if let Some(color) = &style.color {
                        let c = color_to_uicolor(color);
                        if let Some(trans) = &style.color_transition {
                            let btn_ref: Retained<UIButton> = button.clone();
                            let trans = *trans;
                            animate(&trans, Rc::new(move || {
                                let _: () = unsafe { msg_send![&btn_ref, setTitleColor: &*c, forState: 0u64] };
                            }));
                        } else {
                            let _: () = unsafe { msg_send![button, setTitleColor: &*c, forState: 0u64] };
                        }
                    }
                    if let Some(fs) = &style.font_size {
                        let size = length_to_px(fs);
                        if size > 0.0 {
                            let weight = style.font_weight.as_ref().copied().unwrap_or(framework_core::FontWeight::Normal);
                            let ui_weight = font_weight_to_uikit(weight);
                            let font: Retained<NSObject> = unsafe {
                                msg_send_id![
                                    objc2::class!(UIFont),
                                    systemFontOfSize: size,
                                    weight: ui_weight
                                ]
                            };
                            let title_label: Option<Retained<UILabel>> = unsafe { msg_send_id![button, titleLabel] };
                            if let Some(tl) = title_label {
                                let _: () = unsafe { msg_send![&tl, setFont: &*font] };
                            }
                        }
                    }
                }
                IosNode::TextField(_) => apply_text_style(view, style, false),
                _ => {}
            }
        }

        fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
            let enabled = !disabled;
            match node {
                IosNode::Button(b) => {
                    let _: () = unsafe { msg_send![b, setEnabled: enabled] };
                }
                IosNode::TextField(f) => {
                    let _: () = unsafe { msg_send![f, setEnabled: enabled] };
                }
                IosNode::Switch(s) => {
                    let _: () = unsafe { msg_send![s, setEnabled: enabled] };
                }
                IosNode::Slider(s) => {
                    let _: () = unsafe { msg_send![s, setEnabled: enabled] };
                }
                _ => {}
            }
        }

        // =================================================================
        // Navigator (unchanged from the spike, just integrated here)
        // =================================================================

        fn create_navigator(
            &mut self,
            callbacks: NavigatorCallbacks<Self::Node>,
            control: Rc<NavigatorControl>,
        ) -> Self::Node {
            let nav = unsafe { UINavigationController::new(self.mtm) };
            let nav_view = nav.view().expect("UINavigationController.view");

            let stack_rc: Rc<RefCell<Vec<ScreenEntry>>> = Rc::new(RefCell::new(Vec::new()));
            let entry = NavigatorEntry {
                controller: nav.clone(),
                control: control.clone(),
                stack: stack_rc.clone(),
            };
            let key = &*nav_view as *const UIView as usize;
            self.navigator_instances.insert(key, entry);

            let mtm = self.mtm;
            let nav_for_dispatch = nav.clone();
            let mount_for_dispatch = callbacks.mount_screen.clone();
            let release_for_dispatch = callbacks.release_screen.clone();
            let depth_for_dispatch = callbacks.depth_changed.clone();
            let stack_ref = stack_rc.clone();

            control.install(Box::new(move |cmd| {
                let mut stack = stack_ref.borrow_mut();
                match cmd {
                    NavCommand::Push { name, params, url: _ } => {
                        let (node, scope_id) = mount_for_dispatch(name, params);
                        let vc = mount_screen_in_vc(mtm, node.as_view());
                        unsafe { nav_for_dispatch.pushViewController_animated(&vc, true) };
                        stack.push(ScreenEntry { vc, scope_id });
                        depth_for_dispatch(stack.len());
                    }
                    NavCommand::Pop => {
                        if stack.len() <= 1 {
                            return;
                        }
                        let _ = unsafe { nav_for_dispatch.popViewControllerAnimated(true) };
                        if let Some(popped) = stack.pop() {
                            release_for_dispatch(popped.scope_id);
                        }
                        depth_for_dispatch(stack.len());
                    }
                    NavCommand::Replace { name, params, url: _ } => {
                        let (node, scope_id) = mount_for_dispatch(name, params);
                        let vc = mount_screen_in_vc(mtm, node.as_view());
                        if let Some(old) = stack.pop() {
                            release_for_dispatch(old.scope_id);
                        }
                        stack.push(ScreenEntry { vc, scope_id });
                        let vcs: Vec<Retained<UIViewController>> =
                            stack.iter().map(|e| e.vc.clone()).collect();
                        unsafe {
                            nav_for_dispatch.setViewControllers_animated(
                                &objc2_foundation::NSArray::from_vec(vcs),
                                false,
                            );
                        }
                        depth_for_dispatch(stack.len());
                    }
                    NavCommand::Reset { name, params, url: _ } => {
                        let (node, scope_id) = mount_for_dispatch(name, params);
                        let vc = mount_screen_in_vc(mtm, node.as_view());
                        while let Some(prev) = stack.pop() {
                            release_for_dispatch(prev.scope_id);
                        }
                        stack.push(ScreenEntry { vc: vc.clone(), scope_id });
                        unsafe {
                            nav_for_dispatch.setViewControllers_animated(
                                &objc2_foundation::NSArray::from_vec(vec![vc]),
                                false,
                            );
                        }
                        depth_for_dispatch(stack.len());
                    }
                }
            }));

            IosNode::View(nav_view)
        }

        fn navigator_attach_initial(
            &mut self,
            navigator: &Self::Node,
            screen: Self::Node,
            scope_id: u64,
        ) {
            let key = navigator.view_key();
            let Some(entry) = self.navigator_instances.get(&key) else {
                return;
            };
            let root_vc = mount_screen_in_vc(self.mtm, screen.as_view());
            unsafe {
                entry.controller.setViewControllers_animated(
                    &objc2_foundation::NSArray::from_vec(vec![root_vc.clone()]),
                    false,
                );
            }
            entry
                .stack
                .borrow_mut()
                .push(ScreenEntry { vc: root_vc, scope_id });
        }

        fn release_navigator(&mut self, node: &Self::Node) {
            let key = node.view_key();
            if let Some(entry) = self.navigator_instances.remove(&key) {
                drop(entry);
            }
        }

        fn make_navigator_handle(&self, node: &Self::Node) -> NavigatorHandle {
            let key = node.view_key();
            let Some(entry) = self.navigator_instances.get(&key) else {
                return NavigatorHandle::new(Rc::new(()), &IosNavigatorOps);
            };
            NavigatorHandle::with_control(Rc::new(()), &IosNavigatorOps, entry.control.clone())
        }

        fn finish(&mut self, root: Self::Node) {
            if let Some(host) = &self.host_root {
                pin_to_edges(host, root.as_view());
            }
        }
    }

    struct IosNavigatorOps;
    impl NavigatorOps for IosNavigatorOps {}
}

#[cfg(not(target_os = "ios"))]
mod imp {
    use super::*;

    pub struct IosBackend;

    impl Backend for IosBackend {
        type Node = ();

        fn create_view(&mut self) -> Self::Node {
            unreachable!("backend-ios stub: UIKit calls only on iOS target")
        }
        fn create_text(&mut self, _content: &str) -> Self::Node {
            unreachable!()
        }
        fn create_button(&mut self, _label: &str, _on_click: Rc<dyn Fn()>) -> Self::Node {
            unreachable!()
        }
        fn insert(&mut self, _parent: &mut Self::Node, _child: Self::Node) {
            unreachable!()
        }
        fn update_text(&mut self, _node: &Self::Node, _content: &str) {
            unreachable!()
        }
        fn clear_children(&mut self, _node: &Self::Node) {
            unreachable!()
        }
        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
            unreachable!()
        }
        fn finish(&mut self, _root: Self::Node) {
            unreachable!()
        }
    }
}

pub use imp::IosBackend;
