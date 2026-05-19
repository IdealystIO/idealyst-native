//! The `Backend` trait impl for `WgpuBackend`.
//!
//! Builds and mutates the node tree + Taffy layout tree. Text state
//! (glyphon buffers + the shared `FontSystem`) lives in two shared
//! `Rc<RefCell<>>`s so this module can mutate them inline from
//! Backend methods while the renderer and Taffy measure closures
//! reach the same data through their own clones.
//!
//! The renderer (see [`crate::app`]) walks the tree on each frame
//! and submits draws — the Backend itself never talks to wgpu.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Instant;

use framework_core::{Action, Backend, ColorScheme, Easing, StateBits, StyleRules, Tokenized};
use glyphon::FontSystem;
use native_layout::{AvailableSpace, LayoutNode, LayoutTree, Size as TaffySize};

use crate::animation::{AnimProperty, Animator, TweenKey};
use crate::node::{
    new_node, NodeData, NodeKind, WgpuNode, SLIDER_DEFAULT_WIDTH, SLIDER_HEIGHT,
    TEXT_INPUT_DEFAULT_HEIGHT, TOGGLE_ANIM_MS, TOGGLE_HEIGHT, TOGGLE_WIDTH,
};
use crate::scheduler::request_redraw;
use crate::text::TextStore;

pub struct WgpuBackend {
    pub(crate) layout: LayoutTree,
    /// Root node of the rendered tree. The framework's
    /// `render(...)` calls `finish(root)` once the build walker has
    /// emitted every create/insert/apply_style for the tree; we
    /// remember the root there so the renderer can find it.
    pub(crate) roots: Vec<WgpuNode>,
    /// Shared text-buffer store. Cloned into Taffy measure closures
    /// and read by the renderer's frame walk.
    pub(crate) text: Rc<RefCell<TextStore>>,
    /// Shared font system. Same shape and reason as `text`.
    pub(crate) font_system: Rc<RefCell<FontSystem>>,
    /// Tween engine driving native-widget animations (toggle
    /// slide, future slider snap, button press scale, …). Owned
    /// here so any backend method can start a tween; sampled by
    /// the renderer in `walk()`; ticked by the host before each
    /// frame.
    pub(crate) animator: Animator,
    /// Color scheme reported to the app on init. Variants override
    /// via the constructor.
    pub(crate) color_scheme: ColorScheme,
}

impl WgpuBackend {
    pub fn new(
        text: Rc<RefCell<TextStore>>,
        font_system: Rc<RefCell<FontSystem>>,
        color_scheme: ColorScheme,
    ) -> Self {
        Self {
            layout: LayoutTree::new(),
            roots: Vec::new(),
            text,
            font_system,
            animator: Animator::new(),
            color_scheme,
        }
    }

    /// Snapshot of the active root, or `None` if nothing has been
    /// mounted yet. The renderer reads this on each frame.
    pub fn root(&self) -> Option<WgpuNode> {
        self.roots.last().cloned()
    }

    /// Install a Taffy measure callback that asks glyphon for the
    /// wrapped extent given the constraint Taffy passes in. Captures
    /// `Weak`s to the shared text + font_system stores so the
    /// closure can outlive the backend without dangling.
    fn install_text_measure(&mut self, id: LayoutNode) {
        let text_weak: Weak<RefCell<TextStore>> = Rc::downgrade(&self.text);
        let fs_weak: Weak<RefCell<FontSystem>> = Rc::downgrade(&self.font_system);
        self.layout.set_measure_fn(
            id,
            Rc::new(move |known, available| {
                let (Some(text), Some(fs)) = (text_weak.upgrade(), fs_weak.upgrade()) else {
                    return TaffySize { width: 0.0, height: 0.0 };
                };
                let mut text = text.borrow_mut();
                let mut fs = fs.borrow_mut();
                let max_w = known.width.or_else(|| match available.width {
                    AvailableSpace::Definite(v) => Some(v),
                    _ => None,
                });
                let (w, h) = text.measure(&mut fs, id, max_w);
                TaffySize { width: w, height: h }
            }),
        );
    }
}

impl Backend for WgpuBackend {
    type Node = WgpuNode;

    fn color_scheme(&self) -> ColorScheme {
        self.color_scheme
    }

    fn create_view(&mut self) -> Self::Node {
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::View, layout);
        self.roots.push(node.clone());
        node
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        let layout = self.layout.new_node();
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, content, 14.0);
        }
        self.install_text_measure(layout);
        let node = new_node(
            NodeKind::Text { content: content.to_string() },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&framework_core::primitives::icon::IconData>,
        _trailing_icon: Option<&framework_core::primitives::icon::IconData>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, label, 14.0);
        }
        self.install_text_measure(layout);
        let fire = on_click.fire.clone();
        let cb: Rc<dyn Fn()> = Rc::new(move || {
            fire();
            request_redraw();
        });
        let node = new_node(
            NodeKind::Button { label: label.to_string(), on_click: cb },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
        let layout = self.layout.new_node();
        // Wrap to request a redraw — the user's closure mutates
        // app state, but the framework doesn't drive a redraw on
        // its own; we ping winit so the next frame paints the
        // updated tree.
        let cb: Rc<dyn Fn()> = Rc::new(move || {
            on_click();
            request_redraw();
        });
        let node = new_node(NodeKind::Pressable { on_click: cb }, layout);
        self.roots.push(node.clone());
        node
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // The visible glyph buffer holds whichever of value /
        // placeholder is currently being shown. Empty value =>
        // placeholder; otherwise => value. The widget renderer
        // reads `value.is_empty()` from the node to know which
        // color to use.
        let visible = if initial_value.is_empty() {
            placeholder.unwrap_or("")
        } else {
            initial_value
        };
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, visible, 17.0);
        }
        // Pin the field's height; width flexes by default so an
        // input in a column stretches across the parent.
        self.layout
            .set_intrinsic_size(layout, -1.0, TEXT_INPUT_DEFAULT_HEIGHT);
        let node = new_node(
            NodeKind::TextInput {
                value: initial_value.to_string(),
                placeholder: placeholder.map(|s| s.to_string()),
                on_change,
            },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let layout = node.borrow().layout;
        let visible = {
            let mut data = node.borrow_mut();
            if let NodeKind::TextInput { value: stored, placeholder, .. } = &mut data.kind {
                *stored = value.to_string();
                if value.is_empty() {
                    placeholder.clone().unwrap_or_default()
                } else {
                    value.to_string()
                }
            } else {
                return;
            }
        };
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_text(&mut fs, layout, &visible);
        }
        self.layout.mark_dirty(layout);
        request_redraw();
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, TOGGLE_WIDTH, TOGGLE_HEIGHT);
        let node = new_node(
            NodeKind::Toggle { value: initial_value, on_change },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        let layout = node.borrow().layout;
        // Only animate on an actual value change. The framework
        // re-fires the controlled-value Effect even when the new
        // value matches the old, and we don't want a wasted tween.
        let old_value = match &node.borrow().kind {
            NodeKind::Toggle { value: stored, .. } => *stored,
            _ => return,
        };
        if let NodeKind::Toggle { value: stored, .. } = &mut node.borrow_mut().kind {
            *stored = value;
        }
        if old_value != value {
            let target = if value { 1.0 } else { 0.0 };
            // The rest position *before* this flip is where the
            // thumb visually sits when there's no in-flight tween.
            // Pass it so the very first animation (no existing
            // tween to sample) starts from the right place.
            let rest_before = if old_value { 1.0 } else { 0.0 };
            self.animator.animate(
                TweenKey::new(layout, AnimProperty::ToggleThumb),
                target,
                rest_before,
                TOGGLE_ANIM_MS,
                Easing::EaseOut,
                Instant::now(),
            );
        }
        request_redraw();
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, SLIDER_DEFAULT_WIDTH, SLIDER_HEIGHT);
        let node = new_node(
            NodeKind::Slider {
                value: initial_value,
                min,
                max,
                step,
                on_change,
            },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        if let NodeKind::Slider { value: stored, .. } = &mut node.borrow_mut().kind {
            *stored = value;
        }
        request_redraw();
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        let layout = self.layout.new_node();
        // Pin the scrollview's main-axis `min-size` to 0 so the
        // parent's flex layout can shrink it below its children's
        // content height (CSS flex-item-`min: auto` gotcha — the
        // Taffy default would otherwise lock the scrollview to
        // its content's intrinsic size, defeating overflow). The
        // scrollview's children get `flex_shrink: 0` in `insert`
        // so they stay at natural sizes and overflow the now
        // smaller scrollview frame. `-1.0` on the other axis
        // means "leave that axis untouched".
        if horizontal {
            self.layout.set_intrinsic_size(layout, 0.0, -1.0);
        } else {
            self.layout.set_intrinsic_size(layout, -1.0, 0.0);
        }
        let node = new_node(
            NodeKind::ScrollView {
                horizontal,
                offset_x: 0.0,
                offset_y: 0.0,
            },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::ReactiveAnchor, layout);
        self.roots.push(node.clone());
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_layout = parent.borrow().layout;
        let child_layout = child.borrow().layout;
        let parent_is_scroll =
            matches!(parent.borrow().kind, NodeKind::ScrollView { .. });
        self.layout.add_child(parent_layout, child_layout);
        parent.borrow_mut().children.push(child.clone());
        // The child is no longer orphaned — drop it from `roots`.
        self.roots.retain(|n| !Rc::ptr_eq(n, &child));

        // ScrollView children must not shrink. Taffy defaults
        // `flex_shrink: 1.0`; when the scrollview's frame is
        // constrained by its parent (typically `flex_grow: 1` to
        // fill remaining space), Taffy compresses children to fit
        // — so the content never overflows the viewport and
        // there's nothing to scroll. Pinning shrink to 0 keeps
        // children at their natural sizes; the overflow is what
        // makes the scroll machinery actually do something.
        //
        // The author can still override via an explicit
        // `flex_shrink` in their stylesheet — this only sets the
        // Taffy default for children of scrollviews.
        if parent_is_scroll {
            let no_shrink = StyleRules {
                flex_shrink: Some(Tokenized::Literal(0.0)),
                ..Default::default()
            };
            self.layout.set_style(child_layout, &no_shrink);
        }

        request_redraw();
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let layout = node.borrow().layout;
        {
            let mut data = node.borrow_mut();
            match &mut data.kind {
                NodeKind::Text { content: existing } => *existing = content.to_string(),
                NodeKind::Button { label, .. } => *label = content.to_string(),
                _ => {}
            }
        }
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_text(&mut fs, layout, content);
        }
        self.layout.mark_dirty(layout);
        request_redraw();
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let parent_layout = node.borrow().layout;
        let children: Vec<WgpuNode> = node.borrow_mut().children.drain(..).collect();
        for child in &children {
            let child_layout = child.borrow().layout;
            self.layout.remove_child(parent_layout, child_layout);
            // Recursively drop layout entries — the framework drops
            // the WgpuNode Rcs, but Taffy doesn't reference-count
            // its slots; we clean them up here so the tree doesn't
            // leak after a `when` flip.
            drop_subtree(&mut self.layout, &self.text, &mut self.animator, child);
        }
        request_redraw();
    }

    fn finish(&mut self, root: Self::Node) {
        // The framework hands us the root once the build walker has
        // emitted every create/insert/apply_style for this tree.
        // Make sure it's the only entry in `roots` (every other
        // node has either been inserted as a child or is stale).
        self.roots.retain(|n| Rc::ptr_eq(n, &root));
        if self.roots.is_empty() {
            self.roots.push(root);
        }
        request_redraw();
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        node.borrow_mut().state_setter = Some(setter);
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // Clear the setter so a stale closure can't fire on a
        // node whose style scope has dropped.
        node.borrow_mut().state_setter = None;
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let layout = node.borrow().layout;
        self.layout.set_style(layout, style);

        // Snapshot the colors that have transitions declared
        // *before* applying the new style. If the property's old
        // value differs from the new one, start a color tween via
        // the animator. Otherwise the value snaps as it did before.
        //
        // `had_prior_style` distinguishes the very first apply on
        // a node (no "old" value to lerp from — snap to initial)
        // from later re-applies driven by theme swap or state
        // overlay flip.
        let had_prior_style = node.borrow().style.is_some();
        let old_render = node.borrow().render.clone();

        let (is_text, font_size, new_render) = {
            let mut data = node.borrow_mut();
            data.render.apply(style);
            data.style = Some(style.clone());
            (
                matches!(data.kind, NodeKind::Text { .. } | NodeKind::Button { .. }),
                data.render.font_size,
                data.render.clone(),
            )
        };

        if had_prior_style {
            let now = Instant::now();
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BackgroundColor,
                old_render.background.unwrap_or([0.0; 4]),
                new_render.background.unwrap_or([0.0; 4]),
                style.background_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::TextColor,
                old_render.color,
                new_render.color,
                style.color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderTopColor,
                old_render.border_color[0],
                new_render.border_color[0],
                style.border_top_color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderRightColor,
                old_render.border_color[1],
                new_render.border_color[1],
                style.border_right_color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderBottomColor,
                old_render.border_color[2],
                new_render.border_color[2],
                style.border_bottom_color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderLeftColor,
                old_render.border_color[3],
                new_render.border_color[3],
                style.border_left_color_transition.as_ref(),
                now,
            );
        }

        if is_text {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_font_size(&mut fs, layout, font_size);
            drop(text);
            drop(fs);
            self.layout.mark_dirty(layout);
        }
        request_redraw();
    }
}

/// Start a color tween for `property` on `node` if the supplied
/// transition spec exists and the value actually changed. No-op
/// otherwise (the new value already lives in `RenderStyle` and
/// will be sampled as the fallback).
fn maybe_animate_color(
    animator: &mut Animator,
    node: LayoutNode,
    property: AnimProperty,
    old_value: [f32; 4],
    new_value: [f32; 4],
    transition: Option<&framework_core::Transition>,
    now: Instant,
) {
    let Some(t) = transition else { return };
    animator.animate_color(
        TweenKey::new(node, property),
        new_value,
        old_value,
        t.duration_ms,
        t.easing,
        now,
    );
}

fn drop_subtree(
    layout: &mut LayoutTree,
    text: &Rc<RefCell<TextStore>>,
    animator: &mut Animator,
    node: &WgpuNode,
) {
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        drop_subtree(layout, text, animator, child);
    }
    let id = node.borrow().layout;
    text.borrow_mut().remove(id);
    animator.drop_node(id);
    layout.remove_node(id);
}

/// `NodeData.style` is held even though only `render` is read on
/// the hot path — future state-overlay / transition passes will
/// re-derive from `style` without re-allocating.
const _: fn() = || {
    let _: fn(&NodeData) -> Option<&Rc<StyleRules>> = |n| n.style.as_ref();
};
