pub(crate) mod callbacks;
pub(crate) mod graphics;
pub(crate) mod handles;
pub(crate) mod icon;
pub(crate) mod navigator;
pub(crate) mod overlay;
pub(crate) mod style;
pub(crate) mod tab_drawer;

/// Platform log via NSLog. Always visible in Xcode console.
#[allow(dead_code)]
pub(crate) fn ios_log(msg: &str) {
    let ns = objc2_foundation::NSString::from_str(msg);
    // NSLog(@"%@", msg) — the %@ format avoids treating msg as a format string.
    extern "C" {
        fn NSLog(fmt: *const objc2_foundation::NSString, ...);
    }
    let fmt = objc2_foundation::NSString::from_str("%@");
    unsafe { NSLog(&*fmt, &*ns) };
}

/// Platform log with format, for timing etc.
#[allow(dead_code)]
macro_rules! ios_log {
    ($($arg:tt)*) => {
        $crate::imp::ios_log(&format!($($arg)*))
    };
}

use framework_core::primitives::activity_indicator::ActivityIndicatorSize;
use framework_core::primitives::graphics::{OnLost, OnReady, OnResize};
use framework_core::primitives::link::LinkConfig;
use framework_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, TabNavigatorCallbacks, TabsHandle,
};
use framework_core::{Backend, Color, StyleRules};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{
    UIActivityIndicatorView, UIActivityIndicatorViewStyle, UIButton, UIButtonType,
    UILabel, UIScrollView, UISlider, UIStackView, UISwitch,
    UITextField, UIView, UIViewController,
};
use std::collections::HashMap;
use std::rc::Rc;

use callbacks::{
    BoolCallbackTarget, CallbackTarget, FloatCallbackTarget, StringCallbackTarget,
};
use navigator::NavigatorEntry;
use style::{
    animate, apply_style_to_view, apply_text_style, color_to_uicolor, font_weight_to_uikit,
    length_to_px,
};
use tab_drawer::TabDrawerEntry;

// =========================================================================
// IosBackend
// =========================================================================

pub struct IosBackend {
    mtm: MainThreadMarker,
    host_root: Option<Retained<UIView>>,
    navigator_instances: HashMap<usize, NavigatorEntry>,
    tab_drawer_instances: HashMap<usize, TabDrawerEntry>,
    callback_targets: Vec<Retained<NSObject>>,
    scroll_view_inner: HashMap<usize, Retained<UIView>>,
    /// Cache of rasterized icon UIImages keyed by (icon identity, size).
    /// Icon identity = pointer address of the `paths` static slice.
    /// Size = point size as u16 (half-point granularity is enough).
    /// Only used by `render_to_uiimage` — the standalone `create_icon`
    /// uses CAShapeLayer (vector, no raster needed).
    icon_image_cache: HashMap<(usize, u16), Retained<NSObject>>,
    /// Active overlays keyed by content view pointer. Stores the
    /// presented UIViewController so release_overlay can dismiss it.
    overlay_instances: overlay::OverlayInstances,
    /// Taffy-backed flex layout tree, parallel to the UIView tree.
    /// `view_to_layout` maps a view pointer to its layout node so the
    /// `apply_style` / `insert` / `finish` paths can update it. After
    /// build, `finish` calls `layout.compute(...)` and walks the
    /// UIView tree to set each subview's `frame`.
    pub(crate) layout: native_layout::LayoutTree,
    /// Map from view pointer (as key) to (retained reference, layout node).
    /// We hold a `Retained<UIView>` so the layout pass can iterate every
    /// registered view directly — recursing through `UIView.subviews`
    /// misses subtrees that aren't yet attached to the host (e.g. a
    /// `UINavigationController`'s top VC view, which only gets added
    /// on UIKit's first layout pass, after our `finish()` returns).
    pub(crate) view_to_layout: HashMap<usize, (Retained<UIView>, native_layout::LayoutNode)>,
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
    pub(crate) fn as_view(&self) -> &UIView {
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

    pub(crate) fn view_key(&self) -> usize {
        self.as_view() as *const UIView as usize
    }
}

// =========================================================================
// Helpers
// =========================================================================

impl IosBackend {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            host_root: None,
            navigator_instances: HashMap::new(),
            tab_drawer_instances: HashMap::new(),
            callback_targets: Vec::new(),
            scroll_view_inner: HashMap::new(),
            icon_image_cache: HashMap::new(),
            overlay_instances: HashMap::new(),
            layout: native_layout::LayoutTree::new(),
            view_to_layout: HashMap::new(),
        }
    }

    /// Get or create a layout node for a UIView. Called from every
    /// `create_*` method so each native view has a corresponding
    /// node in the layout tree.
    pub(crate) fn layout_for_view(&mut self, view: &UIView) -> native_layout::LayoutNode {
        let key = view as *const UIView as usize;
        if let Some((_, node)) = self.view_to_layout.get(&key) {
            return *node;
        }
        let node = self.layout.new_node();
        let retained = unsafe {
            Retained::retain(view as *const UIView as *mut UIView).expect("retain UIView")
        };
        self.view_to_layout.insert(key, (retained, node));
        node
    }

    /// Look up an existing layout node by view pointer. Returns
    /// `None` for views that weren't created by this backend
    /// (e.g. UIKit-internal scroll view internals).
    pub(crate) fn layout_of(&self, view: &UIView) -> Option<native_layout::LayoutNode> {
        let key = view as *const UIView as usize;
        self.view_to_layout.get(&key).map(|(_, n)| *n)
    }

    pub fn set_host_root(&mut self, view: Retained<UIView>) {
        self.host_root = Some(view);
    }

    fn retain_target<T: objc2::Message>(&mut self, target: &Retained<T>) {
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

/// Pin `child` inside `parent` using Auto Layout (fills parent).
pub(crate) fn pin_to_edges(parent: &UIView, child: &UIView) {
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

/// Mount a framework screen node into a UIViewController.
/// Pins to the safe area so content sits below the nav bar and
/// above the home indicator. The navigator's header_style slot
/// handles the nav bar background color separately.
pub(crate) fn mount_screen_in_vc(mtm: MainThreadMarker, screen: &UIView) -> Retained<UIViewController> {
    let vc = unsafe { UIViewController::new(mtm) };
    let vc_view = vc.view().expect("vc.view");

    let _: () = unsafe {
        objc2::msg_send![screen, setTranslatesAutoresizingMaskIntoConstraints: false]
    };
    unsafe { vc_view.addSubview(screen) };

    // Pin to safe area (below nav bar, above home indicator)
    let guide: Retained<NSObject> = unsafe { msg_send_id![&vc_view, safeAreaLayoutGuide] };
    let g_top: Retained<NSObject> = unsafe { msg_send_id![&guide, topAnchor] };
    let g_bot: Retained<NSObject> = unsafe { msg_send_id![&guide, bottomAnchor] };
    let g_lead: Retained<NSObject> = unsafe { msg_send_id![&guide, leadingAnchor] };
    let g_trail: Retained<NSObject> = unsafe { msg_send_id![&guide, trailingAnchor] };
    let s_top: Retained<NSObject> = unsafe { msg_send_id![screen, topAnchor] };
    let s_bot: Retained<NSObject> = unsafe { msg_send_id![screen, bottomAnchor] };
    let s_lead: Retained<NSObject> = unsafe { msg_send_id![screen, leadingAnchor] };
    let s_trail: Retained<NSObject> = unsafe { msg_send_id![screen, trailingAnchor] };

    // Pin all four edges so the screen ALWAYS fills the safe area.
    // Without this on the bottom edge, a screen would size to its
    // intrinsic content height — fine for short screens but breaks
    // any child with zero intrinsic (UIScrollView, Graphics surface):
    // the parent stack collapses around the intrinsic-sized siblings
    // and the zero-intrinsic child gets nothing.
    //
    // Children that want to pack-to-top inside a tall screen can use
    // a Stack with their own layout (the framework's per-stylesheet
    // alignment rules handle this).
    for (a, b) in [(&s_top, &g_top), (&s_bot, &g_bot), (&s_lead, &g_lead), (&s_trail, &g_trail)] {
        let c: Retained<NSObject> = unsafe { msg_send_id![a, constraintEqualToAnchor: &**b] };
        let _: () = unsafe { msg_send![&c, setActive: true] };
    }

    vc
}

/// Configure a UIViewController's navigationItem and the parent
/// UINavigationBar from `ScreenOptions`. Called after mounting a
/// screen in a stack or drawer navigator.
/// Configure a UIViewController's navigationItem and the parent
/// UINavigationBar from `ScreenOptions`. Returns retained callback
/// targets that must be kept alive (caller stores or forgets them).
pub(crate) fn apply_header_options(
    vc: &UIViewController,
    options: &framework_core::ScreenOptions,
    mtm: MainThreadMarker,
) -> Vec<Retained<NSObject>> {
    let mut retained = Vec::new();

    // Hide/show header
    if let Some(false) = options.header_shown {
        let nav_ctrl: *const NSObject = unsafe { msg_send![vc, navigationController] };
        if !nav_ctrl.is_null() {
            let _: () = unsafe { msg_send![nav_ctrl, setNavigationBarHidden: true, animated: false] };
        }
        return vec![];
    }

    // Title
    if let Some(ref title) = options.title {
        let ns = NSString::from_str(title);
        let _: () = unsafe { msg_send![vc, setTitle: &*ns] };
    }

    // Left bar button
    if let Some(ref btn) = options.header_left {
        let image: Retained<NSObject> = unsafe {
            let name = NSString::from_str(&btn.icon);
            msg_send_id![objc2::class!(UIImage), systemImageNamed: &*name]
        };
        let on_press = btn.on_press.clone();
        let target = CallbackTarget::new(mtm, on_press);
        let sel = objc2::sel!(invoke);
        let bar_item: Retained<NSObject> = unsafe {
            msg_send_id![objc2::class!(UIBarButtonItem), new]
        };
        let _: () = unsafe { msg_send![&bar_item, setImage: &*image] };
        let _: () = unsafe { msg_send![&bar_item, setTarget: &*target] };
        let _: () = unsafe { msg_send![&bar_item, setAction: sel] };
        if let Some(ref tint) = btn.tint {
            let c = color_to_uicolor(tint);
            let _: () = unsafe { msg_send![&bar_item, setTintColor: &*c] };
        }
        let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
        let _: () = unsafe { msg_send![&nav_item, setLeftBarButtonItem: &*bar_item] };
        let obj: Retained<NSObject> = unsafe {
            Retained::retain(Retained::as_ptr(&target) as *mut NSObject).unwrap()
        };
        retained.push(obj);
    }

    // Right bar button
    if let Some(ref btn) = options.header_right {
        let image: Retained<NSObject> = unsafe {
            let name = NSString::from_str(&btn.icon);
            msg_send_id![objc2::class!(UIImage), systemImageNamed: &*name]
        };
        let on_press = btn.on_press.clone();
        let target = CallbackTarget::new(mtm, on_press);
        let sel = objc2::sel!(invoke);
        let bar_item: Retained<NSObject> = unsafe {
            msg_send_id![objc2::class!(UIBarButtonItem), new]
        };
        let _: () = unsafe { msg_send![&bar_item, setImage: &*image] };
        let _: () = unsafe { msg_send![&bar_item, setTarget: &*target] };
        let _: () = unsafe { msg_send![&bar_item, setAction: sel] };
        if let Some(ref tint) = btn.tint {
            let c = color_to_uicolor(tint);
            let _: () = unsafe { msg_send![&bar_item, setTintColor: &*c] };
        }
        let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
        let _: () = unsafe { msg_send![&nav_item, setRightBarButtonItem: &*bar_item] };
        let obj: Retained<NSObject> = unsafe {
            Retained::retain(Retained::as_ptr(&target) as *mut NSObject).unwrap()
        };
        retained.push(obj);
    }

    retained
}

// =========================================================================
// Backend trait implementation
// =========================================================================

impl Backend for IosBackend {
    type Node = IosNode;

    fn color_scheme(&self) -> framework_core::ColorScheme {
        // UITraitCollection.currentTraitCollection.userInterfaceStyle
        // 0 = Unspecified, 1 = Light, 2 = Dark (UIUserInterfaceStyle).
        let tc: Retained<NSObject> =
            unsafe { msg_send_id![objc2::class!(UITraitCollection), currentTraitCollection] };
        let style: isize = unsafe { msg_send![&tc, userInterfaceStyle] };
        match style {
            1 => framework_core::ColorScheme::Light,
            2 => framework_core::ColorScheme::Dark,
            _ => framework_core::ColorScheme::Auto,
        }
    }

    fn create_view(&mut self) -> Self::Node {
        // Plain UIView. Children are positioned via Taffy-computed
        // frames in `finish`. We no longer use UIStackView — its
        // arranged-subview model fights with flex semantics (no
        // flex-grow/shrink, no wrap, zero-intrinsic-collapsing
        // children, opaque constraint conflicts).
        let view = unsafe { UIView::new(self.mtm) };
        // `translatesAutoresizingMaskIntoConstraints` defaults to
        // YES on `[UIView new]`, which is what we want — frame
        // assignment becomes authoritative.
        let _ = self.layout_for_view(&view);
        IosNode::View(view)
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        let label = unsafe { UILabel::new(self.mtm) };
        let ns_text = NSString::from_str(content);
        unsafe { label.setText(Some(&ns_text)) };
        let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };

        // Seed the layout node with the label's intrinsic content size
        // so Taffy can allocate space for the text. Multi-line text is
        // still approximate (single-line measurement) — a follow-up
        // wires Taffy's measure_func into UILabel's
        // systemLayoutSizeFittingSize for proper wrap measurement.
        let layout = self.layout_for_view(&label);
        let isize: objc2_foundation::CGSize =
            unsafe { msg_send![&label, intrinsicContentSize] };
        self.layout
            .set_intrinsic_size(layout, isize.width as f32, isize.height as f32);

        IosNode::Label(label)
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: Rc<dyn Fn()>,
        leading_icon: Option<&framework_core::IconData>,
        _trailing_icon: Option<&framework_core::IconData>,
    ) -> Self::Node {
        let button = unsafe {
            UIButton::buttonWithType(UIButtonType::System, self.mtm)
        };
        let ns_label = NSString::from_str(label);
        let _: () = unsafe { msg_send![&button, setTitle: &*ns_label, forState: 0u64] };

        // Leading icon → UIButton.setImage (renders before title).
        if let Some(icon_data) = leading_icon {
            let image = icon::render_to_uiimage(
                icon_data, 20.0, &mut self.icon_image_cache,
            );
            let _: () = unsafe { msg_send![&button, setImage: &*image, forState: 0u64] };
        }

        let target = CallbackTarget::new(self.mtm, on_click);
        let sel = objc2::sel!(invoke);
        let _: () = unsafe {
            msg_send![&button, addTarget: &*target, action: sel, forControlEvents: 64u64]
        };
        self.retain_target(&target);

        // Seed layout intrinsic from UIButton.intrinsicContentSize
        // (label width + system padding, ~44pt tall).
        let layout = self.layout_for_view(&button);
        let isize: objc2_foundation::CGSize =
            unsafe { msg_send![&button, intrinsicContentSize] };
        self.layout
            .set_intrinsic_size(layout, isize.width as f32, isize.height as f32);

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

        let _: () = unsafe { msg_send![&field, setBorderStyle: 3isize] };

        let target = StringCallbackTarget::new(self.mtm, on_change);
        let sel = objc2::sel!(invoke:);
        let _: () = unsafe {
            msg_send![&field, addTarget: &*target, action: sel, forControlEvents: 131072u64]
        };
        self.retain_target(&target);

        IosNode::TextField(field)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        if let IosNode::TextField(field) = node {
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

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        let scroll = unsafe { UIScrollView::new(self.mtm) };

        // UIScrollView has zero intrinsic content size, so a parent
        // UIStackView would collapse it to 0pt height. Lower its
        // content-hugging priority on both axes so the stack
        // distributes leftover space to the scroll view. Priority 1
        // (vs. default 250 on other children) makes it the greedy one.
        // UILayoutConstraintAxisHorizontal = 0, Vertical = 1.
        let _: () = unsafe { msg_send![&scroll, setContentHuggingPriority: 1f32, forAxis: 0isize] };
        let _: () = unsafe { msg_send![&scroll, setContentHuggingPriority: 1f32, forAxis: 1isize] };

        // Always allow scroll gestures even when content fits — UIKit
        // otherwise disables them when contentSize ≤ bounds, which
        // makes the scroll view feel "dead" when content happens to
        // be short. Matches typical iOS app behavior (Settings, Mail).
        if horizontal {
            let _: () = unsafe { msg_send![&scroll, setAlwaysBounceHorizontal: true] };
        } else {
            let _: () = unsafe { msg_send![&scroll, setAlwaysBounceVertical: true] };
        }

        let inner = unsafe { UIStackView::new(self.mtm) };
        let _: () = unsafe { msg_send![&inner, setAxis: 1isize] };
        let _: () = unsafe { msg_send![&inner, setAlignment: 0isize] };
        let _: () = unsafe {
            msg_send![&inner, setTranslatesAutoresizingMaskIntoConstraints: false]
        };
        unsafe { scroll.addSubview(&inner) };

        let content_guide: Retained<NSObject> = unsafe { msg_send_id![&scroll, contentLayoutGuide] };
        let frame_guide: Retained<NSObject> = unsafe { msg_send_id![&scroll, frameLayoutGuide] };

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

    fn create_icon(
        &mut self,
        data: &framework_core::primitives::icon::IconData,
        color: Option<&Color>,
    ) -> Self::Node {
        icon::create_icon(self.mtm, data, color)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        icon::update_icon_color(node, color)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        icon::update_icon_stroke(node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: framework_core::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        icon::animate_icon_stroke(node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn make_icon_handle(&self, node: &Self::Node) -> framework_core::IconHandle {
        icon::make_handle(node)
    }

    fn create_graphics(
        &mut self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
    ) -> Self::Node {
        graphics::create_graphics(self.mtm, &mut self.callback_targets, on_ready, on_resize, on_lost)
    }

    fn create_link(&mut self, config: LinkConfig) -> Self::Node {
        // Use a UIStackView (vertical) as a tappable container so
        // child primitives (Text, etc.) render inside it. A UIButton
        // would swallow children into its internal title label layout.
        let stack = unsafe { UIStackView::new(self.mtm) };
        let _: () = unsafe { msg_send![&stack, setAxis: 1isize] };
        let _: () = unsafe { msg_send![&stack, setAlignment: 0isize] };
        let _: () = unsafe { msg_send![&stack, setDistribution: 0isize] };
        let _: () = unsafe { msg_send![&stack, setUserInteractionEnabled: true] };

        // Accessibility
        let ns_route = NSString::from_str(config.route);
        let _: () = unsafe { msg_send![&stack, setAccessibilityLabel: &*ns_route] };

        // Add a tap gesture recognizer for the link activation
        let target = CallbackTarget::new(self.mtm, config.on_activate);
        let tap_sel = objc2::sel!(invoke);
        let tap_gr = unsafe {
            objc2_ui_kit::UITapGestureRecognizer::initWithTarget_action(
                self.mtm.alloc(),
                Some(&target),
                Some(tap_sel),
            )
        };
        let _: () = unsafe { msg_send![&stack, addGestureRecognizer: &*tap_gr] };
        self.retain_target(&target);

        IosNode::View(Retained::into_super(stack))
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_view = parent.as_view();
        let parent_key = parent_view as *const UIView as usize;
        let child_view = child.as_view();
        let child_key = child_view as *const UIView as usize;

        // Overlay containers mount themselves into the host window —
        // skip the in-tree insert (see overlay::create_overlay).
        if self.overlay_instances.contains_key(&child_key) {
            return;
        }

        // ScrollView still uses an inner UIStackView for now —
        // migrating it to Taffy needs separate work (contentSize
        // management) that's bigger than the View migration.
        if let Some(inner) = self.scroll_view_inner.get(&parent_key) {
            let _: () = unsafe { msg_send![inner, addArrangedSubview: child_view] };
            return;
        }

        // Overlay parents — absolute positioning per anchor.
        if let Some(entry) = self.overlay_instances.get(&parent_key) {
            unsafe { parent_view.addSubview(child_view) };
            let anchor = entry.anchor.clone();
            overlay::apply_anchor_to_child(parent_view, child_view, &anchor);
            return;
        }

        // Default path: addSubview + add to the parallel Taffy tree.
        // Lazily allocate layout nodes for both views — covers
        // terminal primitives (Text, Button, etc.) that don't
        // pre-register, and the special-case root view passed to
        // `finish`.
        unsafe { parent_view.addSubview(child_view) };
        let p_layout = self.layout_for_view(parent_view);
        let c_layout = self.layout_for_view(child_view);
        self.layout.add_child(p_layout, c_layout);
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

        // Mirror the resolved style into the Taffy node so flex
        // properties (width/height/flex-direction/padding/gap/…) take
        // effect during the layout pass.
        let layout_node = self.layout_for_view(view);
        self.layout.set_style(layout_node, style);

        match node {
            IosNode::Label(_) => apply_text_style(view, style, true),
            IosNode::Button(button) => {
                if let Some(color) = &style.color {
                    let c = color_to_uicolor(color.value());
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
                    let size = length_to_px(fs.value());
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

    fn frame(&self, node: &Self::Node) -> Option<framework_core::primitives::overlay::ViewportRect> {
        // UIView.frame is already in superview coordinates — that's
        // the relative-to-parent rect.
        let view = node.as_view();
        let frame: objc2_foundation::CGRect = unsafe { msg_send![view, frame] };
        Some(framework_core::primitives::overlay::ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<framework_core::primitives::overlay::ViewportRect> {
        // Same conversion as `rect_of_node` in handles.rs: convert
        // bounds to window coordinates. Returns None if the view
        // isn't yet mounted in a window.
        let view = node.as_view();
        let bounds: objc2_foundation::CGRect = unsafe { msg_send![view, bounds] };
        let window: Option<Retained<UIView>> = unsafe {
            let w: *mut UIView = msg_send![view, window];
            if w.is_null() { None } else { Retained::retain(w) }
        };
        let window = window?;
        let frame_in_window: objc2_foundation::CGRect = unsafe {
            msg_send![view, convertRect: bounds, toView: &*window]
        };
        Some(framework_core::primitives::overlay::ViewportRect {
            x: frame_in_window.origin.x as f32,
            y: frame_in_window.origin.y as f32,
            width: frame_in_window.size.width as f32,
            height: frame_in_window.size.height as f32,
        })
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
    // Navigator
    // =================================================================

    fn create_navigator(
        &mut self,
        callbacks: NavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
    ) -> Self::Node {
        navigator::create_navigator(self.mtm, &mut self.navigator_instances, callbacks, control)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
    ) {
        navigator::navigator_attach_initial(self.mtm, &self.navigator_instances, navigator, screen, scope_id, options)
    }

    fn apply_navigator_header_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_header_style(&entry.controller, navigator.as_view(), style);
        }
    }

    fn apply_navigator_title_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_title_style(&entry.controller, style);
        }
    }

    fn apply_navigator_button_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.navigator_instances.get(&key) {
            navigator::apply_nav_button_style(&entry.controller, style);
        }
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        navigator::release_navigator(&mut self.navigator_instances, node)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> NavigatorHandle {
        navigator::make_navigator_handle(&self.navigator_instances, node)
    }

    // =================================================================
    // Overlay
    // =================================================================

    fn create_overlay(
        &mut self,
        anchor: framework_core::primitives::overlay::OverlayAnchor,
        backdrop: framework_core::primitives::overlay::BackdropMode,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
    ) -> Self::Node {
        let (content_view, entry) = overlay::create_overlay(
            self.mtm,
            self.host_root.as_ref(),
            anchor,
            backdrop,
            on_dismiss,
        );
        let key = &*content_view as *const UIView as usize;
        self.overlay_instances.insert(key, entry);
        IosNode::View(content_view)
    }

    fn release_overlay(&mut self, node: &Self::Node) {
        let key = IosBackend::node_key(node);
        if let Some(entry) = self.overlay_instances.remove(&key) {
            overlay::release_overlay(entry);
        }
    }

    // =================================================================
    // Handle factories — override defaults so handles carry the
    // real iOS node, enabling `AnchorableHandle::rect()` to read
    // viewport coords. Required for element-anchored overlays
    // (Popover, Select).
    // =================================================================

    fn make_button_handle(&self, node: &Self::Node) -> framework_core::ButtonHandle {
        framework_core::ButtonHandle::new(Rc::new(node.clone()), &handles::IOS_BUTTON_OPS)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> framework_core::PressableHandle {
        framework_core::PressableHandle::new(Rc::new(node.clone()), &handles::IOS_PRESSABLE_OPS)
    }

    fn make_view_handle(&self, node: &Self::Node) -> framework_core::ViewHandle {
        framework_core::ViewHandle::new(Rc::new(node.clone()), &handles::IOS_VIEW_OPS)
    }

    // =================================================================
    // Tab Navigator
    // =================================================================

    fn create_tab_navigator(
        &mut self,
        callbacks: TabNavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
    ) -> Self::Node {
        tab_drawer::create_tab_navigator(self.mtm, &mut self.tab_drawer_instances, callbacks, control)
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
    ) {
        tab_drawer::tab_navigator_attach_initial(&self.tab_drawer_instances, navigator, screen, scope_id, options)
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        tab_drawer::release_tab_navigator(&mut self.tab_drawer_instances, node)
    }

    fn make_tab_navigator_handle(&self, node: &Self::Node) -> TabsHandle {
        tab_drawer::make_tab_navigator_handle(&self.tab_drawer_instances, node)
    }

    // =================================================================
    // Drawer Navigator
    // =================================================================

    fn create_drawer_navigator(
        &mut self,
        callbacks: DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<NavigatorControl>,
    ) -> Self::Node {
        tab_drawer::create_drawer_navigator(self.mtm, &mut self.tab_drawer_instances, callbacks, control)
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::ScreenOptions,
    ) {
        tab_drawer::drawer_navigator_attach_initial(
            self.mtm, &self.tab_drawer_instances, &mut self.callback_targets,
            navigator, screen, scope_id, options,
        )
    }

    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
        tab_drawer::drawer_navigator_attach_sidebar(
            self.mtm, &self.tab_drawer_instances, &mut self.callback_targets,
            navigator, sidebar,
        )
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        tab_drawer::release_drawer_navigator(&mut self.tab_drawer_instances, node)
    }

    fn make_drawer_navigator_handle(&self, node: &Self::Node) -> DrawerHandle {
        tab_drawer::make_drawer_navigator_handle(&self.tab_drawer_instances, node)
    }

    fn apply_drawer_sidebar_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<framework_core::StyleRules>,
    ) {
        let key = navigator.view_key();
        if let Some(entry) = self.tab_drawer_instances.get(&key) {
            if let Some(ref sidebar) = *entry.sidebar.borrow() {
                if let Some(ref bg) = style.background {
                    let c = style::color_to_uicolor(bg.value());
                    sidebar.setBackgroundColor(Some(&c));
                }
            }
        }
    }

    fn finish(&mut self, root: Self::Node) {
        if let Some(host) = &self.host_root {
            pin_to_edges(host, root.as_view());
        }
        self.run_layout_pass(&root);
    }
}

impl IosBackend {
    /// Run Taffy on every layout-tree root (the app root plus each
    /// screen mount that landed via `mount_screen_in_vc` rather than
    /// `insert`), then walk the UIView tree assigning computed
    /// frames. Uses the host view's bounds as the viewport (falls
    /// back to UIScreen.main.bounds when the host hasn't laid out
    /// yet).
    pub(crate) fn run_layout_pass(&mut self, _root: &IosNode) {
        let (vw, vh) = self.viewport_size();
        ios_log(&format!(
            "[layout] run_layout_pass viewport=({:.1}, {:.1}) registered_views={}",
            vw, vh, self.view_to_layout.len()
        ));
        if vw <= 0.0 || vh <= 0.0 {
            ios_log("[layout] ABORT: viewport is zero");
            return;
        }

        // Find every Taffy root. The framework root is one; each screen
        // mounted via `mount_screen_in_vc` (which bypasses
        // `Backend::insert`) is another.
        let roots: Vec<native_layout::LayoutNode> = self
            .view_to_layout
            .values()
            .map(|(_, n)| *n)
            .filter(|n| self.layout.is_root(*n))
            .collect();

        ios_log(&format!("[layout] {} taffy roots to compute", roots.len()));
        for root_node in &roots {
            self.layout.compute(*root_node, vw, vh);
        }

        // Iterate every registered view directly. Recursing via
        // `UIView.subviews` misses subtrees that aren't yet attached
        // to the framework root — e.g. a UINavigationController's
        // top VC view, which UIKit adds lazily after our `finish()`
        // returns. We hold a `Retained` ref to every view, so direct
        // iteration is both safe and exhaustive.
        let mut applied = 0usize;
        for (_, (view, layout_node)) in self.view_to_layout.iter() {
            let frame = self.layout.frame_of(*layout_node);
            let cg = objc2_foundation::CGRect {
                origin: objc2_foundation::CGPoint {
                    x: frame.x as f64,
                    y: frame.y as f64,
                },
                size: objc2_foundation::CGSize {
                    width: frame.width as f64,
                    height: frame.height as f64,
                },
            };
            let _: () = unsafe { msg_send![view, setFrame: cg] };
            applied += 1;
        }
        ios_log(&format!("[layout] apply_frames done: applied={}", applied));
    }

    /// Return the viewport size for layout. Tries host_root.bounds
    /// first (which is non-zero after UIKit has laid out the host),
    /// then UIScreen.main.bounds.
    fn viewport_size(&self) -> (f32, f32) {
        if let Some(host) = &self.host_root {
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![host, bounds] };
            if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
                return (bounds.size.width as f32, bounds.size.height as f32);
            }
        }
        // UIScreen.main.bounds — device screen size.
        unsafe {
            let screen: Retained<NSObject> =
                msg_send_id![objc2::class!(UIScreen), mainScreen];
            let bounds: objc2_foundation::CGRect = msg_send![&screen, bounds];
            (bounds.size.width as f32, bounds.size.height as f32)
        }
    }

}
