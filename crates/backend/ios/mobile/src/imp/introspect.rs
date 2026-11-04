//! Platform-native render introspection for the iOS backend.
//!
//! Reads the **live UIKit object** — the `UIView` and its backing `CALayer`,
//! a text control's resolved `UIFont`, string and color — normalized to the
//! canonical [`runtime_shared::introspect`] schema. Nothing here comes from
//! the framework's `StyleRules`, so a value a style dropped and the widget
//! kept shows up (`expect(..).to_have_native_opacity(1.0)`).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect, NSArray, NSString};
use objc2_ui_kit::UIView;
use runtime_shared::introspect::{collect_native_tree, keys, NativeNode, NativeRect, NativeValue};
use runtime_shared::BorderStyle;

use backend_ios_core::style::CGColorRef;

use super::{IosBackend, IosNode};

#[link(name = "UIKit", kind = "framework")]
extern "C" {
    static UIFontDescriptorTraitsAttribute: &'static NSString;
    static UIFontWeightTrait: &'static NSString;
}

impl IosBackend {
    /// Read the native render tree for `node`. `None` until it is in a
    /// window: its geometry is window-relative, and an element that isn't
    /// there yet has none to report.
    pub(crate) fn introspect_native_impl(&self, node: &IosNode) -> Option<NativeNode> {
        let root: Retained<UIView> =
            unsafe { Retained::retain(node.as_view() as *const UIView as *mut UIView)? };
        abs_rect(&root)?;
        Some(collect_native_tree(&root, &read_view, &subviews, &|v| self.is_framework_root(v)))
    }

    /// A descendant registered in `view_to_layout` is a separate framework
    /// element with its own read. Anything else (a `UIButton`'s title label,
    /// a per-side border bar) belongs to this primitive.
    fn is_framework_root(&self, view: &Retained<UIView>) -> bool {
        let key = (&**view) as *const UIView as usize;
        self.view_to_layout.contains_key(&key)
    }
}

/// Shallow read of one `UIView`: class, frame, what its layer holds, and for
/// a text control its string, color and font.
fn read_view(view: &Retained<UIView>) -> NativeNode {
    let frame = abs_rect(view).unwrap_or(NativeRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 });
    let mut node = NativeNode::leaf(view.class().name().to_string(), frame);

    let hidden: bool = unsafe { msg_send![&**view, isHidden] };
    node.set(keys::HIDDEN, Some(NativeValue::Flag(hidden)));
    // `alpha` alone: it IS the layer's `opacity`, so a product squares it.
    let alpha: CGFloat = unsafe { msg_send![&**view, alpha] };
    node.set(keys::OPACITY, Some(NativeValue::Number(alpha as f32)));

    let layer: Retained<NSObject> = unsafe { msg_send_id![&**view, layer] };
    let bg: CGColorRef = unsafe { msg_send![&*layer, backgroundColor] };
    node.set(keys::BACKGROUND_COLOR, cgcolor_rgba(bg).map(NativeValue::Color));

    let radius: CGFloat = unsafe { msg_send![&*layer, cornerRadius] };
    if radius > 0.0 {
        node.set(keys::CORNER_RADIUS, Some(NativeValue::Length(radius as f32)));
    }
    read_border(&layer, &mut node);

    let shadow_opacity: f32 = unsafe { msg_send![&*layer, shadowOpacity] };
    if shadow_opacity > 0.0 {
        let shadow_radius: CGFloat = unsafe { msg_send![&*layer, shadowRadius] };
        node.set(keys::SHADOW_RADIUS, Some(NativeValue::Length(shadow_radius as f32)));
        let shadow_color: CGColorRef = unsafe { msg_send![&*layer, shadowColor] };
        node.set(keys::SHADOW_COLOR, cgcolor_rgba(shadow_color).map(NativeValue::Color));
    }

    let is_text_control: bool = unsafe {
        let v = &**view;
        msg_send![v, isKindOfClass: class!(UILabel)]
            || msg_send![v, isKindOfClass: class!(UITextField)]
            || msg_send![v, isKindOfClass: class!(UITextView)]
    };
    if is_text_control {
        node.role = Some("text".to_string());
        read_text(view, &mut node);
    }
    node
}

/// The layer's own (solid) stroke, or for a dashed / dotted border, where
/// `borderWidth` stays 0 because a `CALayer` stroke can't dash, the
/// `CAShapeLayer` `backend_apple_core::border_dash_layer` strokes. The pattern
/// comes off that layer's live dash array.
fn read_border(layer: &NSObject, node: &mut NativeNode) {
    let width: CGFloat = unsafe { msg_send![layer, borderWidth] };
    if width > 0.0 {
        node.set(keys::BORDER_WIDTH, Some(NativeValue::Length(width as f32)));
        let color: CGColorRef = unsafe { msg_send![layer, borderColor] };
        node.set(keys::BORDER_COLOR, cgcolor_rgba(color).map(NativeValue::Color));
        node.set(keys::BORDER_STYLE, Some(NativeValue::Text("solid".to_string())));
        return;
    }
    let Some(dash) = backend_apple_core::border_dash_layer::read_stroke(layer) else { return };
    if dash.width > 0.0 {
        node.set(keys::BORDER_WIDTH, Some(NativeValue::Length(dash.width as f32)));
        node.set(keys::BORDER_COLOR, cgcolor_rgba(dash.color).map(NativeValue::Color));
        let keyword = match dash.style {
            BorderStyle::Solid => "solid",
            BorderStyle::Dashed => "dashed",
            BorderStyle::Dotted => "dotted",
        };
        node.set(keys::BORDER_STYLE, Some(NativeValue::Text(keyword.to_string())));
    }
}

/// `UILabel`, `UITextField` and `UITextView` share these three selectors. A
/// nil string reads as `""`: the control shows no text, which is what an empty
/// field reports on every backend.
fn read_text(view: &Retained<UIView>, node: &mut NativeNode) {
    let s: *mut NSString = unsafe { msg_send![&**view, text] };
    let text = if s.is_null() { String::new() } else { unsafe { &*s }.to_string() };
    node.set(keys::TEXT, Some(NativeValue::Text(text)));

    let color: *mut AnyObject = unsafe { msg_send![&**view, textColor] };
    if !color.is_null() {
        node.set(keys::TEXT_COLOR, uicolor_rgba(unsafe { &*color }).map(NativeValue::Color));
    }

    let font: *mut AnyObject = unsafe { msg_send![&**view, font] };
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
    node.set(keys::FONT_WEIGHT, font_weight_trait(font).map(|w| {
        NativeValue::Number(backend_apple_core::font::css_weight_from_trait(w))
    }));
}

/// `fontDescriptor[UIFontDescriptorTraitsAttribute][UIFontWeightTrait]`.
fn font_weight_trait(font: &AnyObject) -> Option<f64> {
    unsafe {
        let descriptor: *mut AnyObject = msg_send![font, fontDescriptor];
        let descriptor = descriptor.as_ref()?;
        let traits: *mut AnyObject = msg_send![descriptor, objectForKey: UIFontDescriptorTraitsAttribute];
        let traits = traits.as_ref()?;
        let weight: *mut AnyObject = msg_send![traits, objectForKey: UIFontWeightTrait];
        let weight = weight.as_ref()?;
        Some(msg_send![weight, doubleValue])
    }
}

/// The view's rect in window coordinates (logical pt, the `absolute_frame`
/// space). `None` when it isn't in a window.
fn abs_rect(view: &Retained<UIView>) -> Option<NativeRect> {
    let window: *mut UIView = unsafe { msg_send![&**view, window] };
    if window.is_null() {
        return None;
    }
    let bounds: CGRect = unsafe { msg_send![&**view, bounds] };
    let r: CGRect = unsafe { msg_send![&**view, convertRect: bounds, toView: window] };
    Some(NativeRect {
        x: r.origin.x as f32,
        y: r.origin.y as f32,
        width: r.size.width as f32,
        height: r.size.height as f32,
    })
}

fn subviews(view: &Retained<UIView>) -> Vec<Retained<UIView>> {
    let arr: Retained<NSArray<UIView>> = unsafe { msg_send_id![&**view, subviews] };
    let count: usize = unsafe { msg_send![&*arr, count] };
    (0..count).map(|i| unsafe { msg_send_id![&*arr, objectAtIndex: i] }).collect()
}

fn cgcolor_rgba(cg: CGColorRef) -> Option<[f32; 4]> {
    if cg.0.is_null() {
        return None;
    }
    let color: Retained<AnyObject> = unsafe { msg_send_id![class!(UIColor), colorWithCGColor: cg] };
    uicolor_rgba(&color)
}

/// `getRed:green:blue:alpha:` answers in extended sRGB, converting from the
/// color's own space. `None` for a color with no RGB form (a pattern).
fn uicolor_rgba(color: &AnyObject) -> Option<[f32; 4]> {
    let (mut r, mut g, mut b, mut a): (CGFloat, CGFloat, CGFloat, CGFloat) = (0.0, 0.0, 0.0, 0.0);
    let ok: bool = unsafe {
        msg_send![color, getRed: &mut r as *mut CGFloat, green: &mut g as *mut CGFloat, blue: &mut b as *mut CGFloat, alpha: &mut a as *mut CGFloat]
    };
    ok.then_some([r as f32, g as f32, b as f32, a as f32])
}
