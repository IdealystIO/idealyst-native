pub(crate) mod callbacks;
pub(crate) mod graphics;
pub(crate) mod navigator;
pub(crate) mod style;
pub(crate) mod tab_drawer;

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
        }
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
pub(crate) fn mount_screen_in_vc(mtm: MainThreadMarker, screen: &UIView) -> Retained<UIViewController> {
    let vc = unsafe { UIViewController::new(mtm) };
    let vc_view = vc.view().expect("vc.view");
    pin_to_edges(&vc_view, screen);
    vc
}

// =========================================================================
// Backend trait implementation
// =========================================================================

impl Backend for IosBackend {
    type Node = IosNode;

    fn create_view(&mut self) -> Self::Node {
        let stack = unsafe { UIStackView::new(self.mtm) };
        let _: () = unsafe { msg_send![&stack, setAxis: 1isize] };
        let _: () = unsafe { msg_send![&stack, setAlignment: 0isize] };
        let _: () = unsafe { msg_send![&stack, setDistribution: 0isize] };
        IosNode::View(Retained::into_super(stack))
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        let label = unsafe { UILabel::new(self.mtm) };
        let ns_text = NSString::from_str(content);
        unsafe { label.setText(Some(&ns_text)) };
        let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };
        IosNode::Label(label)
    }

    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node {
        let button = unsafe {
            UIButton::buttonWithType(UIButtonType::System, self.mtm)
        };
        let ns_label = NSString::from_str(label);
        let _: () = unsafe { msg_send![&button, setTitle: &*ns_label, forState: 0u64] };

        let target = CallbackTarget::new(self.mtm, on_click);
        let sel = objc2::sel!(invoke);
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

    fn create_scroll_view(&mut self, _horizontal: bool) -> Self::Node {
        let scroll = unsafe { UIScrollView::new(self.mtm) };

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

    fn create_graphics(
        &mut self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
    ) -> Self::Node {
        graphics::create_graphics(self.mtm, &mut self.callback_targets, on_ready, on_resize, on_lost)
    }

    fn create_link(&mut self, config: LinkConfig) -> Self::Node {
        let button = unsafe {
            UIButton::buttonWithType(UIButtonType::System, self.mtm)
        };
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

        if let Some(inner) = self.scroll_view_inner.get(&parent_key) {
            let _: () = unsafe { msg_send![inner, addArrangedSubview: child_view] };
        } else {
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
    ) {
        navigator::navigator_attach_initial(self.mtm, &self.navigator_instances, navigator, screen, scope_id)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        navigator::release_navigator(&mut self.navigator_instances, node)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> NavigatorHandle {
        navigator::make_navigator_handle(&self.navigator_instances, node)
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
    ) {
        tab_drawer::tab_navigator_attach_initial(&self.tab_drawer_instances, navigator, screen, scope_id)
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
    ) {
        tab_drawer::drawer_navigator_attach_initial(
            self.mtm, &self.tab_drawer_instances, &mut self.callback_targets,
            navigator, screen, scope_id,
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

    fn finish(&mut self, root: Self::Node) {
        if let Some(host) = &self.host_root {
            pin_to_edges(host, root.as_view());
        }
    }
}
