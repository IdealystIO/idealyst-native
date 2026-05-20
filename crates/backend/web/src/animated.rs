//! Web backend implementation of `Backend::set_animated_*`.
//!
//! Per-frame writes from
//! [`AnimatedValue`](framework_core::animation::AnimatedValue)
//! arrive here keyed by `(node, AnimProp)`. We:
//!
//! 1. Look up (or create) the node's [`AnimatedNodeState`] in
//!    [`WebBackend::animated_states`].
//! 2. Mutate the component the [`AnimProp`] addresses.
//! 3. Re-emit the affected CSS property *inline* on the element
//!    via `style.setProperty(...)` — bypasses the class-based
//!    stylesheet path so we don't reflow the entire ruleset every
//!    frame.
//!
//! # Transform composition
//!
//! Modern CSS has three independent transform properties:
//! `translate`, `scale`, `rotate`. Each takes its full vector
//! ("`<x> <y>`" for translate and scale, "<deg>" for rotate) so we
//! must compose state at write time — flipping a `Scale` shouldn't
//! clobber a concurrent `TranslateX`. Per-node state keeps the
//! current values of all components; on any update we re-emit the
//! affected property as a complete pair / scalar.
//!
//! # Static-style interaction
//!
//! Inline animated writes take precedence over the class-based
//! transform set by `apply_style`. So an element with both
//! `transform: scale(0.5)` in its stylesheet and
//! `Backend::set_animated_f32(_, Scale, 1.2)` ends up at 1.2 — the
//! inline writes win per CSS specificity. Authors who want the
//! static base to compose into the animated value should also
//! bind a scale-animated value seeded at 0.5.

use std::collections::HashMap;

use framework_core::animation::AnimProp;
use wasm_bindgen::JsCast;

use crate::WebBackend;

/// Mutable per-node animation state. Lives in
/// [`WebBackend::animated_states`] keyed by the node's id from
/// [`WebBackend::node_id`].
#[derive(Clone, Debug)]
pub(crate) struct AnimatedNodeState {
    pub opacity: Option<f32>,
    /// Translate components in DIPs (CSS pixels). `None` axes
    /// render as `0px`.
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,
    /// Per-axis scale; default 1.0 when `None`. `Scale` (uniform)
    /// writes to both axes.
    pub scale_x: Option<f32>,
    pub scale_y: Option<f32>,
    /// Rotation in degrees, clockwise.
    pub rotate_z: Option<f32>,
    pub background_color: Option<[f32; 4]>,
    pub foreground_color: Option<[f32; 4]>,
}

impl Default for AnimatedNodeState {
    fn default() -> Self {
        Self {
            opacity: None,
            translate_x: None,
            translate_y: None,
            scale_x: None,
            scale_y: None,
            rotate_z: None,
            background_color: None,
            foreground_color: None,
        }
    }
}

impl AnimatedNodeState {
    fn any_translate_set(&self) -> bool {
        self.translate_x.is_some() || self.translate_y.is_some()
    }

    fn any_scale_set(&self) -> bool {
        self.scale_x.is_some() || self.scale_y.is_some()
    }
}

impl WebBackend {
    /// `Backend::set_animated_f32` implementation. Routes via
    /// [`AnimProp`] family.
    pub(crate) fn impl_set_animated_f32(
        &mut self,
        node: &web_sys::Node,
        prop: AnimProp,
        value: f32,
    ) {
        // Only HtmlElements carry an inline `style` we can write
        // to. Text nodes, SVG roots in older browsers, etc. quietly
        // no-op — matches the default-trait contract that backend
        // animation support is opt-in.
        let Some(element) = node
            .clone()
            .dyn_into::<web_sys::HtmlElement>()
            .ok()
        else {
            return;
        };

        let id = self.node_id(node);
        let state = self.animated_states.entry(id).or_default();

        match prop {
            AnimProp::Opacity => {
                state.opacity = Some(value);
                let _ = element.style().set_property("opacity", &format!("{}", value));
            }
            AnimProp::TranslateX => {
                state.translate_x = Some(value);
                write_translate(&element, state);
            }
            AnimProp::TranslateY => {
                state.translate_y = Some(value);
                write_translate(&element, state);
            }
            AnimProp::Scale => {
                state.scale_x = Some(value);
                state.scale_y = Some(value);
                write_scale(&element, state);
            }
            AnimProp::ScaleX => {
                state.scale_x = Some(value);
                write_scale(&element, state);
            }
            AnimProp::ScaleY => {
                state.scale_y = Some(value);
                write_scale(&element, state);
            }
            AnimProp::RotateZ => {
                state.rotate_z = Some(value);
                let _ = element.style().set_property("rotate", &format!("{}deg", value));
            }
            // Color variants are silently ignored on the scalar path
            // — they belong on `impl_set_animated_color`. We don't
            // panic because animator code mis-routing a color prop
            // through the f32 path is a programmer error worth
            // diagnosing, but a panic would crash a running page.
            AnimProp::BackgroundColor | AnimProp::ForegroundColor => {}
        }
    }

    /// `Backend::set_animated_color` implementation.
    pub(crate) fn impl_set_animated_color(
        &mut self,
        node: &web_sys::Node,
        prop: AnimProp,
        value: [f32; 4],
    ) {
        let Some(element) = node
            .clone()
            .dyn_into::<web_sys::HtmlElement>()
            .ok()
        else {
            return;
        };

        let id = self.node_id(node);
        let state = self.animated_states.entry(id).or_default();
        let css = rgba_css(value);

        match prop {
            AnimProp::BackgroundColor => {
                state.background_color = Some(value);
                let _ = element.style().set_property("background-color", &css);
            }
            AnimProp::ForegroundColor => {
                state.foreground_color = Some(value);
                let _ = element.style().set_property("color", &css);
            }
            // Mirror the scalar path: scalar variants are ignored
            // here rather than panicking.
            AnimProp::Opacity
            | AnimProp::TranslateX
            | AnimProp::TranslateY
            | AnimProp::Scale
            | AnimProp::ScaleX
            | AnimProp::ScaleY
            | AnimProp::RotateZ => {}
        }
    }

    /// Clear per-node animation state. Called from
    /// `impl_on_node_unstyled` so we don't keep stale state alive
    /// for nodes that have been torn down. The CSS properties on
    /// the element die with the element itself, so we don't need
    /// to walk the element and call `set_property("…", "")`.
    pub(crate) fn impl_drop_animated_state(&mut self, node_id: u32) {
        self.animated_states.remove(&node_id);
    }
}

/// Re-emit the `translate` CSS property from the node's current
/// pair. Unset axes default to `0px` so a single-axis animation
/// reads as "move only on this axis."
fn write_translate(element: &web_sys::HtmlElement, state: &AnimatedNodeState) {
    if !state.any_translate_set() {
        // Nothing to write yet (all axes still default).
        return;
    }
    let tx = state.translate_x.unwrap_or(0.0);
    let ty = state.translate_y.unwrap_or(0.0);
    let _ = element
        .style()
        .set_property("translate", &format!("{}px {}px", tx, ty));
}

/// Re-emit the `scale` CSS property from the node's current pair.
/// Unset axes default to `1.0`.
fn write_scale(element: &web_sys::HtmlElement, state: &AnimatedNodeState) {
    if !state.any_scale_set() {
        return;
    }
    let sx = state.scale_x.unwrap_or(1.0);
    let sy = state.scale_y.unwrap_or(1.0);
    let _ = element
        .style()
        .set_property("scale", &format!("{} {}", sx, sy));
}

/// `[f32;4]` sRGB → CSS `rgba(r, g, b, a)`. Channels are clamped
/// to `0..=1` and the RGB triplet expanded to `0..=255` so the
/// resulting string is in the canonical sRGB byte form most
/// developers expect. Alpha stays in `0..=1` floating-point.
fn rgba_css(value: [f32; 4]) -> String {
    let r = (value[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (value[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (value[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = value[3].clamp(0.0, 1.0);
    format!("rgba({}, {}, {}, {})", r, g, b, a)
}

/// Per-node animation state map. Stored on the backend so it can
/// be cleaned up via `impl_drop_animated_state` when nodes are
/// unmounted. Keys are the same `u32` node-ids the rest of the
/// per-node state tables use (state listeners, dynamic class
/// slots, etc.).
pub(crate) type AnimatedStateMap = HashMap<u32, AnimatedNodeState>;
