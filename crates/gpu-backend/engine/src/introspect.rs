//! Native render introspection for the GPU backend.
//!
//! The renderer has no widget toolkit under it: the only object a node has is
//! its `NodeData`, and each frame paints `NodeData::render` (the projection
//! `apply_style` resolves: post-token, post-state) plus any animated override.
//! So this reports what the PAINTER draws from, the position the Windows reader
//! is in for its painted nodes. A style that drops a prop shows up (the
//! projection is rebuilt without it), and so does an animated value that outlives
//! its style. A bug where this state is right and the pixels are wrong is out of
//! reach; the headless screenshot tests are the tool for that.
//!
//! Each node is one framework element, so a tree is a single node. `hidden` is
//! absent: nodes have no hidden flag (a navigator skips its inactive screens at
//! paint time).

use runtime_shared::introspect::{keys, NativeNode, NativeRect, NativeValue};
use runtime_shared::{BorderStyle, FontWeight};

use crate::backend_impl::WgpuBackend;
use crate::node::{NodeData, NodeKind, WgpuNode};

impl WgpuBackend {
    pub(crate) fn introspect_native_impl(&self, node: &WgpuNode) -> NativeNode {
        let rect = self.absolute_frame_impl(node).unwrap_or_default();
        let frame = NativeRect { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
        let data = node.borrow();
        let mut out = NativeNode::leaf(painted_class(&data.kind), frame);
        read_box(&data, &mut out);
        read_text(&data, &mut out);
        out
    }
}

fn painted_class(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Text { .. } => "GpuText",
        NodeKind::Button { .. } => "GpuButton",
        NodeKind::TextInput { .. } => "GpuTextInput",
        NodeKind::TextArea { .. } => "GpuTextArea",
        NodeKind::Toggle { .. } => "GpuToggle",
        NodeKind::Slider { .. } => "GpuSlider",
        NodeKind::ScrollView { .. } => "GpuScrollView",
        NodeKind::ActivityIndicator { .. } => "GpuActivityIndicator",
        NodeKind::Image { .. } => "GpuImage",
        NodeKind::Icon { .. } => "GpuIcon",
        NodeKind::Link { .. } => "GpuLink",
        _ => "GpuView",
    }
}

fn read_box(data: &NodeData, out: &mut NativeNode) {
    let render = &data.render;
    let animated = data.animated.as_deref();
    let opacity = animated.and_then(|a| a.opacity).unwrap_or(render.opacity);
    out.set(keys::OPACITY, Some(NativeValue::Number(opacity)));
    let background = animated.and_then(|a| a.background_color).or(render.background);
    out.set(keys::BACKGROUND_COLOR, background.map(NativeValue::Color));
    if render.corner_radius[0] > 0.0 {
        out.set(keys::CORNER_RADIUS, Some(NativeValue::Length(render.corner_radius[0])));
    }
    // The top side, as every reader answers: one key can't carry four sides.
    if render.border_width[0] > 0.0 {
        out.set(keys::BORDER_WIDTH, Some(NativeValue::Length(render.border_width[0])));
        out.set(keys::BORDER_COLOR, Some(NativeValue::Color(render.border_color[0])));
        let style = match render.border_style {
            BorderStyle::Solid => "solid",
            BorderStyle::Dashed => "dashed",
            BorderStyle::Dotted => "dotted",
        };
        out.set(keys::BORDER_STYLE, Some(NativeValue::Text(style.to_string())));
    }
    if let Some(shadow) = &render.shadow {
        out.set(keys::SHADOW_RADIUS, Some(NativeValue::Length(shadow.blur)));
        out.set(keys::SHADOW_COLOR, Some(NativeValue::Color(shadow.color)));
    }
}

fn read_text(data: &NodeData, out: &mut NativeNode) {
    let text = match &data.kind {
        NodeKind::Text { content } => content,
        NodeKind::Button { label, .. } => label,
        NodeKind::TextInput { value, .. } | NodeKind::TextArea { value, .. } => value,
        _ => return,
    };
    out.role = Some("text".to_string());
    out.set(keys::TEXT, Some(NativeValue::Text(text.clone())));
    let render = &data.render;
    let color = data.animated.as_deref().and_then(|a| a.foreground_color).unwrap_or(render.color);
    out.set(keys::TEXT_COLOR, Some(NativeValue::Color(color)));
    out.set(keys::FONT_SIZE, Some(NativeValue::Length(render.font_size)));
    out.set(keys::FONT_WEIGHT, Some(NativeValue::Number(css_weight(render.font_weight))));
    out.set(keys::FONT_FAMILY, render.font_family.clone().map(NativeValue::Text));
}

fn css_weight(weight: FontWeight) -> f32 {
    match weight {
        FontWeight::Thin => 100.0,
        FontWeight::ExtraLight => 200.0,
        FontWeight::Light => 300.0,
        FontWeight::Normal => 400.0,
        FontWeight::Medium => 500.0,
        FontWeight::SemiBold => 600.0,
        FontWeight::Bold => 700.0,
        FontWeight::ExtraBold => 800.0,
        FontWeight::Black => 900.0,
    }
}
